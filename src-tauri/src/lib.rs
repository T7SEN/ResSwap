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
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError, RwLock};
use std::time::Instant;

use tauri::{AppHandle, Manager, Runtime};

use crate::apply::OpenEntries;
use crate::display::DisplayBackend;
use crate::display::fake::{FakeDisplay, FixtureProfile};
use crate::display::monitors::to_monitor;
use crate::display::types::Monitor;
use crate::display::win32::Win32Display;
use crate::elevation::ElevatedToolRunner;
use crate::elevation::fake::FakeRunner;
use crate::error::{AppError, Failure};
use crate::ipc::commands::{CommandFaults, CommandName};
use crate::ipc::events::{
    EventSink, NoListeners, TauriSink, emit_error_raised, emit_state_changed,
};
use crate::ipc::types::{
    ActiveChange, AddCustomModeRequest, AppState, ApplyOutcome, ApplyRequest, AutoSwitchState,
    BackendKind, BuildInfo, BusyState, ChangeRef, ChangeSource, ConfirmOutcome,
    CustomChangeOutcome, Diagnostics, ErrorRef, ErrorSource, GpuInfo, PendingChange, ProfileRef,
    RaisedError, RecoveryState, RecoveryStatus, RemoveCustomEntryRequest, RestoreNativeRequest,
    RestoreOutcome, RevertOutcome, RevertReason, SetAutoSwitchRequest, SetAutostartRequest,
    SetHotkeyRequest, ShortcutCreated, UndoAllRequest, UpdateInfo,
};
use crate::profiles::{Profile, ProfileId};
use crate::settings::atomic::Store;
use crate::settings::fake::MemoryStore;
use crate::settings::{Settings, SettingsInput};
use crate::shell::autostart::RunKey;
use crate::shell::fake::{FakeLauncher, FakeRunKey, FakeShortcuts};
use crate::shell::shortcuts::{Launcher, ShortcutWriter};
use crate::system::clock::{Clock, SystemClock, epoch_ms_now};
use crate::system::fake::{FakeClock, FakeProcessList};
use crate::system::processes::ProcessList;
use crate::vendor::fake::FakeVendor;
use crate::vendor::nvidia::fake::FakeRegistry;
use crate::vendor::nvidia::nv_modes::RegistryReader;
use crate::vendor::{CustomEntryList, VendorExtras};

/// Exit code for an argument list ResSwap refuses. main() exits with it before the Tauri
/// builder runs, so nothing reaches a running instance (REQ-SHELL-11).
pub const BAD_ARGUMENTS: i32 = 2;

/// The log file's name, without its extension.
const LOG_FILE_NAME: &str = "ResSwap";

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
    pub vendor: Option<Arc<dyn VendorExtras>>,
    pub registry: Option<Arc<dyn RegistryReader>>,
    pub runner: Option<Arc<dyn ElevatedToolRunner>>,
    pub processes: Option<Arc<dyn ProcessList>>,
    pub run_key: Option<Arc<dyn RunKey>>,
    pub shortcuts: Option<Arc<dyn ShortcutWriter>>,
    pub launcher: Option<Arc<dyn Launcher>>,
    pub store: Option<Arc<dyn Store>>,
    /// Whole-command faults; empty on the real backends.
    pub command_faults: CommandFaults,
    /// A RESSWAP_FAKE_PROFILE value that names no fixture, which start-up raises; `None` on
    /// the real backends.
    pub refused_fixture: Option<String>,
}

