//! The store behind settings.json and journal.json: each file replaced whole through a temp
//! file in the same folder, flushed to disk before the rename (REQ-SET-04, REQ-SAFE-01). The
//! trait lets every fake share one in-memory store; `DiskStore` is the real one.

#![forbid(unsafe_code)]

use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

/// The most bytes ResSwap accepts from one of its own files. Both files stay a few kilobytes,
/// so a larger file is treated as damaged and a huge file cannot exhaust memory.
pub const MAX_FILE_BYTES: u64 = 1024 * 1024;

/// Whether a replace keeps the previous file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Backup {
    /// Copy the current file to `<name>.bak` first (settings.json).
    KeepLastGood,
    /// No copy (journal.json).
    Skip,
}

/// The stages of a replace, in order. A failure at any of them leaves the old file as it
/// was, because only the last one touches it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WriteStage {
    /// Writing `<name>.tmp`.
    TempWrite,
    /// `File::sync_all` on the temp file.
    Sync,
    /// Copying the current file to `<name>.bak`.
    BackupCopy,
    /// Renaming the temp file over the target.
    Rename,
}

impl WriteStage {
    /// The stage as the error detail names it.
    pub const fn label(self) -> &'static str {
        match self {
            Self::TempWrite => "temp file write",
            Self::Sync => "flush to disk",
            Self::BackupCopy => "last good copy",
            Self::Rename => "rename over the target",
        }
    }
}

/// Files ResSwap keeps in its data folder, by fixed name. Memory follows a write only after
/// it succeeds, so memory and disk never disagree.
pub trait Store: Send + Sync {
    /// The bytes of `name`, or `None` when the file does not exist. Reads at most
    /// `MAX_FILE_BYTES + 1` bytes, so a caller sees that a file is too large without loading
    /// all of it. Changes nothing.
    fn read(&self, name: &str) -> io::Result<Option<Vec<u8>>>;

    /// Replaces `name` with `bytes`. A failure before the rename leaves the old file as it
    /// was.
    fn replace(&self, name: &str, bytes: &[u8], backup: Backup) -> io::Result<()>;

    /// Renames `from` to `to` in the data folder, replacing `to`. Moves a damaged file, or one
    /// a newer build wrote, aside before a new one is written, so it is kept for the log.
    fn rename(&self, from: &str, to: &str) -> io::Result<()>;
}

/// The data folder on disk. The caller passes the folder: lib.rs picks the bundle
/// identifier folder in %LOCALAPPDATA%, or RESSWAP_DATA_DIR in a debug build.
#[derive(Clone, Debug)]
pub struct DiskStore {
    folder: PathBuf,
}

impl DiskStore {
    /// A store over `folder`. Nothing is read or created until the first call.
    pub fn new(folder: impl Into<PathBuf>) -> Self {
        Self {
            folder: folder.into(),
        }
    }

    /// The folder this store reads and writes.
    pub fn folder(&self) -> &Path {
        &self.folder
    }

    fn path(&self, name: &str) -> io::Result<PathBuf> {
        Ok(self.folder.join(plain_name(name)?))
    }
}

impl Store for DiskStore {
    fn read(&self, name: &str) -> io::Result<Option<Vec<u8>>> {
        let file = match File::open(self.path(name)?) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error),
        };
        let mut bytes = Vec::new();
        file.take(MAX_FILE_BYTES.saturating_add(1))
            .read_to_end(&mut bytes)?;
        Ok(Some(bytes))
    }

    fn replace(&self, name: &str, bytes: &[u8], backup: Backup) -> io::Result<()> {
        let name = plain_name(name)?;
        // A first run has no data folder yet.
        fs::create_dir_all(&self.folder).map_err(at("data folder"))?;
        replace_file(&self.folder, name, bytes, backup)
    }

    fn rename(&self, from: &str, to: &str) -> io::Result<()> {
        fs::rename(self.path(from)?, self.path(to)?)
    }
}

/// Replaces `dir\name` with `bytes`: `name.tmp` in the same folder, write, `sync_all`, the
/// optional copy of the current file to `name.bak`, then the rename over the target
/// (REQ-SET-04). The rename replaces the target; on Windows std uses MoveFileExW for it.
pub fn replace_file(dir: &Path, name: &str, bytes: &[u8], backup: Backup) -> io::Result<()> {
    let name = plain_name(name)?;
    let target = dir.join(name);
    let temp = dir.join(format!("{name}.tmp"));
    {
        // Truncates a temp file a killed save left behind.
        let mut file = File::create(&temp).map_err(at(WriteStage::TempWrite.label()))?;
        file.write_all(bytes)
            .map_err(at(WriteStage::TempWrite.label()))?;
        // On disk before the rename can expose it.
        file.sync_all().map_err(at(WriteStage::Sync.label()))?;
    }
    if backup == Backup::KeepLastGood {
        match fs::copy(&target, dir.join(format!("{name}.bak"))) {
            Ok(_) => {}
            // The first save has nothing to keep.
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(at(WriteStage::BackupCopy.label())(error)),
        }
    }
    fs::rename(&temp, &target).map_err(at(WriteStage::Rename.label()))
}

/// `name` when it is a plain file name in the data folder. The names are fixed or built
/// from digits by ResSwap, so anything else is a bug and is refused before a path exists.
fn plain_name(name: &str) -> io::Result<&str> {
    let plain = !name.is_empty()
        && name != "."
        && name != ".."
        && !name
            .chars()
            .any(|c| matches!(c, '\\' | '/' | ':') || c.is_control());
    if plain {
        Ok(name)
    } else {
        Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("not a plain file name: {name:?}"),
        ))
    }
}

