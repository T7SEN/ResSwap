//! The in-memory store every fake shares: settings.json, journal.json and their copies as
//! bytes in memory, with a call log and scripted faults. It opens no file (REQ-DISP-01).

#![forbid(unsafe_code)]

use std::collections::BTreeMap;
use std::io;
use std::sync::Mutex;

use crate::apply::journal::JOURNAL_FILE;
use crate::display::fake::{CallLog, FaultScript, When, lock};
use crate::settings::SETTINGS_FILE;
use crate::settings::atomic::{Backup, Store, WriteStage};

/// One of the two files a fault can target.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StoreFile {
    /// settings.json.
    Settings,
    /// journal.json.
    Journal,
}

impl StoreFile {
    /// The file name this target stands for.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Settings => SETTINGS_FILE,
            Self::Journal => JOURNAL_FILE,
        }
    }
}

/// A failure a test scripts into the store. Every failed replace or rename leaves the files
/// as they were.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StoreFault {
    /// The read of any file fails.
    ReadFails,
    /// The replace of any file fails and the old file stays (INJ-D07, INJ-D16).
    WriteFails,
    /// The read of one file fails.
    Read(StoreFile),
    /// The replace of one file fails at one stage: the temp write, the flush, the last good
    /// copy or the rename (INJ-D07 for the journal, INJ-D16 for settings).
    Write(StoreFile, WriteStage),
    /// Moving one file aside fails (INJ-D19 with the rename refused).
    MoveAside(StoreFile),
}

/// One call the store received, in order, with the file it touched.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StoreCall {
    Read(String),
    Replace { name: String, backup: Backup },
    Rename { from: String, to: String },
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

    /// Places `bytes` at `name` without a call in the log, as a fixture or an earlier run
    /// left them (INJ-D18, INJ-D19).
    pub fn put(&self, name: &str, bytes: &[u8]) {
        lock(&self.files).insert(name.to_owned(), bytes.to_vec());
    }

    /// Every file name stored now, in order.
    pub fn names(&self) -> Vec<String> {
        lock(&self.files).keys().cloned().collect()
    }
}

impl Store for MemoryStore {
    fn read(&self, name: &str) -> io::Result<Option<Vec<u8>>> {
        self.log.record(StoreCall::Read(name.to_owned()));
        let fails = self.script.due(|fault| match fault {
            StoreFault::ReadFails => true,
            StoreFault::Read(file) => file.name() == name,
            _ => false,
        });
        if fails.is_some() {
            return Err(io::Error::other("scripted fault: the store read failed"));
        }
        Ok(lock(&self.files).get(name).cloned())
    }

    fn replace(&self, name: &str, bytes: &[u8], backup: Backup) -> io::Result<()> {
        self.log.record(StoreCall::Replace {
            name: name.to_owned(),
            backup,
        });
        let fault = self.script.due(|fault| match fault {
            StoreFault::WriteFails => true,
            StoreFault::Write(file, _) => file.name() == name,
            _ => false,
        });
        match fault {
            Some(StoreFault::Write(_, stage)) => {
                return Err(io::Error::other(format!(
                    "{}: scripted fault: the store write failed",
                    stage.label()
                )));
            }
            Some(_) => {
                return Err(io::Error::other("scripted fault: the store write failed"));
            }
            None => {}
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

    fn rename(&self, from: &str, to: &str) -> io::Result<()> {
        self.log.record(StoreCall::Rename {
            from: from.to_owned(),
            to: to.to_owned(),
        });
        let fails = self.script.due(|fault| match fault {
            StoreFault::MoveAside(file) => file.name() == from,
            _ => false,
        });
        if fails.is_some() {
            return Err(io::Error::other("scripted fault: the store rename failed"));
        }
        let mut files = lock(&self.files);
        let bytes = files.remove(from).ok_or_else(|| {
            io::Error::new(io::ErrorKind::NotFound, format!("{from} does not exist"))
        })?;
        files.insert(to.to_owned(), bytes);
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

    /// A fault aimed at one file leaves the other file's writes alone, so a test can fail the
    /// settings write while the journal keeps working (INJ-D16).
    #[test]
    fn req_safe_01_inj_d07_faults_target_one_file_and_stage() {
        let store = MemoryStore::new();
        store.put("settings.json", b"old settings");
        store.script(
            StoreFault::Write(StoreFile::Settings, WriteStage::Sync),
            When::Always,
        );
        store
            .replace("journal.json", b"journal", Backup::Skip)
            .unwrap();
        let error = store
            .replace("settings.json", b"new", Backup::KeepLastGood)
            .unwrap_err();
        assert!(error.to_string().starts_with("flush to disk"), "{error}");
        assert_eq!(store.file("settings.json"), Some(b"old settings".to_vec()));
        assert_eq!(store.file("settings.json.bak"), None);

        store.script(StoreFault::Read(StoreFile::Journal), When::Once);
        assert!(store.read("settings.json").is_ok());
        assert!(store.read("journal.json").is_err());
        assert!(store.read("journal.json").is_ok());
    }

    #[test]
    fn req_safe_06_inj_d19_rename_moves_or_fails() {
        let store = MemoryStore::new();
        store.put("journal.json", b"bad");
        store.script(StoreFault::MoveAside(StoreFile::Journal), When::Once);
        assert!(
            store
                .rename("journal.json", "journal.damaged-1.json")
                .is_err()
        );
        assert_eq!(store.names(), vec![String::from("journal.json")]);
        store
            .rename("journal.json", "journal.damaged-1.json")
            .unwrap();
        assert_eq!(store.names(), vec![String::from("journal.damaged-1.json")]);
        assert_eq!(store.file("journal.damaged-1.json"), Some(b"bad".to_vec()));
        let missing = store
            .rename("journal.json", "journal.damaged-2.json")
            .unwrap_err();
        assert_eq!(missing.kind(), io::ErrorKind::NotFound);
        assert_eq!(
            store.calls().last(),
            Some(&StoreCall::Rename {
                from: String::from("journal.json"),
                to: String::from("journal.damaged-2.json"),
            })
        );
    }
}
