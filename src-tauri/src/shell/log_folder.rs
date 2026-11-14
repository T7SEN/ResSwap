//! Opening the log folder for the player (REQ-SET-09): Explorer, started from Rust with
//! ShellExecuteExW and the open verb, never through the opener or shell plugin (D-003).
//! Behind a trait, so no test opens a folder.

use std::os::windows::ffi::OsStrExt;
use std::path::Path;

use windows::Win32::System::Com::{
    COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE, CoInitializeEx, CoUninitialize,
};
use windows::Win32::UI::Shell::{
    SEE_MASK_FLAG_NO_UI, SEE_MASK_NOASYNC, SHELLEXECUTEINFOW, ShellExecuteExW,
};
use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
use windows::core::{PCWSTR, w};

use crate::error::AppError;

/// Opens a folder the core picked. The window never sends a path (REQ-SEC-08).
pub trait FolderOpener: Send + Sync {
    /// Shows `folder` in Explorer. Changes nothing on the PC; a failure is INTERNAL, because
    /// no other code describes it.
    fn open(&self, folder: &Path) -> Result<(), AppError>;
}

/// The real opener: ShellExecuteExW with the open verb on the folder path, on a thread of
/// its own with COM initialised apartment-threaded, as the ShellExecuteExW documentation
/// asks ("COM should be initialized before ShellExecuteEx is called").
pub struct ShellFolderOpener;

impl FolderOpener for ShellFolderOpener {
    fn open(&self, folder: &Path) -> Result<(), AppError> {
        let mut path: Vec<u16> = folder.as_os_str().encode_wide().collect();
        if path.is_empty() || path.contains(&0) {
            return Err(AppError::internal(
                "the log folder path is empty or holds a NUL character",
            ));
        }
        path.push(0);
        let worker = std::thread::Builder::new()
            .name(String::from("resswap-open-folder"))
            .spawn(move || open_on_this_thread(&path))
            .map_err(|error| {
                AppError::internal(format!("the thread that opens the log folder: {error}"))
            })?;
        worker.join().map_err(|_| {
            AppError::internal("the thread that opens the log folder ended without a result")
        })?
    }
}

/// Runs on a fresh thread, so COM starts and ends here and no other code shares the
/// apartment. `path` ends with a NUL and holds no other.
fn open_on_this_thread(path: &[u16]) -> Result<(), AppError> {
    // SAFETY: the reserved pointer is None as CoInitializeEx requires, and this thread has
    // not initialised COM before, so the call cannot change an existing apartment.
    let com = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE) };
    let size = u32::try_from(size_of::<SHELLEXECUTEINFOW>())
        .map_err(|_| AppError::internal("SHELLEXECUTEINFOW does not fit its size field"))?;
    let mut info = SHELLEXECUTEINFOW {
        cbSize: size,
        // Return only after the open is started, and show no dialog of the shell's own: the
        // window reports a failure itself.
        fMask: SEE_MASK_NOASYNC | SEE_MASK_FLAG_NO_UI,
        lpVerb: w!("open"),
        lpFile: PCWSTR(path.as_ptr()),
        nShow: SW_SHOWNORMAL.0,
        ..Default::default()
    };
    // SAFETY: `info` is a fully initialised SHELLEXECUTEINFOW with cbSize set to its size;
    // lpVerb is a static NUL-terminated literal and lpFile points into `path`, which ends with
    // a NUL and outlives the call; no handle is requested, so nothing is left to close.
    let result = unsafe { ShellExecuteExW(&raw mut info) };
    if com.is_ok() {
        // SAFETY: balances the successful CoInitializeEx above, on the same thread.
        unsafe { CoUninitialize() };
    }
    result.map_err(|error| {
        AppError::internal(format!(
            "ShellExecuteExW could not open the log folder: {error}"
        ))
    })
}
