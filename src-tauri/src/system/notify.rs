//! The hidden top-level window that receives WM_DISPLAYCHANGE and WM_POWERBROADCAST, which a
//! message-only window never gets (REQ-DISP-23, REQ-SAFE-08). It lives on its own thread
//! with its own message loop, is created with no parent and is never shown. The window
//! procedure does no work beyond handing each notice to the core's callback, which
//! coalesces bursts and re-reads the state.

use std::cell::RefCell;
use std::ffi::c_void;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc;
use std::thread::JoinHandle;

use windows::Win32::Foundation::{GetLastError, HINSTANCE, HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetMessageW, MSG,
    PBT_APMRESUMEAUTOMATIC, PBT_APMSUSPEND, PostMessageW, PostQuitMessage, RegisterClassExW,
    UnregisterClassW, WM_CLOSE, WM_DESTROY, WM_DISPLAYCHANGE, WM_POWERBROADCAST, WNDCLASSEXW,
    WS_EX_TOOLWINDOW, WS_OVERLAPPED,
};
use windows::core::{PCWSTR, w};

use crate::error::AppError;

/// A notice the window hands to the core.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SystemEvent {
    /// WM_DISPLAYCHANGE: some monitor's mode or the layout changed. The message does not say
    /// which, so the core re-reads everything.
    DisplayChanged,
    /// WM_POWERBROADCAST with PBT_APMSUSPEND. Windows allows about two seconds, so the core
    /// only flushes the journal (REQ-SAFE-08).
    Suspending,
    /// WM_POWERBROADCAST with PBT_APMRESUMEAUTOMATIC, sent on every resume.
    Resumed,
}

/// What the window calls for each notice, on the window's own thread.
pub type SystemCallback = Arc<dyn Fn(SystemEvent) + Send + Sync>;

thread_local! {
    /// The callback of the window that runs on this thread. Only the window thread sets it.
    static CALLBACK: RefCell<Option<SystemCallback>> = const { RefCell::new(None) };
}

/// Numbers each window class, so two windows in one process never share a class name.
static CLASS_SERIAL: AtomicU32 = AtomicU32::new(0);

/// The running notice window. Dropping it closes the window, ends its message loop and
/// waits for its thread.
pub struct NotifyWindow {
    /// The window's address, kept as a number so the owner may move between threads; it is
    /// turned back into a window handle only to post WM_CLOSE.
    window: usize,
    thread: Option<JoinHandle<()>>,
}

impl NotifyWindow {
    /// Creates the hidden top-level window on a new thread and returns once it exists.
    /// `callback` runs on that thread for every notice. It must return quickly and never wait
    /// for whoever drops this window, because the drop waits for the window thread. Changes
    /// nothing on the PC.
    pub fn start(callback: SystemCallback) -> Result<Self, AppError> {
        let (ready, created) = mpsc::channel();
        let thread = std::thread::Builder::new()
            .name(String::from("resswap-notify"))
            .spawn(move || run_window(callback, &ready))
            .map_err(|error| {
                AppError::internal(format!("the display notice thread did not start: {error}"))
            })?;
        match created.recv() {
            Ok(Ok(window)) => Ok(Self {
                window,
                thread: Some(thread),
            }),
            Ok(Err(error)) => {
                join(thread);
                Err(error)
            }
            Err(_) => {
                join(thread);
                Err(AppError::internal(
                    "the display notice thread ended before its window existed",
                ))
            }
        }
    }

    fn handle(&self) -> HWND {
        HWND(std::ptr::with_exposed_provenance_mut::<c_void>(self.window))
    }
}

impl Drop for NotifyWindow {
    fn drop(&mut self) {
        // SAFETY: PostMessageW only queues a message and reads no caller memory. The handle
        // names the window this owner's thread created; if that window is already gone, the
        // call fails and nothing else happens.
        let posted = unsafe { PostMessageW(Some(self.handle()), WM_CLOSE, WPARAM(0), LPARAM(0)) };
        match (posted, self.thread.take()) {
            (Ok(()), Some(thread)) => join(thread),
            (Err(error), Some(_)) => {
                // Without the WM_CLOSE the loop may still run, so waiting could block forever.
                log::warn!("the display notice window did not take WM_CLOSE: {error}");
            }
            (_, None) => {}
        }
    }
}

fn join(thread: JoinHandle<()>) {
    if thread.join().is_err() {
        log::error!("the display notice thread ended with a panic");
    }
}

