//! The state journal: what ResSwap changed and has not put back (REQ-SAFE-01, REQ-SAFE-02).
//! This build holds the record of open entries that every fake checks a Ticket against; the
//! journal file and its write protocol join it with the display core.

#![forbid(unsafe_code)]

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex, PoisonError};

use crate::apply::Ticket;

/// The change IDs whose journal entry is open, shared by every fake. A fake that changes
/// the PC panics when its Ticket names no open entry (`pc_changes_need_ticket`).
#[derive(Clone, Debug, Default)]
pub struct OpenEntries {
    open: Arc<Mutex<BTreeSet<String>>>,
}

impl OpenEntries {
    /// Whether the entry for `change_id` is open.
    pub fn is_open(&self, change_id: &str) -> bool {
        // The set stays valid after a panic elsewhere, so a poisoned lock is recovered.
        self.open
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .contains(change_id)
    }

    /// Panics when `ticket` names no open entry. A change with nothing on disk to undo it
    /// is a pipeline bug, so the fakes stop the test at that call.
    pub fn require_open(&self, ticket: &Ticket) {
        assert!(
            self.is_open(ticket.change_id()),
            "change {} has no open journal entry",
            ticket.change_id()
        );
    }

    /// Opens an entry and hands out its Ticket, as the journal write will.
    #[cfg(test)]
    pub(crate) fn open_for_tests(&self, change_id: &str) -> Ticket {
        self.open
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(change_id.to_owned());
        Ticket {
            change_id: change_id.to_owned(),
            _private: (),
        }
    }

    /// Closes the entry a Ticket names, as a recorded outcome will.
    #[cfg(test)]
    pub(crate) fn close_for_tests(&self, ticket: &Ticket) {
        self.open
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(ticket.change_id());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn req_safe_01_entries_open_and_close() {
        let journal = OpenEntries::default();
        let shared = journal.clone();
        let ticket = journal.open_for_tests("change-1");
        assert!(shared.is_open("change-1"));
        shared.require_open(&ticket);
        journal.close_for_tests(&ticket);
        assert!(!shared.is_open("change-1"));
    }

    #[test]
    #[should_panic(expected = "has no open journal entry")]
    fn req_safe_01_closed_entry_is_refused() {
        let journal = OpenEntries::default();
        let ticket = journal.open_for_tests("change-1");
        journal.close_for_tests(&ticket);
        journal.require_open(&ticket);
    }
}
