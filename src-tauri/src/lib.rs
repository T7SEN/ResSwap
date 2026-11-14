//! ResSwap's core: argument handling, the backends, the managed `Core` state and the Tauri
//! builder. main() parses arguments with this crate before the builder runs (REQ-SHELL-11).

pub mod apply;
pub mod display;
pub mod elevation;
pub mod error;
pub mod ipc;
pub mod profiles;
pub mod settings;
pub mod shell;
pub mod system;
pub mod vendor;
pub mod watcher;

use std::ffi::OsString;
use std::fmt::{Display, Write as _};
use std::io;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError, RwLock};
use std::time::Instant;

use tauri::{AppHandle, Manager, RunEvent, Runtime};

use crate::apply::countdown::Countdown;
use crate::apply::journal::Journal;
use crate::apply::{OpenEntries, check_monitor_id};
use crate::display::DisplayBackend;
use crate::display::fake::{FakeDisplay, FixtureProfile};
use crate::display::monitors::to_monitor;
use crate::display::types::{Monitor, MonitorId};
use crate::display::win32::Win32Display;
use crate::elevation::ElevatedToolRunner;
use crate::elevation::fake::FakeRunner;
use crate::error::{AppError, Failure};
use crate::ipc::commands::{CommandFaults, CommandName};
use crate::ipc::events::{
    EventSink, NoListeners, TauriSink, emit_error_raised, emit_state_changed,
};
use crate::ipc::types::{
    ActiveChange, AddCustomModeRequest, AppState, ApplyOutcome, AutoSwitchState, BackendKind,
    BuildInfo, BusyState, ChangeSource, CustomChangeOutcome, Diagnostics, ErrorRef, ErrorSource,
    GpuInfo, MonitorDiagnostics, PendingChange, ProfileRef, RaisedError, RecoveryState,
    RecoveryStatus, RemoveCustomEntryRequest, SetAutoSwitchRequest, SetAutostartRequest,
    SetHotkeyRequest, ShortcutCreated, UndoAllRequest, UpdateInfo,
};
use crate::profiles::{Profile, ProfileId, is_lowercase_uuid};
use crate::settings::atomic::{Backup, DiskStore, Store};
use crate::settings::fake::MemoryStore;
use crate::settings::{Settings, SettingsInput, SettingsStore};
use crate::shell::autostart::RunKey;
use crate::shell::fake::{FakeFolderOpener, FakeLauncher, FakeRunKey, FakeShortcuts};
use crate::shell::log_folder::{FolderOpener, ShellFolderOpener};
use crate::shell::shortcuts::{Launcher, ShortcutWriter};
use crate::shell::window::{KeepModeWindow, NoKeepModeWindow, TauriKeepModeWindow};
use crate::system::clock::{Clock, SystemClock, epoch_ms_now};
use crate::system::fake::{FakeClock, FakeProcessList};
use crate::system::notify::{NotifyWindow, SystemEvent};
use crate::system::processes::ProcessList;
use crate::system::registry::Win32Registry;
use crate::vendor::fake::FakeVendor;
use crate::vendor::nvidia::fake::FakeRegistry;
use crate::vendor::nvidia::nv_modes::RegistryReader;
use crate::vendor::{CustomEntryList, VendorExtras};

/// Exit code for an argument list ResSwap refuses. main() exits with it before the Tauri
/// builder runs, so nothing reaches a running instance (REQ-SHELL-11).
pub const BAD_ARGUMENTS: i32 = 2;

/// The log file's name, without its extension.
const LOG_FILE_NAME: &str = "ResSwap";

/// The size at which the log file is renamed and a new one started (D-027).
const LOG_MAX_FILE_BYTES: u128 = 1_000_000;

/// How many renamed log files are kept beside the current one (D-027).
const LOG_KEPT_OLD_FILES: usize = 5;

/// The data folder as get_diagnostics shows it: with the environment variable, never the
/// expanded user folder, so a pasted report carries no Windows user name. The folder is the
/// bundle identifier's (D-011); a test compares it with tauri.conf.json.
const DATA_FOLDER_TEXT: &str = r"%LOCALAPPDATA%\io.github.T7SEN.ResSwap";

/// The data folder of a debug run started with RESSWAP_DATA_DIR, as get_diagnostics shows it.
const DATA_DIR_TEXT: &str = "%RESSWAP_DATA_DIR%";

/// When main() started: the base of every timing line's mono_ms, and the epoch time of the
/// app.started line that first paint is measured from (REQ-PERF-05).
struct ProcessStart {
    at: Instant,
    epoch_ms: u64,
}

static PROCESS_START: OnceLock<ProcessStart> = OnceLock::new();

/// Takes the process start time. main() calls it on its first line, so app.started marks the
/// start of the process, not the later moment the log can first be written.
pub fn mark_process_start() {
    let _ = PROCESS_START.set(ProcessStart {
        at: Instant::now(),
        epoch_ms: epoch_ms_now(),
    });
}

/// Milliseconds since main() started; `None` in a process that never marked its start, such
/// as a test.
pub fn mono_ms() -> Option<u64> {
    PROCESS_START
        .get()
        .map(|start| u64::try_from(start.at.elapsed().as_millis()).unwrap_or(u64::MAX))
}

/// One timing line: `PERF <event> key=value ...`. Values hold no spaces, so a profile is
/// named by its ID.
pub fn perf_line(event: &str, fields: &[(&str, &dyn Display)]) -> String {
    let mut line = format!("PERF {event}");
    for (key, value) in fields {
        let _ = write!(line, " {key}={value}");
    }
    line
}

/// Writes one timing line with target perf through the log, after the log plugin's own
/// date, time, target and level prefix, which the measurement script parses.
pub fn perf_event(event: &str, fields: &[(&str, &dyn Display)]) {
    log::info!(target: "perf", "{}", perf_line(event, fields));
}

/// The app.started line, written as soon as the log runs. Its epoch_ms is the start of
/// main(), so the first-paint measurement counts the whole start. Writes nothing in a
/// process that never marked its start.
pub fn log_app_started() {
    if let Some(start) = PROCESS_START.get() {
        perf_event(
            "app.started",
            &[("mono_ms", &0), ("epoch_ms", &start.epoch_ms)],
        );
    }
}

/// What a start asks ResSwap to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StartAction {
    /// The Start menu, the desktop icon, a double click.
    Default,
    /// The HKCU Run value.
    Autostart,
    /// A Play shortcut.
    Play { profile_id: ProfileId },
}

/// An argument list ResSwap refuses.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BadArguments;

/// Arguments after the exe path: none, `--autostart`, or `--play <lowercase UUID>`.
/// Anything else is refused. main() and the single-instance callback both use it.
pub fn parse_args<I: IntoIterator<Item = OsString>>(args: I) -> Result<StartAction, BadArguments> {
    let args: Vec<String> = args
        .into_iter()
        .map(|arg| arg.into_string().map_err(|_| BadArguments))
        .collect::<Result<_, _>>()?;
    match args.as_slice() {
        [] => Ok(StartAction::Default),
        [flag] if flag == "--autostart" => Ok(StartAction::Autostart),
        [flag, id] if flag == "--play" => ProfileId::parse(id)
            .map(|profile_id| StartAction::Play { profile_id })
            .ok_or(BadArguments),
        _ => Err(BadArguments),
    }
}

