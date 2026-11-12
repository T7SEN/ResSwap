//! The process list: one snapshot of exe names and process IDs, and nothing else, so no code
//! here ever touches a game process (REQ-WATCH-03). The Toolhelp calls live only in this
//! module; this build holds the trait the watcher reads it through.

use crate::error::AppError;

/// One process in a snapshot.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProcessEntry {
    pub pid: u32,
    /// The exe file name, never a path.
    pub exe: String,
}

/// Every process-list read ResSwap makes.
pub trait ProcessList: Send + Sync {
    /// One snapshot of every process: exe name and process ID only. A failure fails the
    /// whole snapshot, so a short list never reads as a game that exited. Changes nothing.
    fn snapshot(&self) -> Result<Vec<ProcessEntry>, AppError>;
}
