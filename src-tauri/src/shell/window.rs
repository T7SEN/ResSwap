//! Windows built from their tauri.conf.json entries, found by label, so each keeps its
//! capability (REQ-SHELL-13).

#![forbid(unsafe_code)]

use std::path::Path;

use tauri::{AppHandle, Manager, Runtime, WebviewWindowBuilder};

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
    let config = app
        .config()
        .app
        .windows
        .iter()
        .find(|window| window.label == MAIN)
        .ok_or_else(|| AppError::internal("tauri.conf.json has no window labelled main"))?;
    let mut builder = WebviewWindowBuilder::from_config(app, config).map_err(window_error)?;
    if let Some(folder) = webview_data {
        builder = builder.data_directory(folder.to_path_buf());
    }
    builder.build().map_err(window_error)?;
    Ok(())
}

fn window_error(error: tauri::Error) -> AppError {
    AppError::internal(format!("window: {error}"))
}