/// The OS-facing implementations the core runs on, chosen once at start by one switch
/// (REQ-DISP-01, REQ-SEC-05). A part whose real implementation a later phase builds is
/// `None` on the real backends until then, so a release build has nothing to fall back on
/// but the honest error of the command that needs it.
pub struct Backends {
    pub kind: BackendKind,
    /// The fixture behind the fakes; `None` on the real backends.
    pub fixture: Option<FixtureProfile>,
    pub display: Arc<dyn DisplayBackend>,
    pub clock: Arc<dyn Clock>,
    /// The record of open journal entries the changing fakes check every Ticket against.
    /// The journal loads into it, so the fakes follow the real journal.
    pub journal: OpenEntries,
    pub vendor: Option<Arc<dyn VendorExtras>>,
    pub registry: Option<Arc<dyn RegistryReader>>,
    pub runner: Option<Arc<dyn ElevatedToolRunner>>,
    pub processes: Option<Arc<dyn ProcessList>>,
    pub run_key: Option<Arc<dyn RunKey>>,
    pub shortcuts: Option<Arc<dyn ShortcutWriter>>,
    pub launcher: Option<Arc<dyn Launcher>>,
    pub folder_opener: Option<Arc<dyn FolderOpener>>,
    /// Where settings.json and journal.json live; `None` when the data folder is unknown,
    /// and then every journal write is refused, so nothing changes without a record.
    pub store: Option<Arc<dyn Store>>,
    /// Whole-command faults; empty on the real backends.
    pub command_faults: CommandFaults,
    /// A RESSWAP_FAKE_PROFILE value that names no fixture, which start-up raises; `None` on
    /// the real backends.
    pub refused_fixture: Option<String>,
}

impl Backends {
    /// The backends for the player's PC. `data_folder` is the folder settings.json and
    /// journal.json live in: Tauri's app local data folder (tauri 2.12.5
    /// PathResolver::app_local_data_dir, which asks SHGetKnownFolderPath for
    /// FOLDERID_LocalAppData and joins the bundle identifier, D-011), or RESSWAP_DATA_DIR in
    /// a debug run. The folder opener shows the log folder with ShellExecuteExW (REQ-SET-09).
    pub fn real(data_folder: Option<PathBuf>) -> Self {
        Self {
            kind: BackendKind::Real,
            fixture: None,
            display: Arc::new(Win32Display),
            clock: Arc::new(SystemClock::new()),
            journal: OpenEntries::default(),
            vendor: None,
            registry: Some(Arc::new(Win32Registry)),
            runner: None,
            processes: None,
            run_key: None,
            shortcuts: None,
            launcher: None,
            folder_opener: Some(Arc::new(ShellFolderOpener)),
            store: data_folder.map(|folder| Arc::new(DiskStore::new(folder)) as Arc<dyn Store>),
            command_faults: CommandFaults::default(),
            refused_fixture: None,
        }
    }
}

/// Every fake, on one shared record of open journal entries and one shared in-memory store.
/// Tests keep this to script faults and read the call logs.
pub struct Fakes {
    pub profile: FixtureProfile,
    pub journal: OpenEntries,
    pub store: Arc<MemoryStore>,
    /// A store on disk instead of `store`, for a debug fake-backend run with
    /// RESSWAP_DATA_DIR, so a damaged settings.json or a journal fixture placed there is read.
    pub disk_store: Option<Arc<DiskStore>>,
    pub clock: Arc<FakeClock>,
    pub display: Arc<FakeDisplay>,
    pub vendor: Arc<FakeVendor>,
    pub registry: Arc<FakeRegistry>,
    pub runner: Arc<FakeRunner>,
    pub processes: Arc<FakeProcessList>,
    pub run_key: Arc<FakeRunKey>,
    pub shortcuts: Arc<FakeShortcuts>,
    pub launcher: Arc<FakeLauncher>,
    pub folder_opener: Arc<FakeFolderOpener>,
    pub command_faults: CommandFaults,
    pub refused_fixture: Option<String>,
}

impl Fakes {
    /// Every fake on the fixture `profile`, with no fault scripted.
    pub fn new(profile: FixtureProfile) -> Self {
        let journal = OpenEntries::default();
        let registry = Arc::new(FakeRegistry::new(profile));
        Self {
            profile,
            store: Arc::new(MemoryStore::new()),
            disk_store: None,
            clock: Arc::new(FakeClock::new()),
            display: Arc::new(FakeDisplay::new(profile, journal.clone())),
            vendor: Arc::new(FakeVendor::new(profile, journal.clone())),
            runner: Arc::new(FakeRunner::new(journal.clone(), registry.clone())),
            registry,
            processes: Arc::new(FakeProcessList::new()),
            run_key: Arc::new(FakeRunKey::new(journal.clone())),
            shortcuts: Arc::new(FakeShortcuts::new(journal.clone())),
            launcher: Arc::new(FakeLauncher::new()),
            folder_opener: Arc::new(FakeFolderOpener::new()),
            command_faults: CommandFaults::default(),
            refused_fixture: None,
            journal,
        }
    }

    /// The same fakes with whole-command faults, as RESSWAP_FAKE_FAIL sets them.
    #[must_use]
    pub fn with_command_faults(mut self, faults: CommandFaults) -> Self {
        self.command_faults = faults;
        self
    }

    /// The same fakes with a RESSWAP_FAKE_PROFILE value that names no fixture, for start-up
    /// to raise.
    #[must_use]
    pub fn with_refused_fixture(mut self, name: &str) -> Self {
        self.refused_fixture = Some(name.to_owned());
        self
    }

    /// The same fakes on a clock that follows the real time, for a debug app run on the fake
    /// backend, so a countdown runs to its end there. Tests keep the still clock of `new`.
    #[must_use]
    pub fn with_real_time_clock(mut self) -> Self {
        self.clock = Arc::new(FakeClock::following_real_time());
        self
    }

    /// The same fakes with settings.json and journal.json in `folder` on disk, for a debug
    /// fake-backend run with RESSWAP_DATA_DIR. Tests keep the in-memory store.
    #[must_use]
    pub fn with_disk_store(mut self, folder: PathBuf) -> Self {
        self.disk_store = Some(Arc::new(DiskStore::new(folder)));
        self
    }

    /// The backends the core runs on: every one of these fakes.
    pub fn backends(&self) -> Backends {
        let store: Arc<dyn Store> = match &self.disk_store {
            Some(disk) => disk.clone(),
            None => self.store.clone(),
        };
        Backends {
            kind: BackendKind::Fake,
            fixture: Some(self.profile),
            display: self.display.clone(),
            clock: self.clock.clone(),
            journal: self.journal.clone(),
            vendor: Some(self.vendor.clone()),
            registry: Some(self.registry.clone()),
            runner: Some(self.runner.clone()),
            processes: Some(self.processes.clone()),
            run_key: Some(self.run_key.clone()),
            shortcuts: Some(self.shortcuts.clone()),
            launcher: Some(self.launcher.clone()),
            folder_opener: Some(self.folder_opener.clone()),
            store: Some(store),
            command_faults: self.command_faults.clone(),
            refused_fixture: self.refused_fixture.clone(),
        }
    }

    /// Whether any fake recorded a call that changes the PC.
    pub fn changed_anything(&self) -> bool {
        self.display.applied_anything()
            || self.vendor.changed_anything()
            || self.runner.ran_anything()
            || self.run_key.changed_anything()
            || !self.shortcuts.calls().is_empty()
            || !self.launcher.calls().is_empty()
    }
}

/// Picks the backends once, before setup reads anything. One switch selects every fake, and
/// a release build always gets the real backends (REQ-SEC-05). `app_data` is Tauri's app
/// local data folder; RESSWAP_DATA_DIR replaces it in a debug run.
pub fn select_backends(folders: &DataFolders, app_data: Option<PathBuf>) -> Backends {
    match choose_backend(requested_backend().as_deref()) {
        BackendKind::Fake => {
            let (fixture, refused) = choose_fixture(requested_fixture().as_deref());
            let faults = choose_command_faults(requested_command_faults().as_deref());
            let mut fakes = Fakes::new(fixture)
                .with_command_faults(faults)
                .with_real_time_clock();
            if let Some(folder) = folders.data_folder() {
                fakes = fakes.with_disk_store(folder);
            }
            match refused {
                Some(name) => fakes.with_refused_fixture(&name).backends(),
                None => fakes.backends(),
            }
        }
        BackendKind::Real => Backends::real(folders.data_folder().or(app_data)),
    }
}