impl Backends {
    /// The backends for the player's PC.
    pub fn real() -> Self {
        Self {
            kind: BackendKind::Real,
            fixture: None,
            display: Arc::new(Win32Display),
            clock: Arc::new(SystemClock::new()),
            vendor: None,
            registry: None,
            runner: None,
            processes: None,
            run_key: None,
            shortcuts: None,
            launcher: None,
            store: None,
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
    pub clock: Arc<FakeClock>,
    pub display: Arc<FakeDisplay>,
    pub vendor: Arc<FakeVendor>,
    pub registry: Arc<FakeRegistry>,
    pub runner: Arc<FakeRunner>,
    pub processes: Arc<FakeProcessList>,
    pub run_key: Arc<FakeRunKey>,
    pub shortcuts: Arc<FakeShortcuts>,
    pub launcher: Arc<FakeLauncher>,
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
            clock: Arc::new(FakeClock::new()),
            display: Arc::new(FakeDisplay::new(profile, journal.clone())),
            vendor: Arc::new(FakeVendor::new(profile, journal.clone())),
            runner: Arc::new(FakeRunner::new(journal.clone(), registry.clone())),
            registry,
            processes: Arc::new(FakeProcessList::new()),
            run_key: Arc::new(FakeRunKey::new(journal.clone())),
            shortcuts: Arc::new(FakeShortcuts::new(journal.clone())),
            launcher: Arc::new(FakeLauncher::new()),
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

    /// The backends the core runs on: every one of these fakes.
    pub fn backends(&self) -> Backends {
        Backends {
            kind: BackendKind::Fake,
            fixture: Some(self.profile),
            display: self.display.clone(),
            clock: self.clock.clone(),
            vendor: Some(self.vendor.clone()),
            registry: Some(self.registry.clone()),
            runner: Some(self.runner.clone()),
            processes: Some(self.processes.clone()),
            run_key: Some(self.run_key.clone()),
            shortcuts: Some(self.shortcuts.clone()),
            launcher: Some(self.launcher.clone()),
            store: Some(self.store.clone()),
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
/// a release build always gets the real backends (REQ-SEC-05).
pub fn select_backends() -> Backends {
    match choose_backend(requested_backend().as_deref()) {
        BackendKind::Fake => {
            let (fixture, refused) = choose_fixture(requested_fixture().as_deref());
            let faults = choose_command_faults(requested_command_faults().as_deref());
            let fakes = Fakes::new(fixture).with_command_faults(faults);
            match refused {
                Some(name) => fakes.with_refused_fixture(&name).backends(),
                None => fakes.backends(),
            }
        }
        BackendKind::Real => Backends::real(),
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
/// RESSWAP_DATA_DIR writes logs and WebView2 data under that folder instead, so test runs
/// write only inside the repository.
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

/// The runtime state the core owns. The window renders snapshots of it.
struct CoreState {
    revision: u64,
    monitors: Vec<Monitor>,
    settings: Settings,
    profiles: Vec<Profile>,
    active: Vec<ActiveChange>,
    pending: Option<PendingChange>,
    auto_switch: AutoSwitchState,
    gpus: Vec<GpuInfo>,
    busy: Option<BusyState>,
    errors: Vec<RaisedError>,
    recovery: RecoveryState,
    update: Option<UpdateInfo>,
    raised_count: u64,
}

struct CoreInner {
    display: Arc<dyn DisplayBackend>,
    clock: Arc<dyn Clock>,
    fixture: Option<FixtureProfile>,
    command_faults: CommandFaults,
    refused_fixture: Option<String>,
    build: BuildInfo,
    state: Mutex<CoreState>,
    sink: RwLock<Arc<dyn EventSink>>,
}

/// The managed state behind every command, the tray, hotkeys, the watcher and Play
/// shortcuts. A cheap handle around shared state, so it moves into command tasks.
#[derive(Clone)]
pub struct Core {
    inner: Arc<CoreInner>,
}

impl Core {
    pub fn new(backends: Backends) -> Self {
        let state = CoreState {
            revision: 0,
            monitors: Vec::new(),
            settings: Settings::default(),
            profiles: Vec::new(),
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
                state: Mutex::new(state),
                sink: RwLock::new(Arc::new(NoListeners)),
            }),
        }
    }

    pub fn backend(&self) -> BackendKind {
        self.inner.build.backend
    }

    /// The whole-command fault for `command`, if a debug fake run scripted one: the command
    /// answers its code and does nothing else.
    fn scripted(&self, command: CommandName) -> Result<(), AppError> {
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

    fn sink(&self) -> Arc<dyn EventSink> {
        Arc::clone(
            &self
                .inner
                .sink
                .read()
                .unwrap_or_else(PoisonError::into_inner),
        )
    }

    /// The state stays consistent after a panic elsewhere, because every change to it is
    /// made under one lock with no call out while it is held.
    fn lock(&self) -> MutexGuard<'_, CoreState> {
        self.inner
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
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
    fn publish_state(&self) {
        let sink = self.sink();
        if sink.has_listeners() {
            emit_state_changed(sink.as_ref(), &self.snapshot());
        }
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

    pub async fn state(&self) -> Result<AppState, AppError> {
        self.scripted(CommandName::GetState)?;
        Ok(self.snapshot())
    }

    pub async fn list_monitors(&self) -> Result<Vec<Monitor>, AppError> {
        self.scripted(CommandName::ListMonitors)?;
        self.refresh_monitors()
    }

    pub async fn apply(
        &self,
        _request: ApplyRequest,
        _source: ChangeSource,
    ) -> Result<ApplyOutcome, AppError> {
        self.scripted(CommandName::ApplyMode)?;
        Err(not_available(CommandName::ApplyMode))
    }

    /// Keeps the pending change. A change that is not pending is not an error: the window
    /// shows the mode-reverted result instead.
    pub async fn confirm(
        &self,
        request: ChangeRef,
        _source: ChangeSource,
    ) -> Result<ConfirmOutcome, AppError> {
        self.scripted(CommandName::ConfirmMode)?;
        if self.is_pending(&request.change_id) {
            return Err(not_available(CommandName::ConfirmMode));
        }
        Ok(ConfirmOutcome::NotPending)
    }

    /// Puts back the state saved before the pending change.
    pub async fn revert(
        &self,
        request: ChangeRef,
        _reason: RevertReason,
    ) -> Result<RevertOutcome, AppError> {
        self.scripted(CommandName::RevertMode)?;
        if self.is_pending(&request.change_id) {
            return Err(not_available(CommandName::RevertMode));
        }
        Ok(RevertOutcome::NotPending)
    }

    fn is_pending(&self, change_id: &str) -> bool {
        self.lock()
            .pending
            .as_ref()
            .is_some_and(|pending| pending.change_id == change_id)
    }

    pub async fn restore_native(
        &self,
        _request: RestoreNativeRequest,
        _source: ChangeSource,
    ) -> Result<RestoreOutcome, AppError> {
        self.scripted(CommandName::RestoreNative)?;
        Err(not_available(CommandName::RestoreNative))
    }

    pub async fn restore_everything(
        &self,
        _source: ChangeSource,
    ) -> Result<RestoreOutcome, AppError> {
        self.scripted(CommandName::RestoreEverything)?;
        Err(not_available(CommandName::RestoreEverything))
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

    /// The settings in memory: the defaults (D-007) until settings.json is read.
    pub async fn settings(&self) -> Result<Settings, AppError> {
        self.scripted(CommandName::GetSettings)?;
        Ok(self.lock().settings.clone())
    }

    pub async fn save_settings(&self, _request: SettingsInput) -> Result<Settings, AppError> {
        self.scripted(CommandName::SaveSettings)?;
        Err(not_available(CommandName::SaveSettings))
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

    pub async fn open_log_folder(&self) -> Result<(), AppError> {
        self.scripted(CommandName::OpenLogFolder)?;
        Err(not_available(CommandName::OpenLogFolder))
    }

    pub async fn diagnostics(&self) -> Result<Diagnostics, AppError> {
        self.scripted(CommandName::GetDiagnostics)?;
        Err(not_available(CommandName::GetDiagnostics))
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

/// The honest answer of a command whose work this build does not hold: INTERNAL, with the
/// reason in the detail, and nothing changed.
fn not_available(command: CommandName) -> AppError {
    AppError::internal(format!(
        "{} is not available in this build; nothing was changed",
        command.name()
    ))
}

/// Registers the core, the commands and the start-up order on `builder`, then builds the
/// app. The real run passes `tauri::Builder::default()` with the plugins already added; a
/// test passes `tauri::test::mock_builder()`.
pub fn create_app<R: Runtime>(
    builder: tauri::Builder<R>,
    core: Core,
    action: StartAction,
    folders: DataFolders,
) -> tauri::Result<tauri::App<R>> {
    builder
        .manage(core)
        .manage(folders)
        .invoke_handler(ipc::commands::handler())
        .setup(move |app| {
            start_up(app.handle(), action);
            Ok(())
        })
        .build(tauri::generate_context!())
}

/// The start-up order inside setup. A failure is raised and start-up goes on with what
/// works. Setup runs when the event loop starts, which the mock runtime never does, so the
/// integration tests call this themselves.
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
        log::info!("RESSWAP_DATA_DIR moves the logs and the WebView2 data for this debug run");
    }
    if let Some(fixture) = core.inner.fixture {
        log::info!("fake fixture {fixture:?}");
    }
    core.set_event_sink(Arc::new(TauriSink::new(app.clone())));
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
/// app log folder, or in `<RESSWAP_DATA_DIR>\logs` for a debug run that sets it.
pub fn log_plugin<R: Runtime>(folders: &DataFolders) -> tauri::plugin::TauriPlugin<R> {
    use tauri_plugin_log::{Target, TargetKind};

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
        .build()
}

/// Builds and runs the app: the single-instance plugin first, then the log, then the core.
/// The global-shortcut plugin joins with the hotkeys, the updater with the release work.
pub fn run(action: StartAction) {
    let folders = DataFolders::from_env();
    let core = Core::new(select_backends());
    let builder = tauri::Builder::default()
        // First, so a second ResSwap.exe forwards its arguments and exits (REQ-SHELL-10).
        .plugin(tauri_plugin_single_instance::init(on_second_instance))
        .plugin(log_plugin(&folders));
    #[allow(
        clippy::expect_used,
        reason = "start-up invariant: without the app there is no window or log to report to"
    )]
    let app = create_app(builder, core, action, folders).expect("the Tauri app failed to build");
    // Building the app starts the log plugin, so this is the first moment a line reaches the
    // log file; its epoch_ms is still the start of main().
    log_app_started();
    app.run(|_app, _event| {});
}

#[cfg(test)]
mod tests {
    use super::*;

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

        let real = Backends::real();
        assert_eq!(real.kind, BackendKind::Real);
        assert_eq!(real.fixture, None);
        assert!(real.vendor.is_none() && real.registry.is_none() && real.runner.is_none());
        assert!(real.processes.is_none() && real.run_key.is_none());
        assert!(real.shortcuts.is_none() && real.launcher.is_none() && real.store.is_none());
        assert_eq!(real.command_faults, CommandFaults::default());
        assert_eq!(real.refused_fixture, None);
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
}
