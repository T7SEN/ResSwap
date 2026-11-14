//! Windows built from their tauri.conf.json entries, found by label, so each keeps its
//! capability (REQ-SHELL-13): the main window, and the keep-this-mode prompt the core opens
//! for a pending change (REQ-SAFE-03, REQ-UI-06).

#![forbid(unsafe_code)]

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{self, Sender};

use tauri::{AppHandle, Manager, Runtime, WebviewWindowBuilder, WindowEvent};

use crate::error::AppError;

/// The main window's label; the "main" capability binds to it.
pub const MAIN: &str = "main";

/// The keep-this-mode prompt's label; the "keep-mode" capability binds to it.
pub const KEEP_MODE: &str = "keep-mode";

/// Opens the main window, or brings the existing one forward. `webview_data` moves the
/// WebView2 data folder, for debug runs that must write only inside the repository.
///
/// On Windows, building a window from a synchronous event handler deadlocks (tauri 2.12.5,
/// WebviewWindowBuilder::from_config, Known issues), so callers outside setup run this from
/// a task.
pub fn open_main<R: Runtime>(
    app: &AppHandle<R>,
    webview_data: Option<&Path>,
) -> Result<(), AppError> {
    if let Some(window) = app.get_webview_window(MAIN) {
        window.unminimize().map_err(window_error)?;
        window.set_focus().map_err(window_error)?;
        return Ok(());
    }
    let mut builder =
        WebviewWindowBuilder::from_config(app, window_config(app, MAIN)?).map_err(window_error)?;
    if let Some(folder) = webview_data {
        builder = builder.data_directory(folder.to_path_buf());
    }
    builder.build().map_err(window_error)?;
    Ok(())
}

/// The tauri.conf.json entry of the window labelled `label`.
fn window_config<'a, R: Runtime>(
    app: &'a AppHandle<R>,
    label: &str,
) -> Result<&'a tauri::utils::config::WindowConfig, AppError> {
    app.config()
        .app
        .windows
        .iter()
        .find(|window| window.label == label)
        .ok_or_else(|| {
            AppError::internal(format!("tauri.conf.json has no window labelled {label}"))
        })
}

fn window_error(error: tauri::Error) -> AppError {
    AppError::internal(format!("window: {error}"))
}

/// The keep-this-mode prompt as the core drives it. Neither call waits for the window, so
/// the core may make them while it holds its change lock, and the countdown never depends on
/// the window (REQ-SAFE-05).
pub trait KeepModeWindow: Send + Sync {
    /// Opens the prompt, or brings an open one forward. An error means it will not open;
    /// the countdown still runs to its revert.
    fn open(&self) -> Result<(), AppError>;
    /// Destroys the prompt when it is open. Destroying emits no close request, so it never
    /// counts as the player closing it (tauri 2.12.5, WebviewWindow::destroy).
    fn close(&self);
}

/// No prompt at all: a core without an app, as in most tests.
#[derive(Debug, Default)]
pub struct NoKeepModeWindow;

impl KeepModeWindow for NoKeepModeWindow {
    fn open(&self) -> Result<(), AppError> {
        Ok(())
    }

    fn close(&self) {}
}

/// What the window thread does next.
enum WindowOp {
    Open,
    Close,
}

/// The prompt of the running app. Opens and closes run in order on one thread of their own,
/// so a close and the next open never overtake each other, and no window is built from a
/// synchronous event handler, which deadlocks on Windows (tauri 2.12.5,
/// WebviewWindowBuilder::from_config, Known issues).
pub struct TauriKeepModeWindow {
    ops: Sender<WindowOp>,
}

impl TauriKeepModeWindow {
    /// Starts the window thread. `webview_data` is the main window's WebView2 data folder,
    /// so both windows share one browser process. `on_closed` runs when the player closes
    /// the prompt with its title-bar button; it must not wait.
    pub fn start<R: Runtime>(
        app: AppHandle<R>,
        webview_data: Option<PathBuf>,
        on_closed: Arc<dyn Fn() + Send + Sync>,
    ) -> Result<Self, AppError> {
        let (ops, queue) = mpsc::channel::<WindowOp>();
        std::thread::Builder::new()
            .name(String::from("resswap-windows"))
            .spawn(move || {
                for op in queue {
                    match op {
                        WindowOp::Open => {
                            if let Err(error) =
                                open_keep_mode(&app, webview_data.as_deref(), &on_closed)
                            {
                                log::warn!("the keep-mode window did not open: {error}");
                            }
                        }
                        WindowOp::Close => close_keep_mode(&app),
                    }
                }
            })
            .map_err(|error| {
                AppError::internal(format!("the window thread did not start: {error}"))
            })?;
        Ok(Self { ops })
    }
}

impl KeepModeWindow for TauriKeepModeWindow {
    fn open(&self) -> Result<(), AppError> {
        self.ops
            .send(WindowOp::Open)
            .map_err(|_| AppError::internal("the window thread has ended"))
    }

    fn close(&self) {
        if self.ops.send(WindowOp::Close).is_err() {
            log::warn!("the keep-mode window was not closed: the window thread has ended");
        }
    }
}

/// Builds the prompt from its config entry (label keep-mode, 440 x 240, not resizable),
/// focused when it opens so Keep and Revert work from the keyboard (REQ-A11Y-08). An open
/// prompt is only brought forward.
fn open_keep_mode<R: Runtime>(
    app: &AppHandle<R>,
    webview_data: Option<&Path>,
    on_closed: &Arc<dyn Fn() + Send + Sync>,
) -> Result<(), AppError> {
    if let Some(window) = app.get_webview_window(KEEP_MODE) {
        return window.set_focus().map_err(window_error);
    }
    let mut builder = WebviewWindowBuilder::from_config(app, window_config(app, KEEP_MODE)?)
        .map_err(window_error)?
        .focused(true);
    if let Some(folder) = webview_data {
        builder = builder.data_directory(folder.to_path_buf());
    }
    let window = builder.build().map_err(window_error)?;
    let closed = Arc::clone(on_closed);
    window.on_window_event(move |event| {
        if matches!(event, WindowEvent::CloseRequested { .. }) {
            closed();
        }
    });
    window.set_focus().map_err(window_error)
}

fn close_keep_mode<R: Runtime>(app: &AppHandle<R>) {
    if let Some(window) = app.get_webview_window(KEEP_MODE)
        && let Err(error) = window.destroy()
    {
        log::warn!("the keep-mode window was not destroyed: {error}");
    }
}
