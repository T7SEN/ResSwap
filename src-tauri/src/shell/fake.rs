//! The fake Run value, the fake shortcut writer, the fake launcher, the fake folder opener and
//! the fake keep-mode window: state in memory, a call log and scripted faults. They write no
//! registry value, create no file, start no program and open no window (REQ-DISP-01).

#![forbid(unsafe_code)]

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use crate::apply::{OpenEntries, Ticket};
use crate::display::fake::{CallLog, FaultScript, When, lock};
use crate::error::{AppError, Failure};
use crate::shell::autostart::RunKey;
use crate::shell::log_folder::FolderOpener;
use crate::shell::shortcuts::{Launcher, ShortcutSpec, ShortcutWriter};
use crate::shell::window::KeepModeWindow;

/// The Desktop folder the fake reports, in its environment-variable form. Synthetic.
const FAKE_DESKTOP: &str = r"%USERPROFILE%\Desktop";

/// A failure a test scripts into the fake Run value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RunKeyFault {
    /// The read fails: AUTOSTART_FAILED.
    ReadFails,
    /// The write fails and nothing changes: AUTOSTART_FAILED.
    WriteFails,
    /// The write reports success but stores other data, so the read-back differs.
    WritesOtherData,
    /// The delete fails and the value stays: AUTOSTART_FAILED.
    DeleteFails,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RunKeyOp {
    Read,
    Write,
    Delete,
}

impl RunKeyFault {
    fn op(self) -> RunKeyOp {
        match self {
            Self::ReadFails => RunKeyOp::Read,
            Self::WriteFails | Self::WritesOtherData => RunKeyOp::Write,
            Self::DeleteFails => RunKeyOp::Delete,
        }
    }
}

/// One call the fake Run value received, in order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RunKeyCall {
    Read,
    Write { change_id: String, data: String },
    Delete { change_id: String },
}

/// ResSwap's Run value in memory. Absent at start, as on the test PC.
pub struct FakeRunKey {
    journal: OpenEntries,
    value: Mutex<Option<String>>,
    log: CallLog<RunKeyCall>,
    script: FaultScript<RunKeyFault>,
}

impl FakeRunKey {
    /// The fake, checking every Ticket against `journal`.
    pub fn new(journal: OpenEntries) -> Self {
        Self {
            journal,
            value: Mutex::new(None),
            log: CallLog::default(),
            script: FaultScript::default(),
        }
    }

    /// Adds a fault to the script.
    pub fn script(&self, fault: RunKeyFault, when: When) {
        self.script.push(fault, when);
    }

    /// Every call so far, in order.
    pub fn calls(&self) -> Vec<RunKeyCall> {
        self.log.calls()
    }

    /// Whether any call so far changed the fake PC.
    pub fn changed_anything(&self) -> bool {
        self.log.any(|call| !matches!(call, RunKeyCall::Read))
    }

    fn due(&self, op: RunKeyOp) -> Option<RunKeyFault> {
        self.script.due(|fault| fault.op() == op)
    }
}

fn autostart_failed(detail: &str) -> AppError {
    AppError::AutostartFailed(Box::new(Failure::new(format!("scripted fault: {detail}"))))
}

impl RunKey for FakeRunKey {
    fn read(&self) -> Result<Option<String>, AppError> {
        self.log.record(RunKeyCall::Read);
        if self.due(RunKeyOp::Read).is_some() {
            return Err(autostart_failed("the fake Run value could not be read"));
        }
        Ok(lock(&self.value).clone())
    }

    fn write(&self, ticket: &Ticket, data: &str) -> Result<(), AppError> {
        // A change with no open journal entry is a pipeline bug (pc_changes_need_ticket).
        self.journal.require_open(ticket);
        self.log.record(RunKeyCall::Write {
            change_id: ticket.change_id().to_owned(),
            data: data.to_owned(),
        });
        match self.due(RunKeyOp::Write) {
            Some(RunKeyFault::WritesOtherData) => {
                *lock(&self.value) = Some(String::from("scripted other data"));
                Ok(())
            }
            Some(_) => Err(autostart_failed("the fake Run value could not be written")),
            None => {
                *lock(&self.value) = Some(data.to_owned());
                Ok(())
            }
        }
    }