/// The fake backend only for the exact value "fake", and only in a debug build.
fn choose_backend(requested: Option<&str>) -> BackendKind {
    if cfg!(debug_assertions) && requested == Some("fake") {
        BackendKind::Fake
    } else {
        BackendKind::Real
    }
}

/// The fixture a fake run starts from, and the name it refused. An unknown name runs
/// test-pc and comes back for start-up to raise, because a typo would otherwise run the
/// wrong fixture without a word; an empty value counts as unset. A release build never gets
/// here, and ignores the name anyway.
fn choose_fixture(requested: Option<&str>) -> (FixtureProfile, Option<String>) {
    let Some(name) = requested.filter(|name| cfg!(debug_assertions) && !name.is_empty()) else {
        return (FixtureProfile::default(), None);
    };
    match FixtureProfile::from_name(name) {
        Some(fixture) => (fixture, None),
        None => (FixtureProfile::default(), Some(name.to_owned())),
    }
}

/// The whole-command faults of a fake run; always empty in a release build.
fn choose_command_faults(requested: Option<&str>) -> CommandFaults {
    requested
        .filter(|_| cfg!(debug_assertions))
        .map(CommandFaults::parse)
        .unwrap_or_default()
}

#[cfg(debug_assertions)]
fn requested_backend() -> Option<String> {
    std::env::var("RESSWAP_BACKEND").ok()
}

#[cfg(not(debug_assertions))]
fn requested_backend() -> Option<String> {
    None
}

#[cfg(debug_assertions)]
fn requested_fixture() -> Option<String> {
    std::env::var("RESSWAP_FAKE_PROFILE").ok()
}

#[cfg(not(debug_assertions))]
fn requested_fixture() -> Option<String> {
    None
}

#[cfg(debug_assertions)]
fn requested_command_faults() -> Option<String> {
    std::env::var("RESSWAP_FAKE_FAIL").ok()
}

#[cfg(not(debug_assertions))]
fn requested_command_faults() -> Option<String> {
    None
}

/// Where a run writes its files. By default everything sits under the bundle identifier
/// folder in %LOCALAPPDATA%, the logs in its logs folder. A debug run started with
/// RESSWAP_DATA_DIR writes settings, the journal, logs and WebView2 data under that folder
/// instead, so test runs write only inside the repository.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DataFolders {
    root: Option<PathBuf>,
}

impl DataFolders {
    /// The default folders, or the debug override from the environment.
    pub fn from_env() -> Self {
        Self {
            root: choose_data_dir(requested_data_dir()),
        }
    }

    /// Everything under `root`, as a test or a debug run asks.
    pub fn in_folder(root: PathBuf) -> Self {
        Self { root: Some(root) }
    }

    pub fn is_overridden(&self) -> bool {
        self.root.is_some()
    }

    /// The folder of settings.json and journal.json when overridden; `None` means Tauri's
    /// app local data folder.
    pub fn data_folder(&self) -> Option<PathBuf> {
        self.root.clone()
    }

    /// The log folder when overridden; `None` means Tauri's app log folder.
    pub fn log_folder(&self) -> Option<PathBuf> {
        self.root.as_ref().map(|root| root.join("logs"))
    }

    /// The WebView2 data folder when overridden; `None` means WebView2's default under the
    /// bundle identifier folder.
    pub fn webview_folder(&self) -> Option<PathBuf> {
        self.root.as_ref().map(|root| root.join("webview"))
    }
}

/// The debug data folder as an absolute path; always `None` in a release build.
fn choose_data_dir(requested: Option<OsString>) -> Option<PathBuf> {
    let folder = PathBuf::from(requested.filter(|_| cfg!(debug_assertions))?);
    if folder.as_os_str().is_empty() {
        return None;
    }
    Some(std::path::absolute(&folder).unwrap_or(folder))
}

#[cfg(debug_assertions)]
fn requested_data_dir() -> Option<OsString> {
    std::env::var_os("RESSWAP_DATA_DIR")
}

#[cfg(not(debug_assertions))]
fn requested_data_dir() -> Option<OsString> {
    None
}

/// The store when the data folder is unknown: nothing to read, and every write refused, so
/// the journal refuses every change with JOURNAL_WRITE_FAILED (REQ-SAFE-01).
struct NoDataFolder;

impl Store for NoDataFolder {
    fn read(&self, _name: &str) -> io::Result<Option<Vec<u8>>> {
        Ok(None)
    }

    fn replace(&self, _name: &str, _bytes: &[u8], _backup: Backup) -> io::Result<()> {
        Err(io::Error::other("the data folder is not known"))
    }

    fn rename(&self, _from: &str, _to: &str) -> io::Result<()> {
        Err(io::Error::other("the data folder is not known"))
    }
}

/// The runtime state the core owns. The window renders snapshots of it.
pub(crate) struct CoreState {
    pub(crate) revision: u64,
    pub(crate) monitors: Vec<Monitor>,
    pub(crate) settings: Settings,
    pub(crate) profiles: Vec<Profile>,
    pub(crate) active: Vec<ActiveChange>,
    pub(crate) pending: Option<PendingChange>,
    pub(crate) auto_switch: AutoSwitchState,
    pub(crate) gpus: Vec<GpuInfo>,
    pub(crate) busy: Option<BusyState>,
    pub(crate) errors: Vec<RaisedError>,
    pub(crate) recovery: RecoveryState,
    pub(crate) update: Option<UpdateInfo>,
    pub(crate) raised_count: u64,
}

pub(crate) struct CoreInner {
    pub(crate) display: Arc<dyn DisplayBackend>,
    pub(crate) clock: Arc<dyn Clock>,
    pub(crate) fixture: Option<FixtureProfile>,
    pub(crate) command_faults: CommandFaults,
    pub(crate) refused_fixture: Option<String>,
    pub(crate) build: BuildInfo,
    /// journal.json: what ResSwap changed and has not put back (REQ-SAFE-01).
    pub(crate) journal: Journal,
    /// settings.json, read and written by Rust only (REQ-SET-01).
    pub(crate) settings: SettingsStore,
    pub(crate) folder_opener: Option<Arc<dyn FolderOpener>>,
    /// Errors from reading settings.json and journal.json, raised once the app runs.
    pub(crate) load_errors: Mutex<Vec<AppError>>,
    /// Whether settings.json and journal.json have a folder at all.
    pub(crate) has_data_folder: bool,
    /// Set when journal.json was damaged or newer and moved aside, so recovery stays
    /// incomplete until Restore everything puts back what Windows keeps (D-023).
    pub(crate) journal_lost: AtomicBool,
    /// Every change of the PC holds this, one at a time in arrival order (tokio's Mutex is
    /// FIFO). Read commands never take it.
    pub(crate) change_lock: tauri::async_runtime::Mutex<()>,
    pub(crate) state: Mutex<CoreState>,
    pub(crate) sink: RwLock<Arc<dyn EventSink>>,
    pub(crate) keep_mode: RwLock<Arc<dyn KeepModeWindow>>,
    pub(crate) log_folder: RwLock<Option<PathBuf>>,
    pub(crate) data_overridden: AtomicBool,
    /// The running revert countdown, at most one (apply/countdown.rs).
    pub(crate) countdown: Mutex<Option<Countdown>>,
    /// Changes waiting for a pending change on another monitor to end.
    pub(crate) pending_waiters: Mutex<Vec<tauri::async_runtime::Sender<()>>>,
    /// Set between PBT_APMSUSPEND and the next resume.
    pub(crate) suspended: AtomicBool,
    /// Set while a re-read for WM_DISPLAYCHANGE waits, so a burst of notices gives one.
    pub(crate) display_change_queued: AtomicBool,
    /// Journaled monitors a display change found gone, restored when they return.
    pub(crate) missing: Mutex<Vec<MonitorId>>,
}

/// The managed state behind every command, the tray, hotkeys, the watcher and Play
/// shortcuts. A cheap handle around shared state, so it moves into command tasks.
#[derive(Clone)]
pub struct Core {
    pub(crate) inner: Arc<CoreInner>,
}

