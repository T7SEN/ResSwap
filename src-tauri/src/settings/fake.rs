//! The in-memory store every fake shares: settings.json, journal.json and their copies as
//! bytes in memory, with a call log and scripted faults. It opens no file (REQ-DISP-01).

#![forbid(unsafe_code)]

use std::collections::BTreeMap;
use std::io;
use std::sync::Mutex;

use crate::display::fake::{CallLog, FaultScript, When, lock};
use crate::settings::atomic::{Backup, Store};

/// A failure a test scripts into the store.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StoreFault {
    /// The read fails.
    ReadFails,
    /// The replace fails and the old file stays (INJ-D07, INJ-D16).
    WriteFails,
}

/// One call the store received, in order, with the file it touched.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StoreCall {
    Read(String),
    Replace { name: String, backup: Backup },
}

/// The data folder in memory.
#[derive(Default)]
pub struct MemoryStore {
    files: Mutex<BTreeMap<String, Vec<u8>>>,
    log: CallLog<StoreCall>,
    script: FaultScript<StoreFault>,
}

impl MemoryStore {
    /// An empty data folder, as on a first run.
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a fault to the script.
    pub fn script(&self, fault: StoreFault, when: When) {
        self.script.push(fault, when);
    }

    /// Every call so far, in order.
    pub fn calls(&self) -> Vec<StoreCall> {
        self.log.calls()
    }

    /// The bytes of `name` as stored now, without a call in the log.
    pub fn file(&self, name: &str) -> Option<Vec<u8>> {
        lock(&self.files).get(name).cloned()
    }
}

impl Store for MemoryStore {
    fn read(&self, name: &str) -> io::Result<Option<Vec<u8>>> {
        self.log.record(StoreCall::Read(name.to_owned()));
        if self
            .script
            .due(|fault| fault == StoreFault::ReadFails)
            .is_some()
        {
            return Err(io::Error::other("scripted fault: the store read failed"));
        }
        Ok(lock(&self.files).get(name).cloned())
    }

    fn replace(&self, name: &str, bytes: &[u8], backup: Backup) -> io::Result<()> {
        self.log.record(StoreCall::Replace {
            name: name.to_owned(),
            backup,
        });
        if self
            .script
            .due(|fault| fault == StoreFault::WriteFails)
            .is_some()
        {
            return Err(io::Error::other("scripted fault: the store write failed"));
        }
        let mut files = lock(&self.files);
        if backup == Backup::KeepLastGood
            && let Some(current) = files.get(name).cloned()
        {
            files.insert(format!("{name}.bak"), current);
        }
        files.insert(name.to_owned(), bytes.to_vec());
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn req_set_04_store_call_log_records_calls() {
        let store = MemoryStore::new();
        assert_eq!(store.read("settings.json").unwrap(), None);
        store
            .replace("settings.json", b"first", Backup::KeepLastGood)
            .unwrap();
        store
            .replace("settings.json", b"second", Backup::KeepLastGood)
            .unwrap();
        store.replace("journal.json", b"{}", Backup::Skip).unwrap();
        assert_eq!(store.file("settings.json.bak"), Some(b"first".to_vec()));
        assert_eq!(store.file("journal.json.bak"), None);
        assert_eq!(
            store.read("settings.json").unwrap(),
            Some(b"second".to_vec())
        );
        assert_eq!(store.calls().len(), 5);
        assert_eq!(
            store.calls()[3],
            StoreCall::Replace {
                name: String::from("journal.json"),
                backup: Backup::Skip,
            }
        );
    }

    #[test]
    fn req_safe_01_inj_d07_failed_write_keeps_the_old_file() {
        let store = MemoryStore::new();
        store.replace("journal.json", b"old", Backup::Skip).unwrap();
        store.script(StoreFault::WriteFails, When::Once);
        assert!(store.replace("journal.json", b"new", Backup::Skip).is_err());
        assert_eq!(store.file("journal.json"), Some(b"old".to_vec()));
        store.script(StoreFault::ReadFails, When::Once);
        assert!(store.read("journal.json").is_err());
    }
}