    fn delete(&self, ticket: &Ticket) -> Result<(), AppError> {
        self.journal.require_open(ticket);
        self.log.record(RunKeyCall::Delete {
            change_id: ticket.change_id().to_owned(),
        });
        if self.due(RunKeyOp::Delete).is_some() {
            return Err(autostart_failed("the fake Run value could not be deleted"));
        }
        *lock(&self.value) = None;
        Ok(())
    }
}

/// A failure a test scripts into the fake shortcut writer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShortcutFault {
    /// The write fails and no file is left: SHORTCUT_FAILED.
    WriteFails,
}

/// One shortcut the fake wrote, in order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShortcutCall {
    pub change_id: String,
    pub shortcut: ShortcutSpec,
}

/// A Desktop folder in memory.
pub struct FakeShortcuts {
    journal: OpenEntries,
    written: Mutex<Vec<PathBuf>>,
    log: CallLog<ShortcutCall>,
    script: FaultScript<ShortcutFault>,
}

impl FakeShortcuts {
    /// The fake, checking every Ticket against `journal`.
    pub fn new(journal: OpenEntries) -> Self {
        Self {
            journal,
            written: Mutex::new(Vec::new()),
            log: CallLog::default(),
            script: FaultScript::default(),
        }
    }

    /// Adds a fault to the script.
    pub fn script(&self, fault: ShortcutFault, when: When) {
        self.script.push(fault, when);
    }

    /// Every call so far, in order.
    pub fn calls(&self) -> Vec<ShortcutCall> {
        self.log.calls()
    }

    /// The shortcuts on the fake Desktop.
    pub fn written(&self) -> Vec<PathBuf> {
        lock(&self.written).clone()
    }
}

impl ShortcutWriter for FakeShortcuts {
    fn create(&self, ticket: &Ticket, shortcut: &ShortcutSpec) -> Result<PathBuf, AppError> {
        // A change with no open journal entry is a pipeline bug (pc_changes_need_ticket).
        self.journal.require_open(ticket);
        self.log.record(ShortcutCall {
            change_id: ticket.change_id().to_owned(),
            shortcut: shortcut.clone(),
        });
        if let Some(ShortcutFault::WriteFails) = self.script.due(|_| true) {
            return Err(AppError::ShortcutFailed(Box::new(Failure::new(
                "scripted fault: the fake shortcut could not be written",
            ))));
        }
        let path = PathBuf::from(FAKE_DESKTOP).join(format!("{}.lnk", shortcut.file_name));
        lock(&self.written).push(path.clone());
        Ok(path)
    }
}

/// A failure a test scripts into the fake launcher.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LaunchFault {
    /// The start fails: INTERNAL, because no other code describes a failed start.
    StartFails,
}

/// A launcher that records each program it was asked to start and starts none.
#[derive(Default)]
pub struct FakeLauncher {
    log: CallLog<PathBuf>,
    script: FaultScript<LaunchFault>,
}

impl FakeLauncher {
    /// The fake, with nothing started.
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a fault to the script.
    pub fn script(&self, fault: LaunchFault, when: When) {
        self.script.push(fault, when);
    }

    /// Every program the fake was asked to start, in order.
    pub fn calls(&self) -> Vec<PathBuf> {
        self.log.calls()
    }
}

impl Launcher for FakeLauncher {
    fn start(&self, path: &Path) -> Result<(), AppError> {
        self.log.record(path.to_path_buf());
        if let Some(LaunchFault::StartFails) = self.script.due(|_| true) {
            return Err(AppError::Internal {
                detail: String::from(
                    "scripted fault: the fake launcher could not start the program",
                ),
            });
        }
        Ok(())
    }
}

/// A failure a test scripts into the fake folder opener.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FolderFault {
    /// Explorer does not start: INTERNAL.
    OpenFails,
}

/// A folder opener that records each folder it was asked to show and opens none.
#[derive(Default)]
pub struct FakeFolderOpener {
    log: CallLog<PathBuf>,
    script: FaultScript<FolderFault>,
}

