//! IPC-only types: the application state, changes, requests, outcomes and diagnostics. The
//! TypeScript form of every IPC type is rendered from Rust into src/core/types/.

#![forbid(unsafe_code)]

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::display::types::{GpuVendor, Mode, Monitor, MonitorId, Scaling, ScalingChoice};
use crate::error::{ElevatedTool, IpcError, JournalItem, StepKind, StepReport};
use crate::profiles::Profile;
use crate::settings::{HotkeyAction, Settings};
use crate::vendor::{CustomEntry, CustomEntryList};

/// Where a change came from. Every source goes through the same methods on the core.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum ChangeSource {
    Window,
    Tray,
    Hotkey,
    Watcher,
    PlayShortcut,
    Recovery,
    Update,
}

/// A change ResSwap made and has not put back.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct ActiveChange {
    pub change_id: String,
    pub monitor: MonitorId,
    pub monitor_name: Option<String>,
    /// The mode before ResSwap's first change on this monitor.
    pub before: Mode,
    pub now: Mode,
    pub scaling: Scaling,
    /// Monitors this change turned off.
    pub turned_off: Vec<MonitorId>,
    pub profile_id: Option<String>,
    pub source: ChangeSource,
    /// Milliseconds since the Unix epoch.
    pub since_ms: u64,
}

