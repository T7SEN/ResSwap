//! Process-wide Windows settings ResSwap makes before anything else runs, and the system
//! services behind traits: the clock, the process list and the display-change window.

pub mod clock;
pub mod fake;
pub mod handles;
pub mod notify;
pub mod processes;
pub mod registry;
pub mod webview_env;

use std::sync::OnceLock;

use windows::Win32::System::LibraryLoader::{
    LOAD_LIBRARY_SEARCH_SYSTEM32, SetDefaultDllDirectories,
};

/// Whether Windows accepted the DLL search limit; start-up logs it once the log runs.
pub static DLL_SEARCH_LIMITED: OnceLock<bool> = OnceLock::new();

/// Limits the libraries this process loads at run time to System32, so a DLL placed next to
/// ResSwap.exe is not picked up by a later load (SetDefaultDllDirectories). Called first in
/// main().
pub fn restrict_dll_search() {
    // SAFETY: SetDefaultDllDirectories takes no pointer, only a flag value the windows crate
    // defines; main() calls it once, before any other thread exists.
    let accepted = unsafe { SetDefaultDllDirectories(LOAD_LIBRARY_SEARCH_SYSTEM32) }.is_ok();
    let _ = DLL_SEARCH_LIMITED.set(accepted);
}