/// The notice a window message carries, if any.
fn event_for(message: u32, wparam: usize) -> Option<SystemEvent> {
    match message {
        WM_DISPLAYCHANGE => Some(SystemEvent::DisplayChanged),
        WM_POWERBROADCAST => match u32::try_from(wparam).ok()? {
            PBT_APMSUSPEND => Some(SystemEvent::Suspending),
            PBT_APMRESUMEAUTOMATIC => Some(SystemEvent::Resumed),
            _ => None,
        },
        _ => None,
    }
}

/// The class name with its serial, NUL-terminated.
fn class_name() -> Vec<u16> {
    let serial = CLASS_SERIAL.fetch_add(1, Ordering::Relaxed);
    format!("ResSwapNotify{serial}")
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect()
}

/// The window thread: register the class, create the window, report, then run the message
/// loop until WM_DESTROY posts WM_QUIT.
fn run_window(callback: SystemCallback, ready: &mpsc::Sender<Result<usize, AppError>>) {
    CALLBACK.with(|slot| *slot.borrow_mut() = Some(callback));
    let name = class_name();
    let class = PCWSTR(name.as_ptr());

    // SAFETY: a null module name asks for the handle of the exe that started this process,
    // which stays loaded for the life of the process; no caller memory is read.
    let instance = match unsafe { GetModuleHandleW(PCWSTR::null()) } {
        Ok(module) => HINSTANCE::from(module),
        Err(error) => {
            let _ = ready.send(Err(AppError::internal(format!(
                "GetModuleHandleW failed for the display notice window: {error}"
            ))));
            return;
        }
    };

    let description = WNDCLASSEXW {
        cbSize: size_of::<WNDCLASSEXW>() as u32,
        lpfnWndProc: Some(window_proc),
        hInstance: instance,
        lpszClassName: class,
        ..Default::default()
    };
    // SAFETY: `description` is a fully initialised WNDCLASSEXW with cbSize set, its class
    // name points into `name`, which outlives the class (it is unregistered below, before
    // `name` is dropped), and its window procedure is a function of this module.
    let atom = unsafe { RegisterClassExW(&description) };
    if atom == 0 {
        // SAFETY: GetLastError reads this thread's last error and takes no arguments.
        let error = unsafe { GetLastError() };
        let _ = ready.send(Err(AppError::internal(format!(
            "RegisterClassExW failed for the display notice window: {}",
            error.0
        ))));
        return;
    }

    // SAFETY: the class was registered above on this thread with this instance; the title is
    // a static wide literal; no parent (a top-level window, never HWND_MESSAGE), no menu and
    // no creation data; WS_OVERLAPPED without WS_VISIBLE keeps the window hidden.
    let created = unsafe {
        CreateWindowExW(
            WS_EX_TOOLWINDOW,
            class,
            w!("ResSwap"),
            WS_OVERLAPPED,
            0,
            0,
            0,
            0,
            None,
            None,
            Some(instance),
            None,
        )
    };
    let window = match created {
        Ok(window) => window,
        Err(error) => {
            unregister(class, instance);
            let _ = ready.send(Err(AppError::internal(format!(
                "CreateWindowExW failed for the display notice window: {error}"
            ))));
            return;
        }
    };
    if ready.send(Ok(window.0.expose_provenance())).is_err() {
        // Nobody waits for the window: close it again.
        // SAFETY: `window` was created on this thread above and is destroyed only here or by
        // its own WM_CLOSE handling, which has not run.
        let _ = unsafe { DestroyWindow(window) };
    }

    let mut message = MSG::default();
    loop {
        // SAFETY: `message` is a valid MSG on this frame; None takes messages for every
        // window of this thread and its thread messages.
        let status = unsafe { GetMessageW(&mut message, None, 0, 0) };
        match status.0 {
            0 => break,
            -1 => {
                log::error!("GetMessageW failed in the display notice window");
                // SAFETY: `window` belongs to this thread; if it is already destroyed the
                // call fails and nothing else happens.
                let _ = unsafe { DestroyWindow(window) };
                break;
            }
            _ => {
                // SAFETY: `message` was filled by GetMessageW above and is not used after
                // the call returns.
                unsafe { DispatchMessageW(&message) };
            }
        }
    }
    unregister(class, instance);
    CALLBACK.with(|slot| slot.borrow_mut().take());
}