impl FakeFolderOpener {
    /// The fake, with nothing opened.
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a fault to the script.
    pub fn script(&self, fault: FolderFault, when: When) {
        self.script.push(fault, when);
    }

    /// Every folder the fake was asked to show, in order.
    pub fn calls(&self) -> Vec<PathBuf> {
        self.log.calls()
    }
}

impl FolderOpener for FakeFolderOpener {
    fn open(&self, folder: &Path) -> Result<(), AppError> {
        self.log.record(folder.to_path_buf());
        if let Some(FolderFault::OpenFails) = self.script.due(|_| true) {
            return Err(AppError::internal(
                "scripted fault: the fake folder opener could not start Explorer",
            ));
        }
        Ok(())
    }
}

/// A failure a test scripts into the fake keep-mode window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeepModeFault {
    /// The window does not open (INJ-D15): the countdown must still revert.
    OpenFails,
}

/// One call the fake keep-mode window received.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeepModeCall {
    Open,
    Close,
}

/// A keep-mode window that records each open and close and shows nothing.
#[derive(Default)]
pub struct FakeKeepModeWindow {
    log: CallLog<KeepModeCall>,
    script: FaultScript<KeepModeFault>,
    open: Mutex<bool>,
}

impl FakeKeepModeWindow {
    /// The fake, with no window open.
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a fault to the script.
    pub fn script(&self, fault: KeepModeFault, when: When) {
        self.script.push(fault, when);
    }

    /// Every call so far, in order.
    pub fn calls(&self) -> Vec<KeepModeCall> {
        self.log.calls()
    }

    /// Whether the fake window is open now.
    pub fn is_open(&self) -> bool {
        *lock(&self.open)
    }
}

impl KeepModeWindow for FakeKeepModeWindow {
    fn open(&self) -> Result<(), AppError> {
        self.log.record(KeepModeCall::Open);
        if let Some(KeepModeFault::OpenFails) = self.script.due(|_| true) {
            return Err(AppError::internal(
                "scripted fault: the fake keep-mode window did not open",
            ));
        }
        *lock(&self.open) = true;
        Ok(())
    }