/// Adds the stage to an I/O error, so the detail says where the write stopped.
fn at(stage: &'static str) -> impl FnOnce(io::Error) -> io::Error {
    move |error| io::Error::new(error.kind(), format!("{stage}: {error}"))
}

#[cfg(test)]
pub(crate) mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::*;

    static NEXT: AtomicU64 = AtomicU64::new(0);

    /// A folder of its own under the system temp folder, removed when the test ends.
    pub(crate) struct TempFolder {
        path: PathBuf,
    }

    impl TempFolder {
        pub(crate) fn new(test: &str) -> Self {
            let unique = NEXT.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "resswap-test-{test}-{}-{unique}",
                std::process::id()
            ));
            // A folder left by an earlier run with the same process ID is stale.
            let _ = fs::remove_dir_all(&path);
            Self { path }
        }

        pub(crate) fn path(&self) -> &Path {
            &self.path
        }

        pub(crate) fn file(&self, name: &str) -> Option<Vec<u8>> {
            fs::read(self.path.join(name)).ok()
        }
    }

    impl Drop for TempFolder {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    #[test]
    fn req_set_04_replace_keeps_the_last_good_copy() {
        let folder = TempFolder::new("replace");
        let store = DiskStore::new(folder.path());
        assert_eq!(store.read("settings.json").unwrap(), None);
        // The first write creates the folder and has nothing to copy.
        store
            .replace("settings.json", b"first", Backup::KeepLastGood)
            .unwrap();
        assert_eq!(folder.file("settings.json.bak"), None);
        store
            .replace("settings.json", b"second", Backup::KeepLastGood)
            .unwrap();
        assert_eq!(folder.file("settings.json"), Some(b"second".to_vec()));
        assert_eq!(folder.file("settings.json.bak"), Some(b"first".to_vec()));
        assert_eq!(folder.file("settings.json.tmp"), None);
        store.replace("journal.json", b"{}", Backup::Skip).unwrap();
        store.replace("journal.json", b"[]", Backup::Skip).unwrap();
        assert_eq!(folder.file("journal.json.bak"), None);
        assert_eq!(store.read("journal.json").unwrap(), Some(b"[]".to_vec()));
    }

    #[test]
    fn req_set_06_failed_temp_write_leaves_the_old_file() {
        let folder = TempFolder::new("temp-write");
        let store = DiskStore::new(folder.path());
        store
            .replace("settings.json", b"old", Backup::KeepLastGood)
            .unwrap();
        // A folder where the temp file goes makes the temp write fail.
        fs::create_dir(folder.path().join("settings.json.tmp")).unwrap();
        let error = store
            .replace("settings.json", b"new", Backup::KeepLastGood)
            .unwrap_err();
        assert!(error.to_string().starts_with("temp file write"), "{error}");
        assert_eq!(folder.file("settings.json"), Some(b"old".to_vec()));
        assert_eq!(folder.file("settings.json.bak"), None);
    }

    #[test]
    fn req_set_06_failed_rename_leaves_the_target() {
        let folder = TempFolder::new("rename");
        // A folder at the target's name cannot be replaced by a file.
        fs::create_dir_all(folder.path().join("journal.json")).unwrap();
        let store = DiskStore::new(folder.path());
        let error = store
            .replace("journal.json", b"new", Backup::Skip)
            .unwrap_err();
        assert!(
            error.to_string().starts_with("rename over the target"),
            "{error}"
        );
        assert!(folder.path().join("journal.json").is_dir());
    }

    #[test]
    fn req_set_05_reads_stop_after_the_cap() {
        let folder = TempFolder::new("cap");
        let store = DiskStore::new(folder.path());
        let size = usize::try_from(MAX_FILE_BYTES).unwrap() + 100;
        store
            .replace("settings.json", &vec![b' '; size], Backup::Skip)
            .unwrap();
        let read = store.read("settings.json").unwrap().unwrap();
        assert_eq!(read.len(), usize::try_from(MAX_FILE_BYTES).unwrap() + 1);
    }

    #[test]
    fn req_safe_06_rename_moves_a_file_aside() {
        let folder = TempFolder::new("aside");
        let store = DiskStore::new(folder.path());
        store.replace("journal.json", b"bad", Backup::Skip).unwrap();
        store
            .rename("journal.json", "journal.damaged-1.json")
            .unwrap();
        assert_eq!(store.read("journal.json").unwrap(), None);
        assert_eq!(folder.file("journal.damaged-1.json"), Some(b"bad".to_vec()));
        assert!(
            store
                .rename("journal.json", "journal.damaged-2.json")
                .is_err()
        );
    }

    #[test]
    fn req_set_01_names_outside_the_data_folder_are_refused() {
        let folder = TempFolder::new("names");
        let store = DiskStore::new(folder.path());
        for name in ["", ".", "..", r"..\settings.json", "a/b", "C:x", "a\nb"] {
            let read = store.read(name).unwrap_err();
            assert_eq!(read.kind(), io::ErrorKind::InvalidInput, "{name:?}");
            assert!(store.replace(name, b"x", Backup::Skip).is_err(), "{name:?}");
        }
        assert!(!folder.path().exists());
    }
}