/// A mutex whose data stays valid after a panic elsewhere.
pub(crate) fn guard<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Core {
    /// The core on `backends`. Reads settings.json and journal.json at once, through the
    /// same OpenEntries the fakes check, so the fakes follow the real journal; their errors
    /// wait for start-up to raise them.
    pub fn new(backends: Backends) -> Self {
        let has_data_folder = backends.store.is_some();
        let store = backends
            .store
            .unwrap_or_else(|| Arc::new(NoDataFolder) as Arc<dyn Store>);
        let (settings, settings_error) =
            SettingsStore::load(store.clone(), backends.clock.as_ref());
        let (journal, journal_error) =
            Journal::load(store, backends.clock.clone(), backends.journal.clone());
        let state = CoreState {
            revision: 0,
            monitors: Vec::new(),
            settings: settings.settings(),
            profiles: settings.profiles(),
            active: Vec::new(),
            pending: None,
            // Auto-switch is paused at every start (D-007).
            auto_switch: AutoSwitchState::default(),
            gpus: Vec::new(),
            busy: None,
            errors: Vec::new(),
            recovery: RecoveryState {
                status: RecoveryStatus::Clean,
                still_changed: Vec::new(),
            },
            update: None,
            raised_count: 0,
        };
        let journal_lost = AtomicBool::new(journal_error.is_some());
        let load_errors = settings_error.into_iter().chain(journal_error).collect();
        Self {
            inner: Arc::new(CoreInner {
                display: backends.display,
                clock: backends.clock,
                fixture: backends.fixture,
                command_faults: backends.command_faults,
                refused_fixture: backends.refused_fixture,
                build: BuildInfo {
                    version: String::from(env!("CARGO_PKG_VERSION")),
                    debug: cfg!(debug_assertions),
                    backend: backends.kind,
                },
                journal,
                settings,
                folder_opener: backends.folder_opener,
                load_errors: Mutex::new(load_errors),
                has_data_folder,
                journal_lost,
                change_lock: tauri::async_runtime::Mutex::new(()),
                state: Mutex::new(state),
                sink: RwLock::new(Arc::new(NoListeners)),
                keep_mode: RwLock::new(Arc::new(NoKeepModeWindow)),
                log_folder: RwLock::new(None),
                data_overridden: AtomicBool::new(false),
                countdown: Mutex::new(None),
                pending_waiters: Mutex::new(Vec::new()),
                suspended: AtomicBool::new(false),
                display_change_queued: AtomicBool::new(false),
                missing: Mutex::new(Vec::new()),
            }),
        }
    }

    pub fn backend(&self) -> BackendKind {
        self.inner.build.backend
    }

    /// The whole-command fault for `command`, if a debug fake run scripted one: the command
    /// answers its code and does nothing else.
    pub(crate) fn scripted(&self, command: CommandName) -> Result<(), AppError> {
        match self.inner.command_faults.code_for(command) {
            Some(code) => Err(AppError::with_code(
                code,
                Failure::new(format!("scripted fault for {}", command.name())),
            )),
            None => Ok(()),
        }
    }

    /// Where events go from now on.
    pub fn set_event_sink(&self, sink: Arc<dyn EventSink>) {
        *self
            .inner
            .sink
            .write()
            .unwrap_or_else(PoisonError::into_inner) = sink;
    }

    pub(crate) fn sink(&self) -> Arc<dyn EventSink> {
        Arc::clone(
            &self
                .inner
                .sink
                .read()
                .unwrap_or_else(PoisonError::into_inner),
        )
    }

    /// The keep-this-mode prompt the core opens for a pending change from now on.
    pub fn set_keep_mode_window(&self, window: Arc<dyn KeepModeWindow>) {
        *self
            .inner
            .keep_mode
            .write()
            .unwrap_or_else(PoisonError::into_inner) = window;
    }

    pub(crate) fn keep_mode(&self) -> Arc<dyn KeepModeWindow> {
        Arc::clone(
            &self
                .inner
                .keep_mode
                .read()
                .unwrap_or_else(PoisonError::into_inner),
        )
    }

    /// The folder open_log_folder shows: Tauri's app log folder, or the debug override.
    pub fn set_log_folder(&self, folder: Option<PathBuf>, overridden: bool) {
        *self
            .inner
            .log_folder
            .write()
            .unwrap_or_else(PoisonError::into_inner) = folder;
        self.inner
            .data_overridden
            .store(overridden, std::sync::atomic::Ordering::Relaxed);
    }

    /// The state stays consistent after a panic elsewhere, because every change to it is
    /// made under one lock with no call out while it is held.
    pub(crate) fn lock(&self) -> MutexGuard<'_, CoreState> {
        guard(&self.inner.state)
    }

    /// A full copy of the state for the window.
    pub fn snapshot(&self) -> AppState {
        let state = self.lock();
        AppState {
            revision: state.revision,
            monitors: state.monitors.clone(),
            settings: state.settings.clone(),
            profiles: state.profiles.clone(),
            active: state.active.clone(),
            pending: state.pending.clone(),
            auto_switch: state.auto_switch.clone(),
            gpus: state.gpus.clone(),
            busy: state.busy.clone(),
            errors: state.errors.clone(),
            recovery: state.recovery.clone(),
            update: state.update.clone(),
            build: self.inner.build.clone(),
        }
    }

    /// Sends state-changed when a window listens; with none, builds no snapshot.
    pub(crate) fn publish_state(&self) {
        let sink = self.sink();
        if sink.has_listeners() {
            emit_state_changed(sink.as_ref(), &self.snapshot());
        }
    }

    /// Changes the state under its lock, raises the revision, then sends state-changed, so
    /// every snapshot a window receives is newer than the one before.
    pub(crate) fn update<T>(&self, change: impl FnOnce(&mut CoreState) -> T) -> T {
        let out = {
            let mut state = self.lock();
            let out = change(&mut state);
            state.revision += 1;
            out
        };
        self.publish_state();
        out
    }

    /// Reads the monitors again and keeps them; state-changed follows when they differ.
    pub fn refresh_monitors(&self) -> Result<Vec<Monitor>, AppError> {
        let raw = self.inner.display.enumerate()?;
        let monitors: Vec<Monitor> = raw.iter().map(to_monitor).collect();
        let changed = {
            let mut state = self.lock();
            if state.monitors == monitors {
                false
            } else {
                state.monitors.clone_from(&monitors);
                state.revision += 1;
                true
            }
        };
        if changed {
            self.publish_state();
        }
        Ok(monitors)
    }

    /// Records an error no command caller is waiting for, collapsing repeats of the same
    /// code from the same source, and sends error-raised.
    pub fn raise(&self, source: ErrorSource, error: &AppError) {
        log::warn!("{source:?} raised {:?}: {error}", error.code());
        let ipc = error.to_ipc();
        let at_ms = self.inner.clock.epoch_ms();
        let raised = {
            let mut state = self.lock();
            let repeat = state
                .errors
                .iter()
                .position(|entry| entry.source == source && entry.error.code == ipc.code);
            let entry = if let Some(index) = repeat {
                let mut entry = state.errors.remove(index);
                entry.count = entry.count.saturating_add(1);
                entry.at_ms = at_ms;
                entry.error = ipc;
                entry
            } else {
                state.raised_count += 1;
                RaisedError {
                    id: format!("error-{}", state.raised_count),
                    source,
                    at_ms,
                    count: 1,
                    error: ipc,
                }
            };
            state.errors.insert(0, entry.clone());
            state.revision += 1;
            entry
        };
        let sink = self.sink();
        if sink.has_listeners() {
            emit_error_raised(sink.as_ref(), &raised);
            emit_state_changed(sink.as_ref(), &self.snapshot());
        }
    }

    /// Raises the errors from reading settings.json and journal.json, once.
    pub fn raise_load_errors(&self) {
        let errors: Vec<AppError> = std::mem::take(&mut *guard(&self.inner.load_errors));
        for error in &errors {
            self.raise(ErrorSource::Startup, error);
        }
        if !self.inner.has_data_folder {
            self.raise(
                ErrorSource::Startup,
                &AppError::internal(
                    "the data folder could not be found, so no change can be recorded",
                ),
            );
        }
    }

    pub async fn state(&self) -> Result<AppState, AppError> {
        self.scripted(CommandName::GetState)?;
        Ok(self.snapshot())
    }

    /// Every monitor, hidden ones included with their reason; the window lists hidden ones
    /// only while Settings.showHiddenMonitors is on (REQ-DISP-08).
    pub async fn list_monitors(&self) -> Result<Vec<Monitor>, AppError> {
        self.scripted(CommandName::ListMonitors)?;
        self.refresh_monitors()
    }

    pub async fn profiles(&self) -> Result<Vec<Profile>, AppError> {
        self.scripted(CommandName::ListProfiles)?;
        Ok(self.lock().profiles.clone())
    }

    pub async fn save_profile(&self, _request: Profile) -> Result<Profile, AppError> {
        self.scripted(CommandName::SaveProfile)?;
        Err(not_available(CommandName::SaveProfile))
    }

    pub async fn delete_profile(&self, _request: ProfileRef) -> Result<(), AppError> {
        self.scripted(CommandName::DeleteProfile)?;
        Err(not_available(CommandName::DeleteProfile))
    }

    /// Applies a profile for a Play shortcut, from the command line or a second start.
    pub async fn play_profile(
        &self,
        _profile_id: ProfileId,
        _source: ChangeSource,
    ) -> Result<ApplyOutcome, AppError> {
        Err(AppError::internal(
            "Play shortcuts are not available in this build; nothing was changed",
        ))
    }

    pub async fn set_auto_switch(
        &self,
        _request: SetAutoSwitchRequest,
    ) -> Result<AutoSwitchState, AppError> {
        self.scripted(CommandName::SetAutoSwitch)?;
        Err(not_available(CommandName::SetAutoSwitch))
    }

    pub async fn custom_entries(&self) -> Result<CustomEntryList, AppError> {
        self.scripted(CommandName::ListCustomEntries)?;
        Err(not_available(CommandName::ListCustomEntries))
    }

    pub async fn add_custom_mode(
        &self,
        _request: AddCustomModeRequest,
        _source: ChangeSource,
    ) -> Result<CustomChangeOutcome, AppError> {
        self.scripted(CommandName::AddCustomMode)?;
        Err(not_available(CommandName::AddCustomMode))
    }

    pub async fn remove_custom_entry(
        &self,
        _request: RemoveCustomEntryRequest,
        _source: ChangeSource,
    ) -> Result<CustomChangeOutcome, AppError> {
        self.scripted(CommandName::RemoveCustomEntry)?;
        Err(not_available(CommandName::RemoveCustomEntry))
    }

    pub async fn undo_all_changes(
        &self,
        _request: UndoAllRequest,
        _source: ChangeSource,
    ) -> Result<CustomChangeOutcome, AppError> {
        self.scripted(CommandName::UndoAllChanges)?;
        Err(not_available(CommandName::UndoAllChanges))
    }

    /// The settings as settings.json holds them (D-007 defaults on a first run).
    pub async fn settings(&self) -> Result<Settings, AppError> {
        self.scripted(CommandName::GetSettings)?;
        Ok(self.lock().settings.clone())
    }

    /// Checks every field in Rust (REQ-SEC-08), writes settings.json atomically, then keeps
    /// the stored settings in memory and sends state-changed (REQ-SET-04, REQ-SET-06).
    pub async fn save_settings(&self, request: SettingsInput) -> Result<Settings, AppError> {
        self.scripted(CommandName::SaveSettings)?;
        check_settings_input(&request, &self.inner.settings.profiles())?;
        let settings = self.inner.settings.update(|file| {
            file.settings.apply_input(request);
            file.settings.clone()
        })?;
        self.update(|state| state.settings.clone_from(&settings));
        Ok(settings)
    }

    pub async fn set_hotkey(&self, _request: SetHotkeyRequest) -> Result<Settings, AppError> {
        self.scripted(CommandName::SetHotkey)?;
        Err(not_available(CommandName::SetHotkey))
    }

    pub async fn set_autostart(&self, _request: SetAutostartRequest) -> Result<Settings, AppError> {
        self.scripted(CommandName::SetAutostart)?;
        Err(not_available(CommandName::SetAutostart))
    }

    pub async fn create_play_shortcut(
        &self,
        _request: ProfileRef,
    ) -> Result<ShortcutCreated, AppError> {
        self.scripted(CommandName::CreatePlayShortcut)?;
        Err(not_available(CommandName::CreatePlayShortcut))
    }

    pub async fn check_for_update(&self) -> Result<Option<UpdateInfo>, AppError> {
        self.scripted(CommandName::CheckForUpdate)?;
        Err(not_available(CommandName::CheckForUpdate))
    }

    pub async fn install_update(&self, _source: ChangeSource) -> Result<(), AppError> {
        self.scripted(CommandName::InstallUpdate)?;
        Err(not_available(CommandName::InstallUpdate))
    }

    /// Shows the log folder in Explorer, from Rust, so the window never learns or sends a
    /// path (REQ-SET-09, D-003).
    pub async fn open_log_folder(&self) -> Result<(), AppError> {
        self.scripted(CommandName::OpenLogFolder)?;
        let Some(opener) = &self.inner.folder_opener else {
            return Err(AppError::internal(
                "opening the log folder is not available in this build; nothing was opened",
            ));
        };
        let folder = self
            .inner
            .log_folder
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
            .ok_or_else(|| AppError::internal("the log folder is not known"))?;
        opener.open(&folder)
    }

    /// A read-only summary for the About section and bug reports. Changes nothing and makes
    /// no network call; folders carry environment variables, never the user folder.
    pub async fn diagnostics(&self) -> Result<Diagnostics, AppError> {
        self.scripted(CommandName::GetDiagnostics)?;
        let raw = self.inner.display.enumerate()?;
        let monitors: Vec<MonitorDiagnostics> = raw
            .iter()
            .map(|monitor| {
                let shown = to_monitor(monitor);
                MonitorDiagnostics {
                    id: shown.id,
                    name: shown.name,
                    adapter: shown.adapter,
                    output_technology: monitor.output_technology,
                    active: shown.active,
                    hidden: shown.hidden,
                    native: shown.native,
                    current: shown.current,
                    scaling: shown.scaling,
                }
            })
            .collect();
        let journal_open_items = self.inner.journal.items(|id| self.monitor_name(id, &raw));
        let overridden = self
            .inner
            .data_overridden
            .load(std::sync::atomic::Ordering::Relaxed);
        let folder = if overridden {
            DATA_DIR_TEXT
        } else {
            DATA_FOLDER_TEXT
        };
        let (os_name, os_build) = self.os_version();
        let state = self.lock();
        Ok(Diagnostics {
            build: self.inner.build.clone(),
            os_name,
            os_build,
            webview2_version: None,
            gpus: state.gpus.clone(),
            nvapi_version: None,
            monitors,
            nv_modes: None,
            journal_open_items,
            auto_switch: state.auto_switch.enabled,
            settings_folder: String::from(folder),
            log_folder: format!(r"{folder}\logs"),
        })
    }

    /// The Windows name and build get_diagnostics reports. On the fakes, the test PC as
    /// recorded on 11 Nov 2026 (Windows 11 Pro, build 26200). The Win32 reading waits for the
    /// diagnostics screen; until then the real backend reports "Windows" and build 0.
    fn os_version(&self) -> (String, u32) {
        match self.inner.fixture {
            Some(_) => (String::from("Windows 11 Pro"), 26_200),
            None => (String::from("Windows"), 0),
        }
    }

    /// Removes one entry from the raised errors. The window only sends IDs it received, so
    /// an unknown ID is INTERNAL.
    pub async fn dismiss_error(&self, request: ErrorRef) -> Result<(), AppError> {
        self.scripted(CommandName::DismissError)?;
        {
            let mut state = self.lock();
            let before = state.errors.len();
            state.errors.retain(|entry| entry.id != request.error_id);
            if state.errors.len() == before {
                return Err(AppError::internal("no raised error has this ID"));
            }
            state.revision += 1;
        }
        self.publish_state();
        Ok(())
    }
}