    fn close(&self) {
        self.log.record(KeepModeCall::Close);
        *lock(&self.open) = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::{ErrorCode, ErrorContext};

    #[test]
    fn req_set_09_folder_opener_records_and_opens_nothing() {
        let opener = FakeFolderOpener::new();
        let folder = PathBuf::from(r"%LOCALAPPDATA%\io.github.T7SEN.ResSwap\logs");
        opener.open(&folder).unwrap();
        opener.script(FolderFault::OpenFails, When::Once);
        assert_eq!(
            opener.open(&folder).unwrap_err().code(),
            ErrorCode::Internal
        );
        assert_eq!(opener.calls(), vec![folder.clone(), folder]);
    }

    #[test]
    fn req_safe_05_inj_d15_keep_mode_window_can_fail_to_open() {
        let window = FakeKeepModeWindow::new();
        window.open().unwrap();
        assert!(window.is_open());
        window.close();
        window.script(KeepModeFault::OpenFails, When::Once);
        assert!(window.open().is_err());
        assert!(!window.is_open());
        assert_eq!(
            window.calls(),
            vec![KeepModeCall::Open, KeepModeCall::Close, KeepModeCall::Open]
        );
    }

    fn spec() -> ShortcutSpec {
        ShortcutSpec {
            file_name: String::from("Example"),
            target: PathBuf::from(r"%LOCALAPPDATA%\ResSwap\ResSwap.exe"),
            arguments: String::from("--play 0f8fad5b-d9cb-469f-a165-70867728950e"),
            working_folder: PathBuf::from(r"%LOCALAPPDATA%\ResSwap"),
            description: String::from("Example"),
        }
    }

    #[test]
    fn req_shell_07_run_key_call_log_records_calls() {
        let journal = OpenEntries::default();
        let run_key = FakeRunKey::new(journal.clone());
        assert_eq!(run_key.read().unwrap(), None);
        let ticket = journal.open_for_tests("autostart-1");
        let data = r#""%LOCALAPPDATA%\ResSwap\ResSwap.exe" --autostart"#;
        run_key.write(&ticket, data).unwrap();
        assert_eq!(run_key.read().unwrap().as_deref(), Some(data));
        run_key.delete(&ticket).unwrap();
        assert_eq!(run_key.read().unwrap(), None);
        assert_eq!(
            run_key.calls(),
            vec![
                RunKeyCall::Read,
                RunKeyCall::Write {
                    change_id: String::from("autostart-1"),
                    data: data.to_owned(),
                },
                RunKeyCall::Read,
                RunKeyCall::Delete {
                    change_id: String::from("autostart-1"),
                },
                RunKeyCall::Read,
            ]
        );
        assert!(run_key.changed_anything());
    }

    #[test]
    fn req_shell_08_scripted_run_key_faults() {
        let journal = OpenEntries::default();
        let run_key = FakeRunKey::new(journal.clone());
        let ticket = journal.open_for_tests("autostart-1");
        run_key.script(RunKeyFault::WriteFails, When::Once);
        let failed = run_key.write(&ticket, "data").unwrap_err();
        assert_eq!(failed.code(), ErrorCode::AutostartFailed);
        assert_eq!(run_key.read().unwrap(), None);
        run_key.script(RunKeyFault::WritesOtherData, When::Once);
        run_key.write(&ticket, "data").unwrap();
        assert_ne!(run_key.read().unwrap().as_deref(), Some("data"));
        run_key.script(RunKeyFault::DeleteFails, When::Once);
        assert!(run_key.delete(&ticket).is_err());
        assert!(run_key.read().unwrap().is_some());
        run_key.script(RunKeyFault::ReadFails, When::Once);
        assert!(run_key.read().is_err());
    }

    #[test]
    #[should_panic(expected = "has no open journal entry")]
    fn req_disp_01_run_key_change_needs_ticket() {
        let journal = OpenEntries::default();
        let run_key = FakeRunKey::new(journal.clone());
        let ticket = journal.open_for_tests("autostart-1");
        journal.close_for_tests(&ticket);
        let _ = run_key.write(&ticket, "data");
    }

    #[test]
    fn req_prof_06_shortcut_call_log_records_calls() {
        let journal = OpenEntries::default();
        let shortcuts = FakeShortcuts::new(journal.clone());
        let ticket = journal.open_for_tests("shortcut-1");
        let path = shortcuts.create(&ticket, &spec()).unwrap();
        assert_eq!(path, PathBuf::from(r"%USERPROFILE%\Desktop\Example.lnk"));
        assert_eq!(shortcuts.written(), vec![path]);
        assert_eq!(
            shortcuts.calls(),
            vec![ShortcutCall {
                change_id: String::from("shortcut-1"),
                shortcut: spec(),
            }]
        );
        shortcuts.script(ShortcutFault::WriteFails, When::Once);
        let failed = shortcuts.create(&ticket, &spec()).unwrap_err();
        assert_eq!(failed.code(), ErrorCode::ShortcutFailed);
        assert_eq!(failed.to_ipc().context, ErrorContext::default());
        assert_eq!(shortcuts.written().len(), 1);
    }

    #[test]
    #[should_panic(expected = "has no open journal entry")]
    fn req_disp_01_shortcut_change_needs_ticket() {
        let journal = OpenEntries::default();
        let shortcuts = FakeShortcuts::new(journal.clone());
        let ticket = journal.open_for_tests("shortcut-1");
        journal.close_for_tests(&ticket);
        let _ = shortcuts.create(&ticket, &spec());
    }

    #[test]
    fn req_prof_07_launcher_call_log_records_calls() {
        let launcher = FakeLauncher::new();
        let game = PathBuf::from(r"%ProgramFiles%\Example\game.exe");
        launcher.start(&game).unwrap();
        launcher.script(LaunchFault::StartFails, When::Once);
        let failed = launcher.start(&game).unwrap_err();
        assert_eq!(failed.code(), ErrorCode::Internal);
        assert_eq!(launcher.calls(), vec![game.clone(), game]);
    }
}