/// A change to a mode never confirmed on that monitor, applied and waiting for Keep or
/// Revert.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct PendingChange {
    pub change_id: String,
    pub monitor: MonitorId,
    pub monitor_name: Option<String>,
    pub from: Mode,
    pub to: Mode,
    pub scaling: Scaling,
    /// Settings.revertCountdownSecs when the change started (10 by default, D-007).
    pub countdown_secs: u32,
    pub seconds_left: u32,
    pub source: ChangeSource,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct CountdownTick {
    pub change_id: String,
    pub seconds_left: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum RevertReason {
    Timeout,
    Player,
    KeepWindowClosed,
    Superseded,
    Quit,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct ModeReverted {
    pub change_id: String,
    pub monitor: MonitorId,
    pub monitor_name: Option<String>,
    pub reason: RevertReason,
    /// The mode read back after the revert.
    pub restored: Mode,
    pub steps: Vec<StepReport>,
}

/// What a custom-resolution change will do, shown to the player before any step runs.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct ChangePlan {
    pub plan_id: String,
    /// In the order they will run.
    pub steps: Vec<StepKind>,
    /// Each one shows an administrator prompt.
    pub elevated_tools: Vec<ElevatedTool>,
    /// The screen goes black briefly while the graphics driver restarts.
    pub driver_restart: bool,
    /// Entries made by other tools that this plan removes.
    pub other_entries: Vec<CustomEntry>,
    pub backup_first: bool,
    /// Why this route was chosen, for example the error that ruled out NVAPI.
    pub why: Option<IpcError>,
}

/// Everything the window renders. The core owns it; the window keeps the snapshot with the
/// highest revision.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct AppState {
    /// Grows by one with every change to the state.
    pub revision: u64,
    pub monitors: Vec<Monitor>,
    pub settings: Settings,
    pub profiles: Vec<Profile>,
    pub active: Vec<ActiveChange>,
    pub pending: Option<PendingChange>,
    pub auto_switch: AutoSwitchState,
    pub gpus: Vec<GpuInfo>,
    /// Set while an operation that changes the PC runs.
    pub busy: Option<BusyState>,
    /// Errors raised in the background, newest first, until dismissed.
    pub errors: Vec<RaisedError>,
    pub recovery: RecoveryState,
    pub update: Option<UpdateInfo>,
    pub build: BuildInfo,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct AutoSwitchState {
    /// False at every start (D-007).
    pub enabled: bool,
    /// IDs of profiles with auto-switch on.
    pub watched: Vec<String>,
    pub matched: Option<WatcherMatch>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum WatcherPhase {
    Started,
    Exited,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct WatcherMatch {
    pub profile_id: String,
    pub profile_name: String,
    pub exe: String,
    pub phase: WatcherPhase,
    /// For Exited: seconds until the restore runs.
    pub restore_in_secs: Option<u32>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum NvapiStatus {
    Available,
    Missing,
    NotApplicable,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct GpuInfo {
    pub name: String,
    pub vendor: GpuVendor,
    pub driver_version: Option<String>,
    pub nvapi: NvapiStatus,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum BusyAction {
    Applying,
    Reverting,
    Restoring,
    AddingCustom,
    RemovingCustom,
    Undoing,
    InstallingUpdate,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct BusyState {
    pub action: BusyAction,
    pub step: StepKind,
    /// True while an administrator prompt is open.
    pub waiting_for_uac: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum RecoveryStatus {
    Clean,
    Recovered,
    Incomplete,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct RecoveryState {
    pub status: RecoveryStatus,
    pub still_changed: Vec<JournalItem>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum ErrorSource {
    Watcher,
    Hotkey,
    Tray,
    PlayShortcut,
    Startup,
    Resume,
    DisplayChange,
    UpdateCheck,
}

/// An error that happened with no command caller waiting.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct RaisedError {
    pub id: String,
    pub source: ErrorSource,
    pub at_ms: u64,
    /// Repeats of the same code from the same source collapse into one entry.
    pub count: u32,
    pub error: IpcError,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct UpdateInfo {
    pub version: String,
    pub date: Option<String>,
    pub notes: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum BackendKind {
    Real,
    Fake,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct BuildInfo {
    pub version: String,
    pub debug: bool,
    /// Fake only in a debug build started on the fake backend.
    pub backend: BackendKind,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum ApplyRequest {
    Mode {
        monitor: MonitorId,
        mode: Mode,
        scaling: ScalingChoice,
    },
    Profile {
        profile_id: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum ApplyOutcome {
    Applied {
        change: ActiveChange,
        steps: Vec<StepReport>,
        warnings: Vec<IpcError>,
    },
    PendingConfirm {
        pending: PendingChange,
        steps: Vec<StepReport>,
        warnings: Vec<IpcError>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct ChangeRef {
    pub change_id: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum ConfirmOutcome {
    Kept {
        // Boxed, so the empty NotPending answer stays small.
        change: Box<ActiveChange>,
        warnings: Vec<IpcError>,
    },
    NotPending,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum RevertOutcome {
    Reverted { reverted: ModeReverted },
    NotPending,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct RestoreNativeRequest {
    /// Null means every monitor ResSwap changed.
    pub monitor: Option<MonitorId>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct RestoreOutcome {
    pub restored: Vec<MonitorId>,
    pub steps: Vec<StepReport>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct ProfileRef {
    pub profile_id: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct SetAutoSwitchRequest {
    pub enabled: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct AddCustomModeRequest {
    pub monitor: MonitorId,
    pub width: u32,
    pub height: u32,
    pub approved_plan: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct RemoveCustomEntryRequest {
    pub entry_id: String,
    pub approved_plan: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct UndoAllRequest {
    pub approved_plan: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum CustomChangeOutcome {
    Done {
        entries: CustomEntryList,
        steps: Vec<StepReport>,
    },
    NeedsApproval {
        // Boxed, so the other outcomes stay small.
        plan: Box<ChangePlan>,
    },
    AlreadyGone {
        entries: CustomEntryList,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct SetHotkeyRequest {
    pub action: HotkeyAction,
    /// Null removes the binding.
    pub accelerator: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct SetAutostartRequest {
    pub enabled: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct ShortcutCreated {
    /// Shown to the player; never sent back to the core.
    pub path: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct ErrorRef {
    pub error_id: String,
}

/// A read-only summary for the About section and for bug reports.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct Diagnostics {
    pub build: BuildInfo,
    /// For example "Windows 11 Pro".
    pub os_name: String,
    pub os_build: u32,
    pub webview2_version: Option<String>,
    pub gpus: Vec<GpuInfo>,
    /// The NVAPI interface version string when NVAPI loaded.
    pub nvapi_version: Option<String>,
    pub monitors: Vec<MonitorDiagnostics>,
    pub nv_modes: Option<NvModesDiagnostics>,
    pub journal_open_items: Vec<JournalItem>,
    pub auto_switch: bool,
    /// Shown with environment variables, never with the expanded user folder.
    pub settings_folder: String,
    pub log_folder: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct MonitorDiagnostics {
    pub id: MonitorId,
    pub name: Option<String>,
    pub adapter: String,
    /// The CCD outputTechnology value as a number.
    pub output_technology: i64,
    pub active: bool,
    pub hidden: bool,
    pub native: Option<Mode>,
    pub current: Option<Mode>,
    pub scaling: Scaling,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct NvModesDiagnostics {
    /// The display class subkey, for example "0001".
    pub adapter_key: String,
    pub value_type: String,
    pub groups: u32,
    pub own_groups: u32,
}

#[cfg(test)]
mod ts_export {
    use std::collections::BTreeMap;
    use std::path::{Path, PathBuf};

    use ts_rs::{Config, TS};

    use super::*;
    use crate::display::types::{Aspect, HiddenReason, Preset, RefreshRate};
    use crate::error::{
        ErrorCode, ErrorContext, JournalItemKind, ProfileField, ProfileProblem, SettingsSource,
        StepStatus,
    };
    use crate::ipc::commands::CommandName;
    use crate::ipc::events::CoreEvent;
    use crate::settings::{ConfirmedMode, HotkeyBinding, SettingsInput};
    use crate::vendor::{CustomSource, Origin};

    type Render = fn(&Config) -> (String, String);

    /// Every type the window sees, once. A type missing here has no TypeScript file.
    const TS_TYPES: &[Render] = &[
        // display/types.rs
        render::<MonitorId>,
        render::<RefreshRate>,
        render::<Mode>,
        render::<Scaling>,
        render::<ScalingChoice>,
        render::<Aspect>,
        render::<Preset>,
        render::<GpuVendor>,
        render::<HiddenReason>,
        render::<Monitor>,
        // error.rs
        render::<ErrorCode>,
        render::<IpcError>,
        render::<ErrorContext>,
        render::<ProfileProblem>,
        render::<ProfileField>,
        render::<SettingsSource>,
        render::<StepKind>,
        render::<StepStatus>,
        render::<StepReport>,
        render::<JournalItemKind>,
        render::<JournalItem>,
        render::<ElevatedTool>,
        // profiles/mod.rs and settings/mod.rs
        render::<Profile>,
        render::<HotkeyAction>,
        render::<HotkeyBinding>,
        render::<ConfirmedMode>,
        render::<Settings>,
        render::<SettingsInput>,
        // vendor/mod.rs
        render::<CustomSource>,
        render::<Origin>,
        render::<CustomEntry>,
        render::<CustomEntryList>,
        // ipc/types.rs
        render::<ChangeSource>,
        render::<ActiveChange>,
        render::<PendingChange>,
        render::<CountdownTick>,
        render::<RevertReason>,
        render::<ModeReverted>,
        render::<ChangePlan>,
        render::<AppState>,
        render::<AutoSwitchState>,
        render::<WatcherPhase>,
        render::<WatcherMatch>,
        render::<NvapiStatus>,
        render::<GpuInfo>,
        render::<BusyAction>,
        render::<BusyState>,
        render::<RecoveryStatus>,
        render::<RecoveryState>,
        render::<ErrorSource>,
        render::<RaisedError>,
        render::<UpdateInfo>,
        render::<BackendKind>,
        render::<BuildInfo>,
        render::<ApplyRequest>,
        render::<ApplyOutcome>,
        render::<ChangeRef>,
        render::<ConfirmOutcome>,
        render::<RevertOutcome>,
        render::<RestoreNativeRequest>,
        render::<RestoreOutcome>,
        render::<ProfileRef>,
        render::<SetAutoSwitchRequest>,
        render::<AddCustomModeRequest>,
        render::<RemoveCustomEntryRequest>,
        render::<UndoAllRequest>,
        render::<CustomChangeOutcome>,
        render::<SetHotkeyRequest>,
        render::<SetAutostartRequest>,
        render::<ShortcutCreated>,
        render::<ErrorRef>,
        render::<Diagnostics>,
        render::<MonitorDiagnostics>,
        render::<NvModesDiagnostics>,
        // ipc/commands.rs and ipc/events.rs
        render::<CommandName>,
        render::<CoreEvent>,
    ];

    /// The file name and the file text ts-rs renders for one type, in memory.
    fn render<T: TS + 'static>(config: &Config) -> (String, String) {
        let name = T::output_path()
            .expect("an exported type has a file name")
            .to_string_lossy()
            .into_owned();
        let text = T::export_to_string(config).expect("ts-rs renders the type");
        (name, text)
    }

    /// The ts-rs settings from src-tauri/.cargo/config.toml, which Cargo sets for this run.
    fn config() -> Config {
        let config = Config::from_env();
        assert_eq!(
            config.large_int(),
            "number",
            "TS_RS_LARGE_INT is not set: run cargo from src-tauri, where .cargo/config.toml sets it"
        );
        config
    }

    fn types_dir() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../src/core/types")
    }

    fn rendered() -> BTreeMap<String, String> {
        let config = config();
        let files: BTreeMap<String, String> =
            TS_TYPES.iter().map(|render| render(&config)).collect();
        assert_eq!(
            files.len(),
            TS_TYPES.len(),
            "two types render to one file name"
        );
        files
    }

    fn files_on_disk() -> Vec<PathBuf> {
        match std::fs::read_dir(types_dir()) {
            Ok(entries) => entries
                .map(|entry| entry.expect("readable entry").path())
                .collect(),
            Err(_) => Vec::new(),
        }
    }

    fn file_name(path: &Path) -> String {
        path.file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default()
    }

    #[test]
    fn req_ui_15_ts_bindings_current() {
        let expected = rendered();
        let mut stale: Vec<String> = Vec::new();
        for (name, text) in &expected {
            let on_disk = std::fs::read(types_dir().join(name)).unwrap_or_default();
            if on_disk != text.as_bytes() {
                stale.push(name.clone());
            }
        }
        for path in files_on_disk() {
            let name = file_name(&path);
            if !expected.contains_key(&name) {
                stale.push(name);
            }
        }
        assert!(
            stale.is_empty(),
            "src/core/types is stale: {stale:?}. From src-tauri run: cargo test --all-features ts_bindings_write -- --ignored"
        );
    }

    #[test]
    #[ignore = "writes src/core/types; run by name"]
    fn req_ui_15_ts_bindings_write() {
        let expected = rendered();
        std::fs::create_dir_all(types_dir()).expect("create src/core/types");
        for (name, text) in &expected {
            std::fs::write(types_dir().join(name), text).expect("write a type file");
        }
        for path in files_on_disk() {
            if !expected.contains_key(&file_name(&path)) {
                std::fs::remove_file(&path).expect("remove a file for a type no longer listed");
            }
        }
    }

    #[test]
    fn req_ui_15_tagged_unions_use_kind_and_camel_case() {
        let request = ApplyRequest::Profile {
            profile_id: String::from("0f8fad5b-d9cb-469f-a165-70867728950e"),
        };
        assert_eq!(
            serde_json::to_value(&request).unwrap(),
            serde_json::json!({ "kind": "profile", "profileId": "0f8fad5b-d9cb-469f-a165-70867728950e" })
        );
        assert_eq!(
            serde_json::to_value(ConfirmOutcome::NotPending).unwrap(),
            serde_json::json!({ "kind": "notPending" })
        );
        assert_eq!(
            serde_json::to_value(RestoreNativeRequest { monitor: None }).unwrap(),
            serde_json::json!({ "monitor": null })
        );
    }
}
