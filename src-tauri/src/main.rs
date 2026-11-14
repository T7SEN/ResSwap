#![forbid(unsafe_code)]
// Release builds run without a console window.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    // The start of the process for the app.started timing line; reading the clock loads no
    // library, so the DLL search limit below still comes before any load.
    resswap_lib::mark_process_start();
    // Before anything else can load a library.
    resswap_lib::system::restrict_dll_search();
    // Before any thread or the web view exists: a release build drops the variable that
    // could open a debugging port on its window (REQ-SEC-06).
    resswap_lib::system::webview_env::clear_browser_arguments();
    let action = match resswap_lib::parse_args(std::env::args_os().skip(1)) {
        Ok(action) => action,
        // Refused before the builder: no window, no plugin, nothing forwarded (REQ-SHELL-11).
        Err(resswap_lib::BadArguments) => std::process::exit(resswap_lib::BAD_ARGUMENTS),
    };
    resswap_lib::run(action);
}