/// The longest revert countdown save_settings accepts, a provisional bound until the allowed
/// range is set at P2 (REQ-SET-08, D-028). Without it a countdown could keep an untested
/// mode on screen for years.
const MAX_REVERT_COUNTDOWN_SECS: u32 = 60;

/// Checks a save_settings request in Rust (REQ-SEC-08). The window's controls prevent each
/// refusal, so each one is INTERNAL. A countdown of 0 would revert at once, and one above
/// MAX_REVERT_COUNTDOWN_SECS would leave the safety revert too far away (D-028).
fn check_settings_input(input: &SettingsInput, profiles: &[Profile]) -> Result<(), AppError> {
    if input.revert_countdown_secs == 0 {
        return Err(AppError::internal(
            "refused settings: a revert countdown of 0 seconds",
        ));
    }
    if input.revert_countdown_secs > MAX_REVERT_COUNTDOWN_SECS {
        return Err(AppError::internal(format!(
            "refused settings: a revert countdown of {} seconds is longer than \
             {MAX_REVERT_COUNTDOWN_SECS} seconds",
            input.revert_countdown_secs
        )));
    }
    if let Some(monitor) = &input.selected_monitor {
        check_monitor_id(monitor)?;
    }
    if let Some(profile) = &input.play_stretched_profile
        && !(is_lowercase_uuid(profile) && profiles.iter().any(|known| known.id == *profile))
    {
        return Err(AppError::internal(
            "refused settings: playStretchedProfile names no saved profile",
        ));
    }
    Ok(())
}

