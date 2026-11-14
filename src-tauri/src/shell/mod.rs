//! The Windows shell side of ResSwap: windows, the tray, hotkeys, Start with Windows and Play
//! shortcuts. This build opens windows; the others hold the traits their fakes need.

pub mod autostart;
pub mod fake;
pub mod hotkeys;
pub mod log_folder;
pub mod shortcuts;
pub mod theme;
pub mod tray;
pub mod window;
