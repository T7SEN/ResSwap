//! The store behind settings.json and journal.json: each file replaced whole through a temp
//! file in the same folder, flushed to disk before the rename (REQ-SET-04, REQ-SAFE-01). This
//! build holds the trait every store implements; the disk store joins it with the display
//! core.

#![forbid(unsafe_code)]

use std::io;

/// Whether a replace keeps the previous file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Backup {
    /// Copy the current file to `<name>.bak` first (settings.json).
    KeepLastGood,
    /// No copy (journal.json).
    Skip,
}

/// Files ResSwap keeps in its data folder, by fixed name. Memory follows a write only after
/// it succeeds, so memory and disk never disagree.
pub trait Store: Send + Sync {
    /// The bytes of `name`, or `None` when the file does not exist. Changes nothing.
    fn read(&self, name: &str) -> io::Result<Option<Vec<u8>>>;

    /// Replaces `name` with `bytes`. A failure before the rename leaves the old file as it
    /// was.
    fn replace(&self, name: &str, bytes: &[u8], backup: Backup) -> io::Result<()>;
}