/// The honest answer of a command whose work this build does not hold: INTERNAL, with the
/// reason in the detail, and nothing changed.
fn not_available(command: CommandName) -> AppError {
    AppError::internal(format!(
        "{} is not available in this build; nothing was changed",
        command.name()
    ))
}

/// Registers the core, the commands and the start-up order on `builder`, then builds the
/// app. A test passes `tauri::test::mock_builder()`; the real run builds the app first and
/// manages the core afterwards, because the data folder comes from the app's path resolver.
pub fn create_app<R: Runtime>(
    builder: tauri::Builder<R>,
    core: Core,
    action: StartAction,
    folders: DataFolders,
) -> tauri::Result<tauri::App<R>> {
    build_app(builder.manage(core), action, folders)
}

/// The builder steps every run shares. The core must be managed before the event loop
/// starts, because setup reads it.
fn build_app<R: Runtime>(
    builder: tauri::Builder<R>,
    action: StartAction,
    folders: DataFolders,
) -> tauri::Result<tauri::App<R>> {
    builder
        .manage(folders)
        .invoke_handler(ipc::commands::handler())
        .setup(move |app| {
            start_up(app.handle(), action);
            Ok(())
        })
        .build(tauri::generate_context!())
}

/// The notice window, kept for the life of the app (REQ-DISP-23).
#[derive(Default)]
struct SystemNotices(Mutex<Option<NotifyWindow>>);

/// The start-up order inside setup, recovery before anything that can change the PC
/// (REQ-SAFE-06, REQ-SAFE-07): settings and journal were read when the core was built; their
/// errors are raised, recovery runs, then the notice window and the first read, then the
/// start action. A failure is raised and start-up goes on with what works. Setup runs when
/// the event loop starts, which the mock runtime never does, so the integration tests call
/// this themselves.
pub fn start_up<R: Runtime>(app: &AppHandle<R>, action: StartAction) {
    let core = app.state::<Core>().inner().clone();
    let folders = app.state::<DataFolders>().inner().clone();
    log::info!(
        "ResSwap {} started: debug build {}, {:?} backend",
        env!("CARGO_PKG_VERSION"),
        cfg!(debug_assertions),
        core.backend()
    );
    match system::DLL_SEARCH_LIMITED.get() {
        Some(true) => log::info!("DLL search limited to System32"),
        Some(false) => log::warn!("SetDefaultDllDirectories refused the System32 limit"),
        None => {}
    }
    if folders.is_overridden() {
        log::info!(
            "RESSWAP_DATA_DIR moves settings, the journal, the logs and the WebView2 data for \
             this debug run"
        );
    }
    if let Some(fixture) = core.inner.fixture {
        log::info!("fake fixture {fixture:?}");
    }
    core.set_event_sink(Arc::new(TauriSink::new(app.clone())));
    let log_folder = folders
        .log_folder()
        .or_else(|| app.path().app_log_dir().ok());
    core.set_log_folder(log_folder, folders.is_overridden());
    let closed = core.clone();
    match TauriKeepModeWindow::start(
        app.clone(),
        folders.webview_folder(),
        Arc::new(move || closed.keep_window_closed()),
    ) {
        Ok(window) => core.set_keep_mode_window(Arc::new(window)),
        Err(error) => core.raise(ErrorSource::Startup, &error),
    }
    // A typo in a fixture name or a scripted fault would otherwise pass silently and make a
    // test misleading.
    if let Some(name) = &core.inner.refused_fixture {
        let error = AppError::internal(format!(
            "fake fixture name refused: {name}; this run uses test-pc"
        ));
        core.raise(ErrorSource::Startup, &error);
    }
    for entry in core.inner.command_faults.refused() {
        let error = AppError::internal(format!("scripted fault entry refused: {entry}"));
        core.raise(ErrorSource::Startup, &error);
    }
    core.raise_load_errors();
    // Before anything that can change the PC or react to a game (REQ-SAFE-06, REQ-SAFE-07).
    tauri::async_runtime::block_on(core.recover_at_start());
    if core.backend() == BackendKind::Real {
        start_notices(app, &core);
    }
    if let Err(error) = core.refresh_monitors() {
        core.raise(ErrorSource::Startup, &error);
    }
    match action {
        StartAction::Default => {}
        // This build has no tray, so a Run-value start opens the window to stay reachable.
        StartAction::Autostart => log::info!("started with --autostart"),
        StartAction::Play { profile_id } => {
            let play = core.play_profile(profile_id, ChangeSource::PlayShortcut);
            if let Err(error) = tauri::async_runtime::block_on(play) {
                core.raise(ErrorSource::PlayShortcut, &error);
            }
        }
    }
    if let Err(error) = shell::window::open_main(app, folders.webview_folder().as_deref()) {
        core.raise(ErrorSource::Startup, &error);
    }
}

/// Starts the hidden top-level notice window on the Win32 backend and keeps it for the
/// life of the app. Each notice only queues the core's work on a task, so the window thread
/// never waits on the core (REQ-DISP-23, REQ-SAFE-08).
fn start_notices<R: Runtime>(app: &AppHandle<R>, core: &Core) {
    let target = core.clone();
    let callback = Arc::new(move |event: SystemEvent| {
        let core = target.clone();
        tauri::async_runtime::spawn(async move { core.on_system_event(event).await });
    });
    match NotifyWindow::start(callback) {
        Ok(window) => {
            app.manage(SystemNotices::default());
            *guard(&app.state::<SystemNotices>().0) = Some(window);
        }
        Err(error) => core.raise(ErrorSource::Startup, &error),
    }
}

