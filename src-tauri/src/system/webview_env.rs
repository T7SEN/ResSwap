//! The web view's process environment. WebView2 adds the text of
//! WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS to the browser arguments of every web view in the
//! process, so a program that sets it for ResSwap could open a remote debugging port on its
//! window. A release build removes it at the start of main(); a debug build keeps it, because
//! the test runner opens the port of its own debug build that way (REQ-SEC-06).

/// The variable WebView2 reads for extra browser arguments.
pub const ADDITIONAL_ARGS_VAR: &str = "WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS";

/// Whether this build removes the variable: release builds only.
pub const fn clears_browser_arguments() -> bool {
    !cfg!(debug_assertions)
}

/// Removes WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS from this process's environment in a release
/// build; does nothing in a debug build. main() calls it right after the DLL search limit,
/// before it reads its arguments, starts any thread or builds the web view.
pub fn clear_browser_arguments() {
    if clears_browser_arguments() {
        remove_browser_arguments();
    }
}

fn remove_browser_arguments() {
    // SAFETY: std::env::remove_var is unsafe because another thread could read or write the
    // environment at the same time. Its documentation for this toolchain calls it sound in a
    // single-threaded program, and always sound on Windows, the only target ResSwap builds
    // for. main() calls this before it starts any thread, before the Tauri builder and before
    // a web view exists, so no other thread of ResSwap's reads the environment yet.
    unsafe { std::env::remove_var(ADDITIONAL_ARGS_VAR) };
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The same test runs in both builds. `cargo test` (debug) shows the variable kept, as
    /// the test runner needs; `cargo test --release` shows it removed.
    #[test]
    fn req_sec_06_release_build_clears_webview_arguments() {
        // The value is a neutral placeholder: the test checks only that the variable goes.
        // SAFETY: setting a variable is sound on Windows even with other test threads
        // running, as the std documentation for this toolchain says; no other test reads
        // this variable.
        unsafe { std::env::set_var(ADDITIONAL_ARGS_VAR, "--resswap-test-argument") };
        clear_browser_arguments();
        let kept = std::env::var_os(ADDITIONAL_ARGS_VAR).is_some();
        assert_eq!(kept, cfg!(debug_assertions));
        assert_eq!(clears_browser_arguments(), !cfg!(debug_assertions));
        remove_browser_arguments();
        assert!(std::env::var_os(ADDITIONAL_ARGS_VAR).is_none());
    }
}