fn unregister(class: PCWSTR, instance: HINSTANCE) {
    // SAFETY: `class` names the class this thread registered with `instance` and points at a
    // NUL-terminated string that outlives the call; no window of the class exists any more.
    let unregistered = unsafe { UnregisterClassW(class, Some(instance)) };
    if let Err(error) = unregistered {
        log::warn!("UnregisterClassW failed for the display notice window: {error}");
    }
}

/// Hands one notice to the callback. A panic in the callback is caught here, because it must
/// not cross the window procedure back into Windows.
fn notify(event: SystemEvent) {
    let callback = CALLBACK.with(|slot| slot.borrow().clone());
    if let Some(callback) = callback
        && catch_unwind(AssertUnwindSafe(|| callback(event))).is_err()
    {
        log::error!("the display notice callback panicked on {event:?}");
    }
}

extern "system" fn window_proc(
    window: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if let Some(event) = event_for(message, wparam.0) {
        notify(event);
    }
    match message {
        // "This message has no return value" (WM_DISPLAYCHANGE).
        WM_DISPLAYCHANGE => LRESULT(0),
        // "Return TRUE if an application processes this message" (WM_POWERBROADCAST).
        WM_POWERBROADCAST => LRESULT(1),
        WM_DESTROY => {
            // SAFETY: PostQuitMessage only posts WM_QUIT to this thread's queue.
            unsafe { PostQuitMessage(0) };
            LRESULT(0)
        }
        // DefWindowProcW destroys the window on WM_CLOSE, which then sends WM_DESTROY.
        // SAFETY: the arguments are the ones Windows passed to this window procedure.
        _ => unsafe { DefWindowProcW(window, message, wparam, lparam) },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use windows::Win32::UI::WindowsAndMessaging::SendMessageW;

    #[test]
    fn req_disp_23_messages_map_to_notices() {
        assert_eq!(
            event_for(WM_DISPLAYCHANGE, 32),
            Some(SystemEvent::DisplayChanged)
        );
        assert_eq!(
            event_for(WM_POWERBROADCAST, PBT_APMSUSPEND as usize),
            Some(SystemEvent::Suspending)
        );
        assert_eq!(
            event_for(WM_POWERBROADCAST, PBT_APMRESUMEAUTOMATIC as usize),
            Some(SystemEvent::Resumed)
        );
        // PBT_APMRESUMESUSPEND is 7: the core acts on PBT_APMRESUMEAUTOMATIC instead.
        assert_eq!(event_for(WM_POWERBROADCAST, 7), None);
        assert_eq!(event_for(WM_CLOSE, 0), None);
        assert_eq!(event_for(WM_DESTROY, 0), None);
    }

    // Creates and closes one hidden window in this test process and sends it messages
    // directly. Nothing is broadcast and nothing on the PC changes.
    #[test]
    fn req_disp_23_notify_window_hands_messages_to_the_callback() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let record = seen.clone();
        let callback: SystemCallback = Arc::new(move |event| {
            record.lock().unwrap().push(event);
        });
        let notify = NotifyWindow::start(callback).unwrap();
        let window = notify.handle();

        // SAFETY: `window` is the live window of `notify`, whose thread runs a message loop,
        // so each send returns once the window procedure has handled the message.
        let display = unsafe { SendMessageW(window, WM_DISPLAYCHANGE, Some(WPARAM(32)), None) };
        // SAFETY: as above.
        let suspend = unsafe {
            SendMessageW(
                window,
                WM_POWERBROADCAST,
                Some(WPARAM(PBT_APMSUSPEND as usize)),
                None,
            )
        };
        // SAFETY: as above.
        let resume = unsafe {
            SendMessageW(
                window,
                WM_POWERBROADCAST,
                Some(WPARAM(PBT_APMRESUMEAUTOMATIC as usize)),
                None,
            )
        };
        assert_eq!(display, LRESULT(0));
        assert_eq!(suspend, LRESULT(1));
        assert_eq!(resume, LRESULT(1));
        assert_eq!(
            *seen.lock().unwrap(),
            vec![
                SystemEvent::DisplayChanged,
                SystemEvent::Suspending,
                SystemEvent::Resumed
            ]
        );

        drop(notify);
        // The thread has ended, so the callback it held is gone too.
        assert_eq!(Arc::strong_count(&seen), 1);
    }
}