/// A second ResSwap.exe forwarded its arguments. It waits on the forwarding message, so
/// this only parses and queues the work, and never changes the PC itself.
fn on_second_instance<R: Runtime>(app: &AppHandle<R>, argv: Vec<String>, _cwd: String) {
    match parse_args(argv.into_iter().skip(1).map(OsString::from)) {
        Ok(StartAction::Default) => {
            let app = app.clone();
            tauri::async_runtime::spawn_blocking(move || {
                let folders = app.state::<DataFolders>().inner().clone();
                if let Err(error) =
                    shell::window::open_main(&app, folders.webview_folder().as_deref())
                {
                    app.state::<Core>().raise(ErrorSource::Startup, &error);
                }
            });
        }
        // Already running: nothing to do.
        Ok(StartAction::Autostart) => {}
        Ok(StartAction::Play { profile_id }) => {
            let core = app.state::<Core>().inner().clone();
            tauri::async_runtime::spawn(async move {
                if let Err(error) = core
                    .play_profile(profile_id, ChangeSource::PlayShortcut)
                    .await
                {
                    core.raise(ErrorSource::PlayShortcut, &error);
                }
            });
        }
        Err(BadArguments) => {
            log::warn!("a second start passed arguments ResSwap refuses; they were ignored");
        }
    }
}

/// The log plugin with its default line layout, `[date][time][target][level] message`,
/// which the measurement script and the first-paint test parse. It writes ResSwap.log in the
/// app log folder, or in `<RESSWAP_DATA_DIR>\logs` for a debug run that sets it. A full file
/// is renamed with its date and a new one started; the five newest old files are kept, so
/// the folder holds about 6 MB at most and a hardware round keeps its lines (D-027).
pub fn log_plugin<R: Runtime>(folders: &DataFolders) -> tauri::plugin::TauriPlugin<R> {
    use tauri_plugin_log::{RotationStrategy, Target, TargetKind};

    let file = match folders.log_folder() {
        Some(path) => TargetKind::Folder {
            path,
            file_name: Some(String::from(LOG_FILE_NAME)),
        },
        None => TargetKind::LogDir {
            file_name: Some(String::from(LOG_FILE_NAME)),
        },
    };
    let mut targets = vec![Target::new(file)];
    let level = if cfg!(debug_assertions) {
        targets.push(Target::new(TargetKind::Stdout));
        log::LevelFilter::Debug
    } else {
        log::LevelFilter::Info
    };
    tauri_plugin_log::Builder::new()
        .level(level)
        .targets(targets)
        .max_file_size(LOG_MAX_FILE_BYTES)
        .rotation_strategy(RotationStrategy::KeepSome(LOG_KEPT_OLD_FILES))
        .build()
}

/// Builds and runs the app: the single-instance plugin first, then the log, then the core.
/// The global-shortcut plugin joins with the hotkeys, the updater with the release work.
pub fn run(action: StartAction) {
    let folders = DataFolders::from_env();
    let builder = tauri::Builder::default()
        // First, so a second ResSwap.exe forwards its arguments and exits (REQ-SHELL-10).
        .plugin(tauri_plugin_single_instance::init(on_second_instance))
        .plugin(log_plugin(&folders));
    #[allow(
        clippy::expect_used,
        reason = "start-up invariant: without the app there is no window or log to report to"
    )]
    let app = build_app(builder, action, folders.clone()).expect("the Tauri app failed to build");
    // The data folder comes from the app's path resolver, so the core is built now, before
    // the event loop runs setup.
    let app_data = app.path().app_local_data_dir().ok();
    app.manage(Core::new(select_backends(&folders, app_data)));
    // Building the app starts the log plugin, so this is the first moment a line reaches the
    // log file; its epoch_ms is still the start of main().
    log_app_started();
    app.run(|handle, event| {
        if let RunEvent::Exit = event {
            // A pending change goes back before the process ends (reason Quit).
            let core = handle.state::<Core>().inner().clone();
            tauri::async_runtime::block_on(core.prepare_quit());
        }
    });
}

