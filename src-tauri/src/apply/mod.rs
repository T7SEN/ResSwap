//! The apply pipeline: test, journal, act, read back, record (REQ-SAFE-01). This file holds
//! the `Ticket` that every call changing the PC requires; the journal, recovery and the
//! revert countdown sit in the child modules.

#![forbid(unsafe_code)]

pub mod countdown;
pub mod journal;
pub mod recovery;

pub use journal::OpenEntries;

/// Proof that the journal entry for a change is on disk. Its fields are private to this
/// module and its children, so only the pipeline can build one and no command reaches a
/// changing backend call without it.
#[derive(Debug)]
pub struct Ticket {
    change_id: String,
    _private: (),
}

impl Ticket {
    /// The change this Ticket belongs to: the change ID of its journal entry.
    pub fn change_id(&self) -> &str {
        &self.change_id
    }
}
