//! Embeds the Windows manifest and turns every app command into a permission.

/// Every command the window may call. Each one becomes the permission `allow-<command>`
/// that a capability must grant; the `command_names_match` test compares this list with
/// `CommandName` in src/ipc/commands.rs.
const COMMANDS: &[&str] = &[
    "list_monitors",
    "get_state",
    "apply_mode",
    "confirm_mode",
    "revert_mode",
    "restore_native",
    "restore_everything",
    "list_profiles",
    "save_profile",
    "delete_profile",
    "set_auto_switch",
    "list_custom_entries",
    "add_custom_mode",
    "remove_custom_entry",
    "undo_all_changes",
    "get_settings",
    "save_settings",
    "set_hotkey",
    "set_autostart",
    "create_play_shortcut",
    "check_for_update",
    "install_update",
    "open_log_folder",
    "get_diagnostics",
    "dismiss_error",
];

fn main() {
    // Common-Controls v6 plus asInvoker, so ResSwap.exe never asks for elevation (REQ-SAFE-10).
    let windows = tauri_build::WindowsAttributes::new().app_manifest(include_str!("app.manifest"));
    let manifest = tauri_build::AppManifest::new().commands(COMMANDS);
    let attributes = tauri_build::Attributes::new()
        .windows_attributes(windows)
        .app_manifest(manifest);
    #[allow(
        clippy::expect_used,
        reason = "a build script reports a failure by panicking"
    )]
    tauri_build::try_build(attributes).expect("tauri-build failed");
    link_test_manifest();
}

/// The integration tests link the Tauri runtime, which imports from Common-Controls v6.
/// tauri-build embeds its manifest in the app exe only, so without this a test exe stops at
/// load with STATUS_ENTRYPOINT_NOT_FOUND.
#[allow(
    clippy::print_stdout,
    reason = "Cargo reads build script instructions from standard output"
)]
fn link_test_manifest() {
    println!("cargo:rustc-link-arg-tests=/MANIFEST:EMBED");
    println!(
        "cargo:rustc-link-arg-tests=/MANIFESTDEPENDENCY:type='win32' name='Microsoft.Windows.Common-Controls' version='6.0.0.0' processorArchitecture='*' publicKeyToken='6595b64144ccf1df' language='*'"
    );
}