impl Core {
    /// The name a monitor has in `live`, or the last name the core showed for it.
    pub(crate) fn monitor_name(
        &self,
        id: &MonitorId,
        live: &[crate::display::RawMonitor],
    ) -> Option<String> {
        if let Some(found) = crate::display::monitors::find_monitor(id, live) {
            return crate::display::monitors::display_name(found.name.as_deref());
        }
        self.lock()
            .monitors
            .iter()
            .find(|monitor| crate::display::monitors::same_monitor(&monitor.id, id))
            .and_then(|monitor| monitor.name.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ipc::types::ApplyRequest;

    const ID: &str = "0f8fad5b-d9cb-469f-a165-70867728950e";

    fn args(list: &[&str]) -> Vec<OsString> {
        list.iter().map(OsString::from).collect()
    }

    #[test]
    fn req_shell_11_parse_args_accepts_three_forms() {
        assert_eq!(parse_args(args(&[])), Ok(StartAction::Default));
        assert_eq!(
            parse_args(args(&["--autostart"])),
            Ok(StartAction::Autostart)
        );
        assert_eq!(
            parse_args(args(&["--play", ID])),
            Ok(StartAction::Play {
                profile_id: ProfileId::parse(ID).unwrap()
            })
        );
    }

    #[test]
    fn req_shell_11_parse_args_refuses_everything_else() {
        let upper = ID.to_uppercase();
        for refused in [
            vec!["--unknown"],
            vec!["--autostart", "--autostart"],
            vec!["--autostart", "--play", ID],
            vec!["--play"],
            vec!["--play", upper.as_str()],
            vec!["--play", "not-a-uuid"],
            vec!["--play", ID, "--play", ID],
            vec!["--play", ID, "extra"],
            vec!["--PLAY", ID],
            vec!["/autostart"],
            vec![""],
        ] {
            assert_eq!(parse_args(args(&refused)), Err(BadArguments), "{refused:?}");
        }
    }

    #[test]
    fn req_shell_11_parse_args_refuses_text_that_is_not_unicode() {
        use std::os::windows::ffi::OsStringExt;
        // A lone surrogate cannot become a String.
        let broken = OsString::from_wide(&[0xD800]);
        assert_eq!(parse_args([broken]), Err(BadArguments));
        assert_ne!(BAD_ARGUMENTS, 0);
    }

    #[test]
    fn req_sec_05_fake_backend_only_in_debug_builds() {
        let expected = if cfg!(debug_assertions) {
            BackendKind::Fake
        } else {
            BackendKind::Real
        };
        assert_eq!(choose_backend(Some("fake")), expected);
        assert_eq!(choose_backend(None), BackendKind::Real);
        assert_eq!(choose_backend(Some("FAKE")), BackendKind::Real);
        assert_eq!(choose_backend(Some("real")), BackendKind::Real);
    }

    /// A debug app run on the fake backend gets the clock that follows the real time; the
    /// fakes a test builds keep the still clock.
    #[test]
    fn req_safe_03_app_runs_get_the_real_time_clock() {
        assert!(!Fakes::new(FixtureProfile::TestPc).clock.follows_real_time());
        let app = Fakes::new(FixtureProfile::TestPc).with_real_time_clock();
        assert!(app.clock.follows_real_time());
    }

    /// The other debug variables follow the same rule: a release build ignores each one.
    #[test]
    fn req_sec_05_debug_variables_only_in_debug_builds() {
        let debug = cfg!(debug_assertions);
        let (fixture, refused) = choose_fixture(Some("two-monitors"));
        assert_eq!(fixture == FixtureProfile::TwoMonitors, debug);
        assert_eq!(refused, None);
        // An unknown name runs test-pc; only a debug build keeps it for start-up to raise.
        let (fixture, refused) = choose_fixture(Some("two-monitor"));
        assert_eq!(fixture, FixtureProfile::TestPc);
        assert_eq!(refused.as_deref() == Some("two-monitor"), debug);
        assert_eq!(choose_fixture(Some("")), (FixtureProfile::TestPc, None));
        assert_eq!(choose_fixture(None), (FixtureProfile::TestPc, None));

        let faults = choose_command_faults(Some("apply_mode:MODE_REJECTED"));
        assert_eq!(faults.code_for(CommandName::ApplyMode).is_some(), debug);
        assert_eq!(choose_command_faults(None), CommandFaults::default());

        let folder = choose_data_dir(Some(OsString::from(r"C:\ResSwap-test-data")));
        assert_eq!(folder.is_some(), debug);
        assert_eq!(choose_data_dir(Some(OsString::new())), None);
        assert_eq!(choose_data_dir(None), None);
    }

    fn same<T: ?Sized, U: ?Sized>(a: &Arc<T>, b: &Arc<U>) -> bool {
        std::ptr::addr_eq(Arc::as_ptr(a), Arc::as_ptr(b))
    }

    fn same_part<T: ?Sized, U: ?Sized>(part: Option<&Arc<T>>, fake: &Arc<U>) -> bool {
        part.is_some_and(|part| same(part, fake))
    }

    /// One switch: the fake side is every fake from one `Fakes`, and the real side holds no
    /// fake at all.
    #[test]
    fn req_sec_05_one_switch_selects_every_fake() {
        let fakes = Fakes::new(FixtureProfile::TestPc);
        let backends = fakes.backends();
        assert_eq!(backends.kind, BackendKind::Fake);
        assert_eq!(backends.fixture, Some(FixtureProfile::TestPc));
        assert!(same(&backends.display, &fakes.display));
        assert!(same(&backends.clock, &fakes.clock));
        assert!(same_part(backends.vendor.as_ref(), &fakes.vendor));
        assert!(same_part(backends.registry.as_ref(), &fakes.registry));
        assert!(same_part(backends.runner.as_ref(), &fakes.runner));
        assert!(same_part(backends.processes.as_ref(), &fakes.processes));
        assert!(same_part(backends.run_key.as_ref(), &fakes.run_key));
        assert!(same_part(backends.shortcuts.as_ref(), &fakes.shortcuts));
        assert!(same_part(backends.launcher.as_ref(), &fakes.launcher));
        assert!(same_part(backends.store.as_ref(), &fakes.store));
        assert_eq!(backends.refused_fixture, None);

        assert!(same_part(
            backends.folder_opener.as_ref(),
            &fakes.folder_opener
        ));

        // The real side: the Win32 registry reader, a disk store in the data folder, and no
        // fake at all; parts a later phase builds stay empty.
        let real = Backends::real(Some(PathBuf::from(r"C:\ResSwap-test-data")));
        assert_eq!(real.kind, BackendKind::Real);
        assert_eq!(real.fixture, None);
        assert!(real.registry.is_some() && real.store.is_some());
        assert!(real.vendor.is_none() && real.runner.is_none());
        assert!(real.processes.is_none() && real.run_key.is_none());
        assert!(real.shortcuts.is_none() && real.launcher.is_none());
        assert!(real.folder_opener.is_some());
        assert_eq!(real.command_faults, CommandFaults::default());
        assert_eq!(real.refused_fixture, None);
        // With no data folder there is no store, and the core refuses every change.
        assert!(Backends::real(None).store.is_none());
    }

    #[test]
    fn req_ui_08_command_faults_answer_their_code() {
        let faults = CommandFaults::parse("apply_mode:MODE_REJECTED,get_settings:SETTINGS_CORRUPT");
        let fakes = Fakes::new(FixtureProfile::TestPc).with_command_faults(faults);
        let core = Core::new(fakes.backends());
        let request = ApplyRequest::Profile {
            profile_id: String::from(ID),
        };
        let apply = tauri::async_runtime::block_on(core.apply(request, ChangeSource::Window));
        let error = apply.unwrap_err();
        assert_eq!(error.code(), crate::error::ErrorCode::ModeRejected);
        assert_eq!(
            error.to_ipc().detail,
            "Windows did not accept the mode: scripted fault for apply_mode"
        );
        let settings = tauri::async_runtime::block_on(core.settings());
        assert_eq!(
            settings.unwrap_err().code(),
            crate::error::ErrorCode::SettingsCorrupt
        );
        // Commands without a fault answer as before.
        assert!(tauri::async_runtime::block_on(core.state()).is_ok());
        assert!(!fakes.changed_anything());
    }

    #[test]
    fn req_perf_05_timing_line_format() {
        let line = perf_line(
            "app.started",
            &[("mono_ms", &0), ("epoch_ms", &1_794_355_200_000_u64)],
        );
        assert_eq!(line, "PERF app.started mono_ms=0 epoch_ms=1794355200000");
        assert_eq!(perf_line("watch.snapshots", &[]), "PERF watch.snapshots");
        // Before main() marks its start, as in every test, no mono_ms exists.
        assert_eq!(mono_ms(), None);
    }

    #[test]
    fn req_ui_08_raised_errors_collapse_and_dismiss() {
        let fakes = Fakes::new(FixtureProfile::TestPc);
        let core = Core::new(fakes.backends());
        let first = AppError::internal("first");
        core.raise(ErrorSource::Startup, &first);
        fakes.clock.advance(1_000);
        core.raise(ErrorSource::Startup, &AppError::internal("second"));
        core.raise(ErrorSource::PlayShortcut, &first);
        let state = core.snapshot();
        assert_eq!(state.errors.len(), 2);
        assert_eq!(state.errors[0].source, ErrorSource::PlayShortcut);
        assert_eq!(state.errors[1].count, 2);
        // The time comes from the core's clock, the fake one here.
        assert_eq!(
            state.errors[1].at_ms,
            crate::system::fake::FAKE_EPOCH_MS + 1_000
        );
        assert_eq!(state.errors[1].error.detail, "internal error: second");
        assert_eq!(state.revision, 3);

        let dismiss = |id: &str| {
            tauri::async_runtime::block_on(core.dismiss_error(ErrorRef {
                error_id: id.to_owned(),
            }))
        };
        let id = state.errors[1].id.clone();
        assert!(dismiss(&id).is_ok());
        assert_eq!(core.snapshot().errors.len(), 1);
        let unknown = dismiss(&id).unwrap_err();
        assert_eq!(unknown.code(), crate::error::ErrorCode::Internal);
    }

    #[test]
    fn req_disp_01_refresh_reads_the_fake_and_counts_revisions() {
        let fakes = Fakes::new(FixtureProfile::TestPc);
        let core = Core::new(fakes.backends());
        assert_eq!(core.backend(), BackendKind::Fake);
        let monitors = core.refresh_monitors().unwrap();
        assert_eq!(monitors.len(), 2);
        assert!(!monitors[0].hidden);
        assert!(monitors[1].hidden);
        assert_eq!(core.snapshot().revision, 1);
        // Nothing differs on a second read, so the revision stays.
        core.refresh_monitors().unwrap();
        assert_eq!(core.snapshot().revision, 1);
        assert!(!fakes.display.applied_anything());
    }

    /// The data folder get_diagnostics shows is the bundle identifier's (D-011).
    #[test]
    fn req_set_01_data_folder_text_follows_the_bundle_identifier() {
        let config: serde_json::Value =
            serde_json::from_str(include_str!("../tauri.conf.json")).unwrap();
        let identifier = config["identifier"].as_str().unwrap();
        assert_eq!(DATA_FOLDER_TEXT, format!(r"%LOCALAPPDATA%\{identifier}"));
    }

    /// A debug fake-backend run with RESSWAP_DATA_DIR reads and writes its files on disk;
    /// the fakes a test builds keep the in-memory store.
    #[test]
    fn req_set_01_fake_runs_with_a_data_folder_use_the_disk() {
        let fakes = Fakes::new(FixtureProfile::TestPc);
        assert!(fakes.disk_store.is_none());
        let folder = PathBuf::from(r"C:\ResSwap-test-data");
        let on_disk = Fakes::new(FixtureProfile::TestPc).with_disk_store(folder.clone());
        let store = on_disk.backends().store.unwrap();
        let disk = on_disk.disk_store.as_ref().unwrap();
        assert!(same(&store, disk));
        assert_eq!(disk.folder(), folder.as_path());
    }
}
