//! Play shortcuts: a .lnk on the player's Desktop that starts ResSwap.exe with `--play <id>`,
//! and the launch of the game or launcher after the profile applies (REQ-PROF-06,
//! REQ-PROF-07).

use std::path::{Path, PathBuf};

use crate::apply::Ticket;
use crate::error::AppError;

/// What one Play shortcut holds.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShortcutSpec {
    /// The file name without `.lnk`, cleaned from the profile name.
    pub file_name: String,
    /// The running ResSwap.exe.
    pub target: PathBuf,
    /// Exactly `--play <profile-id>`.
    pub arguments: String,
    /// ResSwap.exe's folder.
    pub working_folder: PathBuf,
    /// The profile name.
    pub description: String,
}

/// Writes Play shortcuts. A write takes a Ticket, so it runs only after its journal entry is
/// on disk.
pub trait ShortcutWriter: Send + Sync {
    /// Writes the shortcut in the Desktop folder, loads it back to compare, and returns its
    /// path. Changes the PC.
    fn create(&self, ticket: &Ticket, shortcut: &ShortcutSpec) -> Result<PathBuf, AppError>;
}

/// Starts the program a Play shortcut names.
pub trait Launcher: Send + Sync {
    /// Starts the program at `path` with the open verb, no arguments and no handle kept, so
    /// ResSwap holds nothing of the game (REQ-PROF-07).
    fn start(&self, path: &Path) -> Result<(), AppError>;
}
