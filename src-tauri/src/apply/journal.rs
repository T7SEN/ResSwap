//! The state journal: what ResSwap changed and has not put back (REQ-SAFE-01, REQ-SAFE-02).
//! journal.json is written whole before every call that changes the PC and after every
//! verified step. A Ticket comes only from a write that opens an entry or moves it to a
//! changing status, so no changing call runs before its record is on disk. The record of
//! open entries that every fake checks a Ticket against follows the journal.

#![forbid(unsafe_code)]

use std::collections::BTreeSet;
use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::apply::Ticket;
use crate::display::modes::{rational_eq, to_mode};
use crate::display::monitors::same_monitor;
use crate::display::types::{MonitorId, Scaling, ScalingChoice};
use crate::display::{GdiFlag, Rational, RawMode, RawMonitor, Route, RouteChoice};
use crate::error::{AppError, Failure, JournalItem, JournalItemKind};
use crate::ipc::types::ChangeSource;
use crate::settings::atomic::{Backup, DiskStore, MAX_FILE_BYTES, Store};
use crate::settings::migrations::{self, MigrationError, Step};
use crate::system::clock::Clock;
use crate::vendor::CustomSource;

/// The journal file in the data folder (REQ-SAFE-01).
pub const JOURNAL_FILE: &str = "journal.json";

/// The journal schema this build writes and reads. A build that changes the shape raises it
/// and adds a step to `JOURNAL_STEPS` with a fixture test.
pub const JOURNAL_SCHEMA_VERSION: u32 = 1;

/// The journal migrations, in order; none while version 1 is current.
pub const JOURNAL_STEPS: &[Step] = &[];

const VERSION_POINTER: &str = "/schemaVersion";

/// The longest change or operation ID the journal accepts. A lowercase UUID has 36
/// characters.
const MAX_ID_LEN: usize = 64;

/// journal.json as a whole. Never crosses the IPC; `JournalItem` is the window's summary.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct JournalFile {
    pub schema_version: u32,
    /// Milliseconds since the Unix epoch of the last write; 0 before the first.
    pub written_ms: u64,
    /// Baselines of the monitors with an open change.
    pub monitors: Vec<MonitorRecord>,
    /// The layout before the first layout change (multi-monitor work).
    pub layout: Option<LayoutRecord>,
    /// Display changes not yet put back.
    pub changes: Vec<ChangeRecord>,
    /// Custom-entry writes and driver restarts in flight (NVIDIA work).
    pub operations: Vec<OperationRecord>,
    /// Custom entries ResSwap added and has not removed (REQ-NV-10).
    pub own_entries: Vec<OwnEntryRecord>,
}

/// One monitor's state just before ResSwap's first open change on it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MonitorRecord {
    /// Device path plus EDID ids (REQ-DISP-20).
    pub identity: MonitorId,
    /// DISPLAYCONFIG_ADAPTER_NAME.adapterDevicePath, to map a saved adapter LUID to this
    /// boot's LUID.
    pub adapter_device_path: String,
    /// The GDI device name when the baseline was taken; for the log only, never for matching.
    pub gdi_name_hint: String,
    pub baseline: DisplayState,
    pub taken_ms: u64,
}

/// A monitor's mode, scaling and position as read back.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DisplayState {
    pub width: u32,
    pub height: u32,
    /// The CCD target mode's vSyncFreq, never the GDI integer (REQ-DISP-17).
    pub refresh: Rational,
    /// Read from the CCD path.
    pub scaling: Scaling,
    pub position_x: i32,
    pub position_y: i32,
    /// The DEVMODEW fields of the GDI route, when that route read them.
    pub gdi: Option<GdiModeRecord>,
}

/// The DEVMODEW fields the GDI route needs to put a mode back. dmDisplayFlags shares a union
/// with dmNup, and dmDisplayFixedOutput sits in the display member of the first union.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GdiModeRecord {
    pub pels_width: u32,
    pub pels_height: u32,
    pub bits_per_pel: u32,
    pub display_frequency: u32,
    pub display_flags: u32,
    pub display_fixed_output: u32,
}

/// The layout before the first layout change: every path and mode from one
/// QueryDisplayConfig call. Adapter LUIDs last one boot, so they are mapped again by device
/// path before use.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LayoutRecord {
    /// Whether the query used QDC_VIRTUAL_MODE_AWARE.
    pub virtual_mode_aware: bool,
    pub adapters: Vec<AdapterRecord>,
    pub paths: Vec<PathRecord>,
    pub modes: Vec<ModeRecord>,
    /// The identity of each path's target, in path order.
    pub targets: Vec<MonitorId>,
    pub taken_ms: u64,
}

/// A LUID as read: LowPart and HighPart.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LuidRecord {
    pub low_part: u32,
    pub high_part: i32,
}

/// One adapter of the layout: its LUID as read, and the device path that finds it again.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AdapterRecord {
    pub luid: LuidRecord,
    pub adapter_device_path: String,
}

/// DISPLAYCONFIG_PATH_INFO field by field. Converted only in display/win32.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PathRecord {
    pub source_info: PathSourceRecord,
    pub target_info: PathTargetRecord,
    pub flags: u32,
}

/// DISPLAYCONFIG_PATH_SOURCE_INFO. The union of modeInfoIdx and its bit fields is kept as
/// the raw u32.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PathSourceRecord {
    pub adapter_id: LuidRecord,
    pub id: u32,
    pub mode_info_idx: u32,
    pub status_flags: u32,
}

/// DISPLAYCONFIG_PATH_TARGET_INFO. Enumerations and BOOL keep their raw i32 values; the
/// union of modeInfoIdx and its bit fields is kept as the raw u32.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PathTargetRecord {
    pub adapter_id: LuidRecord,
    pub id: u32,
    pub mode_info_idx: u32,
    pub output_technology: i32,
    pub rotation: i32,
    pub scaling: i32,
    pub refresh_rate: Rational,
    pub scan_line_ordering: i32,
    pub target_available: i32,
    pub status_flags: u32,
}

/// DISPLAYCONFIG_MODE_INFO: the header fields and the union member that `info_type` names.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ModeRecord {
    pub info_type: i32,
    pub id: u32,
    pub adapter_id: LuidRecord,
    pub info: ModeInfoRecord,
}

/// The union member of DISPLAYCONFIG_MODE_INFO.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ModeInfoRecord {
    Target(TargetModeRecord),
    Source(SourceModeRecord),
    DesktopImage(DesktopImageRecord),
}

/// DISPLAYCONFIG_TARGET_MODE's DISPLAYCONFIG_VIDEO_SIGNAL_INFO. The union of videoStandard
/// and its bit fields is kept as the raw u32.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TargetModeRecord {
    pub pixel_rate: u64,
    pub h_sync_freq: Rational,
    pub v_sync_freq: Rational,
    pub active_size: RegionRecord,
    pub total_size: RegionRecord,
    pub video_standard: u32,
    pub scan_line_ordering: i32,
}

/// DISPLAYCONFIG_2DREGION.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RegionRecord {
    pub cx: u32,
    pub cy: u32,
}

/// DISPLAYCONFIG_SOURCE_MODE.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SourceModeRecord {
    pub width: u32,
    pub height: u32,
    pub pixel_format: i32,
    pub position: PointRecord,
}

/// POINTL.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PointRecord {
    pub x: i32,
    pub y: i32,
}

/// RECTL.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RectRecord {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

/// DISPLAYCONFIG_DESKTOP_IMAGE_INFO.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DesktopImageRecord {
    pub path_source_size: PointRecord,
    pub desktop_image_region: RectRecord,
    pub desktop_image_clip: RectRecord,
}

/// Where a display change stands. Intent, Reverting and Restoring are written before a call
/// that changes the PC; Applied, Pending and Kept after a verified step.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ChangeStatus {
    Intent,
    Applied,
    Pending,
    Kept,
    Reverting,
    Restoring,
}

impl ChangeStatus {
    /// Whether the write of this status comes right before a call that changes the PC, and
    /// so gives a Ticket.
    pub const fn is_changing(self) -> bool {
        matches!(self, Self::Intent | Self::Reverting | Self::Restoring)
    }

    /// The moves of the record lifecycle: intent to applied or pending; pending to kept or
    /// reverting; any status to restoring. Intent and reverting may also go to reverting,
    /// for the put-back after a rejected apply and a second try. Pending may also go to
    /// applied: a game or Windows set another mode during the countdown, so ResSwap stops it
    /// and keeps the change open as an active change, never fighting the new mode.
    const fn may_follow(self, from: Self) -> bool {
        match self {
            Self::Intent => false,
            Self::Applied => matches!(from, Self::Intent | Self::Pending),
            Self::Pending => matches!(from, Self::Intent),
            Self::Kept => matches!(from, Self::Pending),
            Self::Reverting => matches!(from, Self::Intent | Self::Pending | Self::Reverting),
            Self::Restoring => true,
        }
    }
}

/// One display change not yet put back, written before the change with the full previous
/// state (REQ-SAFE-02).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ChangeRecord {
    pub change_id: String,
    pub kind: JournalItemKind,
    /// None for a layout change.
    pub monitor: Option<MonitorId>,
    /// The state just before this change; revert_mode returns to it.
    pub previous: DisplayState,
    pub target: DisplayState,
    pub scaling_requested: ScalingChoice,
    /// The route the apply used, so a revert takes the same one.
    pub route: Route,
    /// Some exactly when `route` is Gdi.
    pub gdi_flag: Option<GdiFlag>,
    pub status: ChangeStatus,
    pub source: ChangeSource,
    pub profile_id: Option<String>,
    pub started_ms: u64,
    pub updated_ms: u64,
}

impl ChangeRecord {
    /// The route and GDI flag the apply used.
    pub fn route_choice(&self) -> RouteChoice {
        RouteChoice {
            route: self.route,
            gdi_flag: self.gdi_flag,
        }
    }
}

/// What a custom-entry operation does.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum OperationKind {
    NvModesWrite,
    NvModesBackupRestore,
    DriverRestart,
}

/// Where an operation stands. Intent and ToolRunning are written before an elevated tool
/// runs; Written, RestartDue and Done after a verified step. Done removes the record.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum OperationStatus {
    Intent,
    ToolRunning,
    Written,
    RestartDue,
    Done,
}

impl OperationStatus {
    const fn may_follow(self, from: Self) -> bool {
        match self {
            Self::Intent => false,
            Self::ToolRunning => !matches!(from, Self::Done),
            Self::Written => matches!(from, Self::ToolRunning),
            Self::RestartDue => matches!(from, Self::Intent | Self::ToolRunning | Self::Written),
            Self::Done => matches!(from, Self::ToolRunning | Self::Written | Self::RestartDue),
        }
    }
}

/// A custom-entry write or a driver restart in flight.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OperationRecord {
    pub op_id: String,
    pub kind: OperationKind,
    /// Checked again against a fresh enumeration before use.
    pub adapter_instance_id: String,
    /// The display class key number as found; compared, never trusted.
    pub adapter_key: String,
    /// REG_MULTI_SZ or REG_SZ, as found.
    pub value_type: String,
    pub before_value: Vec<String>,
    pub expected_value: Vec<String>,
    /// A file name in the backups folder of the data folder.
    pub backup_file: Option<String>,
    pub status: OperationStatus,
    pub started_ms: u64,
    pub updated_ms: u64,
}

/// A custom resolution entry ResSwap added and has not removed, so list_custom_entries
/// marks it as its own (REQ-NV-10).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OwnEntryRecord {
    pub entry_id: String,
    pub source: CustomSource,
    pub width: u32,
    pub height: u32,
    /// The NV_Modes group text as written; None for an NVAPI custom display.
    pub raw_group: Option<String>,
    pub adapter_instance_id: String,
    pub added_ms: u64,
    pub backup_file: Option<String>,
}

impl DisplayState {
    /// The mode part of this state.
    pub fn raw_mode(&self) -> RawMode {
        RawMode {
            width: self.width,
            height: self.height,
            refresh: self.refresh,
        }
    }

    /// Whether `mode` has this state's size and an equal rational (REQ-DISP-17).
    pub fn shows(&self, mode: &RawMode) -> bool {
        self.width == mode.width
            && self.height == mode.height
            && rational_eq(self.refresh, mode.refresh)
    }

    /// The state of `monitor` as a backend read it: its current mode, scaling and position.
    /// None when the monitor has no current mode (not active).
    pub fn of_monitor(monitor: &RawMonitor) -> Option<Self> {
        let current = monitor.current?;
        Some(Self {
            width: current.width,
            height: current.height,
            refresh: current.refresh,
            scaling: monitor.scaling,
            position_x: monitor.position_x,
            position_y: monitor.position_y,
            gdi: None,
        })
    }
}

impl MonitorRecord {
    /// The baseline record of `monitor` as a fresh read shows it. None when the monitor has
    /// no current mode.
    pub fn baseline_of(monitor: &RawMonitor, taken_ms: u64) -> Option<Self> {
        Some(Self {
            identity: monitor.id.clone(),
            adapter_device_path: monitor.adapter_device_path.clone(),
            gdi_name_hint: monitor.gdi_name.clone(),
            baseline: DisplayState::of_monitor(monitor)?,
            taken_ms,
        })
    }
}

impl Default for JournalFile {
    fn default() -> Self {
        Self::empty()
    }
}

impl JournalFile {
    /// A journal with nothing in it, at the current schema version.
    pub fn empty() -> Self {
        Self {
            schema_version: JOURNAL_SCHEMA_VERSION,
            written_ms: 0,
            monitors: Vec::new(),
            layout: None,
            changes: Vec::new(),
            operations: Vec::new(),
            own_entries: Vec::new(),
        }
    }

    /// Whether anything waits to be put back. Own entries alone do not count: they stay
    /// until the player removes them.
    pub fn needs_recovery(&self) -> bool {
        !self.monitors.is_empty()
            || self.layout.is_some()
            || !self.changes.is_empty()
            || !self.operations.is_empty()
    }

    /// The baseline of `monitor`, matched by identity.
    pub fn baseline_of(&self, monitor: &MonitorId) -> Option<&MonitorRecord> {
        self.monitors
            .iter()
            .find(|record| same_monitor(&record.identity, monitor))
    }

    /// The open changes on `monitor`, oldest first.
    pub fn changes_on<'a>(
        &'a self,
        monitor: &'a MonitorId,
    ) -> impl Iterator<Item = &'a ChangeRecord> + 'a {
        self.changes.iter().filter(move |change| {
            change
                .monitor
                .as_ref()
                .is_some_and(|on| same_monitor(on, monitor))
        })
    }

    /// The change with this ID.
    pub fn change(&self, change_id: &str) -> Option<&ChangeRecord> {
        self.changes
            .iter()
            .find(|change| change.change_id == change_id)
    }

    /// What is still changed, for Diagnostics and `stillChanged`: one item per open change,
    /// the layout when it is left with no change open (the snapshot taken before the first
    /// change is the baseline of the open changes, not a change of its own), one item per
    /// operation in flight.
    /// `name_of` gives a monitor's name from a fresh read; the journal stores none.
    pub fn items(&self, name_of: impl Fn(&MonitorId) -> Option<String>) -> Vec<JournalItem> {
        let mut items: Vec<JournalItem> = self
            .changes
            .iter()
            .map(|change| JournalItem {
                kind: change.kind,
                monitor_name: change.monitor.as_ref().and_then(&name_of),
                mode: match change.kind {
                    JournalItemKind::Layout | JournalItemKind::NvModes => None,
                    JournalItemKind::Mode
                    | JournalItemKind::Scaling
                    | JournalItemKind::NvapiCustomDisplay => {
                        Some(to_mode(change.target.raw_mode()))
                    }
                },
            })
            .collect();
        if self.layout.is_some() && self.changes.is_empty() {
            items.push(JournalItem {
                kind: JournalItemKind::Layout,
                monitor_name: None,
                mode: None,
            });
        }
        items.extend(self.operations.iter().map(|_| JournalItem {
            kind: JournalItemKind::NvModes,
            monitor_name: None,
            mode: None,
        }));
        items
    }

    /// Every ID a Ticket can carry: the open changes and the operations in flight.
    fn ids(&self) -> BTreeSet<String> {
        self.changes
            .iter()
            .map(|change| change.change_id.clone())
            .chain(
                self.operations
                    .iter()
                    .map(|operation| operation.op_id.clone()),
            )
            .collect()
    }

    fn has_id(&self, id: &str) -> bool {
        self.changes.iter().any(|change| change.change_id == id)
            || self
                .operations
                .iter()
                .any(|operation| operation.op_id == id)
    }

    /// The first check that fails, if any. A file that fails one is treated as damaged.
    fn problem(&self) -> Option<&'static str> {
        if self.schema_version != JOURNAL_SCHEMA_VERSION {
            return Some("schemaVersion is not the version this build writes");
        }
        let mut ids = BTreeSet::new();
        let all_ids = self
            .changes
            .iter()
            .map(|change| change.change_id.as_str())
            .chain(
                self.operations
                    .iter()
                    .map(|operation| operation.op_id.as_str()),
            );
        for id in all_ids {
            if !is_valid_id(id) {
                return Some("a change or operation ID is empty, too long or has other characters");
            }
            if !ids.insert(id) {
                return Some("two records share an ID");
            }
        }
        let mut entries = BTreeSet::new();
        if !self
            .own_entries
            .iter()
            .all(|entry| entries.insert(entry.entry_id.as_str()))
        {
            return Some("two own entries share an ID");
        }
        for (index, record) in self.monitors.iter().enumerate() {
            if record.baseline.refresh.denominator == 0 {
                return Some("a baseline refresh rate has the denominator 0");
            }
            if self.monitors[..index]
                .iter()
                .any(|earlier| same_monitor(&earlier.identity, &record.identity))
            {
                return Some("two baselines name the same monitor");
            }
        }
        for change in &self.changes {
            if let Some(problem) = route_problem(change) {
                return Some(problem);
            }
            if change.previous.refresh.denominator == 0 || change.target.refresh.denominator == 0 {
                return Some("a change's refresh rate has the denominator 0");
            }
            if change
                .monitor
                .as_ref()
                .is_some_and(|monitor| self.baseline_of(monitor).is_none())
            {
                return Some("a change names a monitor with no baseline");
            }
        }
        None
    }
}

/// A change or operation ID: 1 to 64 ASCII letters, digits or hyphens, so a lowercase UUID
/// passes and nothing from the file can carry a path or a line break into the log.
fn is_valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= MAX_ID_LEN
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
}

fn route_problem(change: &ChangeRecord) -> Option<&'static str> {
    match (change.route, change.gdi_flag) {
        (Route::Gdi, Some(_)) | (Route::Ccd, None) => None,
        _ => Some("a change's GDI flag does not match its route"),
    }
}

/// The shared record of the IDs whose journal entry is open: the journal opens an ID with
/// the write that creates its record and closes it with the write that removes it. A fake
/// that changes the PC panics when its Ticket names no open entry (`pc_changes_need_ticket`).
#[derive(Clone, Debug, Default)]
pub struct OpenEntries {
    open: Arc<Mutex<BTreeSet<String>>>,
}

impl OpenEntries {
    // The set stays valid after a panic elsewhere, so a poisoned lock is recovered.
    fn set(&self) -> MutexGuard<'_, BTreeSet<String>> {
        self.open.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Whether the entry for `change_id` is open.
    pub fn is_open(&self, change_id: &str) -> bool {
        self.set().contains(change_id)
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

    /// Opens an entry and hands out its Ticket, as a journal write does.
    #[cfg(test)]
    pub(crate) fn open_for_tests(&self, change_id: &str) -> Ticket {
        self.set().insert(change_id.to_owned());
        ticket(change_id)
    }

    /// Closes the entry a Ticket names, as a recorded outcome does.
    #[cfg(test)]
    pub(crate) fn close_for_tests(&self, ticket: &Ticket) {
        self.set().remove(ticket.change_id());
    }

    /// Brings the set in line with a write that turned `before` into `after`.
    fn follow(&self, before: &BTreeSet<String>, after: &BTreeSet<String>) {
        let mut set = self.set();
        for closed in before.difference(after) {
            set.remove(closed);
        }
        for opened in after.difference(before) {
            set.insert(opened.clone());
        }
    }
}

/// Only the journal builds a Ticket, and only after the write that justifies it succeeded.
fn ticket(id: &str) -> Ticket {
    Ticket {
        change_id: id.to_owned(),
        _private: (),
    }
}

fn internal(detail: &str) -> AppError {
    AppError::internal(format!("journal: {detail}"))
}

fn write_failed(detail: String) -> AppError {
    AppError::JournalWriteFailed(Box::new(Failure::new(detail)))
}

/// Why journal.json could not be used at start.
#[derive(Debug, thiserror::Error)]
enum Damage {
    #[error("the read failed: {0}")]
    Unreadable(std::io::Error),
    #[error("the file is larger than the 1 MiB cap")]
    TooLarge,
    #[error("the file is not valid JSON: {0}")]
    NotJson(serde_json::Error),
    #[error("{0}")]
    Version(MigrationError),
    #[error("the file does not match the journal shape: {0}")]
    Shape(serde_json::Error),
    #[error("a record failed its check: {0}")]
    Check(&'static str),
}

/// Reads journal.json: the size cap, the JSON, the migrations on the raw JSON, the shape
/// with unknown fields refused at every depth, then the checks.
fn read_journal(store: &dyn Store) -> Result<Option<JournalFile>, Damage> {
    let Some(bytes) = store.read(JOURNAL_FILE).map_err(Damage::Unreadable)? else {
        return Ok(None);
    };
    if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > MAX_FILE_BYTES {
        return Err(Damage::TooLarge);
    }
    let raw: Value = serde_json::from_slice(&bytes).map_err(Damage::NotJson)?;
    let current = migrations::run(raw, VERSION_POINTER, JOURNAL_STEPS, JOURNAL_SCHEMA_VERSION)
        .map_err(Damage::Version)?;
    let file: JournalFile = serde_json::from_value(current).map_err(Damage::Shape)?;
    match file.problem() {
        Some(problem) => Err(Damage::Check(problem)),
        None => Ok(Some(file)),
    }
}

struct JournalState {
    file: JournalFile,
    /// Why writes are refused: a damaged or newer journal.json that could not be moved
    /// aside, which a new journal would overwrite.
    write_refused: Option<String>,
}

/// journal.json in memory and on disk, with one writer: the holder of the pipeline lock.
/// Every write changes a copy, writes it whole through the store, and only then becomes
/// memory, so a failed write leaves memory, disk and the open entries as they were.
pub struct Journal {
    store: Arc<dyn Store>,
    clock: Arc<dyn Clock>,
    open: OpenEntries,
    state: Mutex<JournalState>,
}

impl Journal {
    /// Reads journal.json from `folder`, the data folder the caller chose.
    pub fn in_folder(
        folder: &Path,
        clock: Arc<dyn Clock>,
        open: OpenEntries,
    ) -> (Self, Option<AppError>) {
        Self::load(Arc::new(DiskStore::new(folder)), clock, open)
    }

    /// Reads journal.json from `store` and opens every ID it holds in `open`. A missing file
    /// starts an empty journal. A file a newer build wrote is never read for recovery and
    /// never overwritten: it is renamed journal.newer-<ms>.json. A file that fails to read,
    /// parse or pass its checks is renamed journal.damaged-<ms>.json. Both start a new
    /// journal and return RECOVERY_INCOMPLETE for start-up to raise, so the player can use
    /// Restore everything. When the rename fails, the file stays and every write is refused
    /// with JOURNAL_WRITE_FAILED, so no change runs without a record and the old file is
    /// never overwritten.
    pub fn load(
        store: Arc<dyn Store>,
        clock: Arc<dyn Clock>,
        open: OpenEntries,
    ) -> (Self, Option<AppError>) {
        let (file, write_refused, error) = match read_journal(store.as_ref()) {
            Ok(Some(file)) => (file, None, None),
            Ok(None) => (JournalFile::empty(), None, None),
            Err(damage) => {
                let kind = match damage {
                    Damage::Version(MigrationError::Newer { .. }) => "newer",
                    _ => "damaged",
                };
                let aside = format!("journal.{kind}-{}.json", clock.epoch_ms());
                let mut detail = format!("{JOURNAL_FILE}: {damage}");
                let write_refused = match store.rename(JOURNAL_FILE, &aside) {
                    Ok(()) => {
                        detail.push_str(&format!(
                            "; kept as {aside}; a new journal starts, so what it listed is not \
                             restored at start"
                        ));
                        None
                    }
                    Err(error) => {
                        detail.push_str(&format!(
                            "; it could not be moved aside ({error}), so every change is \
                             refused until it can be"
                        ));
                        Some(format!(
                            "{JOURNAL_FILE} holds a {kind} file that could not be moved aside: \
                             {error}"
                        ))
                    }
                };
                let error = AppError::RecoveryIncomplete(Box::new(Failure::new(detail)));
                (JournalFile::empty(), write_refused, Some(error))
            }
        };
        open.follow(&BTreeSet::new(), &file.ids());
        let journal = Self {
            store,
            clock,
            open,
            state: Mutex::new(JournalState {
                file,
                write_refused,
            }),
        };
        (journal, error)
    }

    // Memory is replaced only after a write succeeded, so it stays valid after a panic.
    fn state(&self) -> MutexGuard<'_, JournalState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The write protocol: change a copy, serialise it, replace journal.json through the
    /// temp file, flush and rename, then keep the copy and move the open entries with it. A
    /// failure is JOURNAL_WRITE_FAILED and nothing changes (REQ-SAFE-01).
    fn write<T>(
        &self,
        change: impl FnOnce(&mut JournalFile, u64) -> Result<T, AppError>,
    ) -> Result<T, AppError> {
        let mut state = self.state();
        if let Some(reason) = &state.write_refused {
            return Err(write_failed(reason.clone()));
        }
        let now = self.clock.epoch_ms();
        let mut next = state.file.clone();
        let out = change(&mut next, now)?;
        next.written_ms = now;
        let bytes = serde_json::to_vec_pretty(&next)
            .map_err(|error| write_failed(format!("the journal did not serialise: {error}")))?;
        self.store
            .replace(JOURNAL_FILE, &bytes, Backup::Skip)
            .map_err(|error| write_failed(format!("{JOURNAL_FILE}: {error}")))?;
        self.open.follow(&state.file.ids(), &next.ids());
        state.file = next;
        Ok(out)
    }

    /// Writes a new change with status intent, and the monitor's baseline when this is the
    /// first open change on it, then hands out the Ticket (REQ-SAFE-01, REQ-SAFE-02). A
    /// baseline for a monitor that already has one is left out: the first one stays.
    pub fn open_change(
        &self,
        record: ChangeRecord,
        baseline: Option<MonitorRecord>,
    ) -> Result<Ticket, AppError> {
        self.open_change_with(record, baseline, None)
    }

    /// `open_change`, with the snapshot of the whole display configuration in the same write
    /// when the journal keeps none yet (REQ-SAFE-02): the first open change carries it, so
    /// Restore everything and recovery can put every monitor back from one QueryDisplayConfig
    /// read. A snapshot offered while one is kept is left out: the first one stays.
    pub fn open_change_with(
        &self,
        record: ChangeRecord,
        baseline: Option<MonitorRecord>,
        layout: Option<LayoutRecord>,
    ) -> Result<Ticket, AppError> {
        let id = record.change_id.clone();
        self.write(|file, _now| {
            if record.status != ChangeStatus::Intent {
                return Err(internal("a new change starts with status intent"));
            }
            if !is_valid_id(&id) {
                return Err(internal(
                    "the change ID is empty, too long or has other characters",
                ));
            }
            if file.has_id(&id) {
                return Err(internal("the change ID is in the journal already"));
            }
            if let Some(problem) = route_problem(&record) {
                return Err(internal(problem));
            }
            if let Some(monitor) = &record.monitor
                && file.baseline_of(monitor).is_none()
            {
                match baseline {
                    Some(baseline) if same_monitor(&baseline.identity, monitor) => {
                        file.monitors.push(baseline);
                    }
                    _ => return Err(internal("the first change on a monitor needs its baseline")),
                }
            }
            if file.layout.is_none() {
                file.layout = layout;
            }
            file.changes.push(record);
            Ok(())
        })?;
        Ok(ticket(&id))
    }

    /// Writes a verified status: applied, pending or kept. No Ticket: nothing changes the PC
    /// after this write.
    pub fn advance(&self, change_id: &str, status: ChangeStatus) -> Result<(), AppError> {
        if status.is_changing() {
            return Err(internal(
                "a changing status is written by open_change, begin_revert or begin_restore",
            ));
        }
        self.write(|file, now| move_change(file, change_id, status, now))
    }

    /// Writes status reverting before the previous state goes back, and hands out the
    /// Ticket for that call.
    pub fn begin_revert(&self, change_id: &str) -> Result<Ticket, AppError> {
        self.write(|file, now| move_change(file, change_id, ChangeStatus::Reverting, now))?;
        Ok(ticket(change_id))
    }

    /// Writes status restoring on every change in `change_ids` in one write, and hands out
    /// one Ticket per change, in the same order. An empty list writes nothing.
    pub fn begin_restore(&self, change_ids: &[&str]) -> Result<Vec<Ticket>, AppError> {
        if change_ids.is_empty() {
            return Ok(Vec::new());
        }
        self.write(|file, now| {
            for change_id in change_ids {
                move_change(file, change_id, ChangeStatus::Restoring, now)?;
            }
            Ok(())
        })?;
        Ok(change_ids.iter().map(|id| ticket(id)).collect())
    }

    /// Removes the changes in `change_ids` once their put-back verified, or recovery found
    /// them gone, in one write. A monitor's baseline goes with its last change, and the
    /// snapshot goes with the last change of all.
    pub fn close(&self, change_ids: &[&str]) -> Result<(), AppError> {
        if change_ids.is_empty() {
            return Ok(());
        }
        self.write(|file, _now| {
            let mut monitors = Vec::new();
            for change_id in change_ids {
                let index = file
                    .changes
                    .iter()
                    .position(|change| change.change_id == *change_id)
                    .ok_or_else(|| internal("no open change has this ID"))?;
                if let Some(monitor) = file.changes.remove(index).monitor {
                    monitors.push(monitor);
                }
            }
            for monitor in &monitors {
                if file.changes_on(monitor).next().is_none() {
                    file.monitors
                        .retain(|record| !same_monitor(&record.identity, monitor));
                }
            }
            if file.changes.is_empty() {
                file.layout = None;
            }
            Ok(())
        })
    }

    /// Writes the layout baseline before the first layout change. Ok(false) when one is kept
    /// already: the first one stays, and nothing is written.
    pub fn record_layout(&self, record: LayoutRecord) -> Result<bool, AppError> {
        if self.state().file.layout.is_some() {
            return Ok(false);
        }
        self.write(|file, _now| {
            if file.layout.is_some() {
                return Ok(false);
            }
            file.layout = Some(record);
            Ok(true)
        })
    }

    /// Removes the layout baseline once the layout verified back.
    pub fn clear_layout(&self) -> Result<(), AppError> {
        self.write(|file, _now| {
            file.layout = None;
            Ok(())
        })
    }

    /// Writes a new operation with status intent and hands out its Ticket.
    pub fn open_operation(&self, record: OperationRecord) -> Result<Ticket, AppError> {
        let id = record.op_id.clone();
        self.write(|file, _now| {
            if record.status != OperationStatus::Intent {
                return Err(internal("a new operation starts with status intent"));
            }
            if !is_valid_id(&id) {
                return Err(internal(
                    "the operation ID is empty, too long or has other characters",
                ));
            }
            if file.has_id(&id) {
                return Err(internal("the operation ID is in the journal already"));
            }
            file.operations.push(record);
            Ok(())
        })?;
        Ok(ticket(&id))
    }

    /// Moves an operation on. Status toolRunning is written before an elevated tool runs and
    /// gives its Ticket; done removes the record; the other statuses give no Ticket.
    pub fn advance_operation(
        &self,
        op_id: &str,
        status: OperationStatus,
    ) -> Result<Option<Ticket>, AppError> {
        self.write(|file, now| {
            let index = file
                .operations
                .iter()
                .position(|operation| operation.op_id == op_id)
                .ok_or_else(|| internal("no operation has this ID"))?;
            let operation = &mut file.operations[index];
            if !status.may_follow(operation.status) {
                return Err(internal("the operation cannot move to that status"));
            }
            if status == OperationStatus::Done {
                file.operations.remove(index);
            } else {
                operation.status = status;
                operation.updated_ms = now;
            }
            Ok(())
        })?;
        Ok((status == OperationStatus::ToolRunning).then(|| ticket(op_id)))
    }

    /// Removes an operation that recovery proved never wrote anything.
    pub fn close_operation(&self, op_id: &str) -> Result<(), AppError> {
        self.write(|file, _now| {
            let before = file.operations.len();
            file.operations.retain(|operation| operation.op_id != op_id);
            if file.operations.len() == before {
                return Err(internal("no operation has this ID"));
            }
            Ok(())
        })
    }

    /// Records a custom entry that verified as added.
    pub fn add_own_entry(&self, record: OwnEntryRecord) -> Result<(), AppError> {
        self.write(|file, _now| {
            if file
                .own_entries
                .iter()
                .any(|entry| entry.entry_id == record.entry_id)
            {
                return Err(internal("the own entry is recorded already"));
            }
            file.own_entries.push(record);
            Ok(())
        })
    }

    /// Removes a custom entry that verified as removed.
    pub fn remove_own_entry(&self, entry_id: &str) -> Result<(), AppError> {
        self.write(|file, _now| {
            let before = file.own_entries.len();
            file.own_entries.retain(|entry| entry.entry_id != entry_id);
            if file.own_entries.len() == before {
                return Err(internal("no own entry has this ID"));
            }
            Ok(())
        })
    }

    /// The whole journal as memory holds it.
    pub fn snapshot(&self) -> JournalFile {
        self.state().file.clone()
    }

    /// The change with this ID.
    pub fn change(&self, change_id: &str) -> Option<ChangeRecord> {
        self.state().file.change(change_id).cloned()
    }

    /// The open changes on `monitor`, oldest first.
    pub fn open_changes_on(&self, monitor: &MonitorId) -> Vec<ChangeRecord> {
        self.state().file.changes_on(monitor).cloned().collect()
    }

    /// The baseline of `monitor`, when ResSwap has an open change on it.
    pub fn baseline_of(&self, monitor: &MonitorId) -> Option<MonitorRecord> {
        self.state().file.baseline_of(monitor).cloned()
    }

    /// Whether anything waits to be put back; nothing open means start-up recovery reports
    /// clean (REQ-SAFE-06).
    pub fn needs_recovery(&self) -> bool {
        self.state().file.needs_recovery()
    }

    /// What is still changed, as the window lists it.
    pub fn items(&self, name_of: impl Fn(&MonitorId) -> Option<String>) -> Vec<JournalItem> {
        self.state().file.items(name_of)
    }

    /// Why writes are refused, when a damaged or newer file could not be moved aside.
    pub fn write_refused(&self) -> Option<String> {
        self.state().write_refused.clone()
    }
}

/// Moves one change to `status` when the lifecycle allows it.
fn move_change(
    file: &mut JournalFile,
    change_id: &str,
    status: ChangeStatus,
    now: u64,
) -> Result<(), AppError> {
    let change = file
        .changes
        .iter_mut()
        .find(|change| change.change_id == change_id)
        .ok_or_else(|| internal("no open change has this ID"))?;
    if !status.may_follow(change.status) {
        return Err(internal("the change cannot move to that status"));
    }
    change.status = status;
    change.updated_ms = now;
    Ok(())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::display::fake::When;
    use crate::error::ErrorCode;
    use crate::settings::atomic::WriteStage;
    use crate::settings::atomic::tests::TempFolder;
    use crate::settings::fake::{MemoryStore, StoreCall, StoreFault, StoreFile};
    use crate::system::fake::{FAKE_EPOCH_MS, FakeClock};

    const OPEN_CHANGE: &str = include_str!("../../tests/fixtures/journal-v1-open-change.json");
    const CHANGE_1: &str = "5f0e3c1a-8b2d-4e6f-9a1b-2c3d4e5f6a7b";
    const CHANGE_2: &str = "6a1f4d2b-9c3e-4f70-8b2c-3d4e5f6a7b8c";

    fn lg() -> MonitorId {
        MonitorId {
            device_path: String::new(),
            edid_manufacturer_id: Some(0x6D1E),
            edid_product_code: Some(0x5B8E),
            connector_instance: 1,
        }
    }

    fn state(width: u32, height: u32, hz: u32) -> DisplayState {
        DisplayState {
            width,
            height,
            refresh: Rational::hz(hz),
            scaling: Scaling::Identity,
            position_x: 0,
            position_y: 0,
            gdi: None,
        }
    }

    fn baseline() -> MonitorRecord {
        MonitorRecord {
            identity: lg(),
            adapter_device_path: String::from("fake-adapter-path:test-pc:rtx-4060-ti"),
            gdi_name_hint: String::from(r"\\.\DISPLAY1"),
            baseline: state(1920, 1080, 240),
            taken_ms: FAKE_EPOCH_MS,
        }
    }

    fn change(id: &str, target: DisplayState) -> ChangeRecord {
        ChangeRecord {
            change_id: id.to_owned(),
            kind: JournalItemKind::Mode,
            monitor: Some(lg()),
            previous: state(1920, 1080, 240),
            target,
            scaling_requested: ScalingChoice::FullScreen,
            route: Route::Ccd,
            gdi_flag: None,
            status: ChangeStatus::Intent,
            source: ChangeSource::Window,
            profile_id: None,
            started_ms: FAKE_EPOCH_MS,
            updated_ms: FAKE_EPOCH_MS,
        }
    }

    struct Setup {
        store: Arc<MemoryStore>,
        open: OpenEntries,
        journal: Journal,
    }

    fn setup() -> Setup {
        let store = Arc::new(MemoryStore::new());
        let open = OpenEntries::default();
        let (journal, error) =
            Journal::load(store.clone(), Arc::new(FakeClock::new()), open.clone());
        assert!(error.is_none());
        Setup {
            store,
            open,
            journal,
        }
    }

    fn on_disk(store: &MemoryStore) -> JournalFile {
        serde_json::from_slice(&store.file(JOURNAL_FILE).unwrap()).unwrap()
    }

    fn reload(store: &Arc<MemoryStore>) -> (Journal, OpenEntries, Option<AppError>) {
        let open = OpenEntries::default();
        let (journal, error) =
            Journal::load(store.clone(), Arc::new(FakeClock::new()), open.clone());
        (journal, open, error)
    }

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

    /// The Ticket comes after the write: the intent record is on disk and the entry is open
    /// for every fake when the caller gets it.
    #[test]
    fn req_safe_01_ticket_comes_from_the_write() {
        let s = setup();
        assert_eq!(s.store.file(JOURNAL_FILE), None);
        let ticket = s
            .journal
            .open_change(change(CHANGE_1, state(1280, 960, 240)), Some(baseline()))
            .unwrap();
        assert_eq!(ticket.change_id(), CHANGE_1);
        s.open.require_open(&ticket);
        let disk = on_disk(&s.store);
        assert_eq!(disk.changes[0].status, ChangeStatus::Intent);
        assert_eq!(disk.written_ms, FAKE_EPOCH_MS);
        assert_eq!(disk, s.journal.snapshot());
        assert_eq!(
            s.store.calls(),
            vec![
                StoreCall::Read(String::from(JOURNAL_FILE)),
                StoreCall::Replace {
                    name: String::from(JOURNAL_FILE),
                    backup: Backup::Skip,
                },
            ]
        );
    }

    /// INJ-D07: each stage of the write fails in turn. JOURNAL_WRITE_FAILED, no Ticket, no
    /// open entry, and memory and disk as they were.
    #[test]
    fn req_safe_01_inj_d07_failed_write_gives_no_ticket() {
        for stage in [
            WriteStage::TempWrite,
            WriteStage::Sync,
            WriteStage::BackupCopy,
            WriteStage::Rename,
        ] {
            let s = setup();
            s.journal
                .open_change(change(CHANGE_1, state(1280, 960, 240)), Some(baseline()))
                .unwrap();
            let before_disk = s.store.file(JOURNAL_FILE);
            let before = s.journal.snapshot();
            s.store
                .script(StoreFault::Write(StoreFile::Journal, stage), When::Once);
            let error = s
                .journal
                .open_change(change(CHANGE_2, state(1440, 1080, 240)), None)
                .unwrap_err();
            assert_eq!(error.code(), ErrorCode::JournalWriteFailed, "{stage:?}");
            assert!(error.to_ipc().detail.contains(stage.label()), "{stage:?}");
            assert!(!s.open.is_open(CHANGE_2), "{stage:?}");
            assert_eq!(s.journal.snapshot(), before, "{stage:?}");
            assert_eq!(s.store.file(JOURNAL_FILE), before_disk, "{stage:?}");
        }
    }

    /// The record holds the previous state with its rational and scaling, the route, and the
    /// baseline of the first change on the monitor; a second change keeps that baseline.
    #[test]
    fn req_safe_02_entry_holds_previous_state_and_baseline() {
        let s = setup();
        s.journal
            .open_change(change(CHANGE_1, state(1280, 960, 240)), Some(baseline()))
            .unwrap();
        s.journal.advance(CHANGE_1, ChangeStatus::Kept).unwrap_err();
        s.journal.advance(CHANGE_1, ChangeStatus::Applied).unwrap();
        let mut second = change(CHANGE_2, state(1440, 1080, 240));
        second.previous = state(1280, 960, 240);
        second.route = Route::Gdi;
        second.gdi_flag = Some(GdiFlag::Fullscreen);
        let mut other_baseline = baseline();
        other_baseline.baseline = state(1280, 960, 240);
        s.journal.open_change(second, Some(other_baseline)).unwrap();
        let disk = serde_json::to_value(on_disk(&s.store)).unwrap();
        assert_eq!(disk["monitors"].as_array().map(Vec::len), Some(1));
        assert_eq!(
            disk["monitors"][0]["baseline"],
            json!({
                "width": 1920, "height": 1080,
                "refresh": { "numerator": 240, "denominator": 1 },
                "scaling": "identity", "positionX": 0, "positionY": 0, "gdi": null
            })
        );
        assert_eq!(
            disk["monitors"][0]["adapterDevicePath"],
            json!("fake-adapter-path:test-pc:rtx-4060-ti")
        );
        assert_eq!(disk["changes"][1]["previous"]["width"], json!(1280));
        assert_eq!(disk["changes"][1]["route"], json!("gdi"));
        assert_eq!(disk["changes"][1]["gdiFlag"], json!("fullscreen"));
        let journal_change = s.journal.change(CHANGE_2).unwrap();
        assert_eq!(
            journal_change.route_choice(),
            RouteChoice {
                route: Route::Gdi,
                gdi_flag: Some(GdiFlag::Fullscreen),
            }
        );
        assert_eq!(s.journal.open_changes_on(&lg()).len(), 2);
        assert_eq!(
            s.journal.baseline_of(&lg()).map(|record| record.baseline),
            Some(state(1920, 1080, 240))
        );
    }

    #[test]
    fn req_safe_01_invalid_entries_are_refused_before_the_write() {
        let s = setup();
        let refused = [
            (change(CHANGE_1, state(1280, 960, 240)), None),
            (
                ChangeRecord {
                    status: ChangeStatus::Applied,
                    ..change(CHANGE_1, state(1280, 960, 240))
                },
                Some(baseline()),
            ),
            (
                ChangeRecord {
                    gdi_flag: Some(GdiFlag::Zero),
                    ..change(CHANGE_1, state(1280, 960, 240))
                },
                Some(baseline()),
            ),
            (
                ChangeRecord {
                    route: Route::Gdi,
                    ..change(CHANGE_1, state(1280, 960, 240))
                },
                Some(baseline()),
            ),
            (change("change 1", state(1280, 960, 240)), Some(baseline())),
            (change("", state(1280, 960, 240)), Some(baseline())),
        ];
        for (record, record_baseline) in refused {
            let error = s.journal.open_change(record, record_baseline).unwrap_err();
            assert_eq!(error.code(), ErrorCode::Internal);
        }
        s.journal
            .open_change(change(CHANGE_1, state(1280, 960, 240)), Some(baseline()))
            .unwrap();
        let duplicate = s
            .journal
            .open_change(change(CHANGE_1, state(1280, 960, 240)), None)
            .unwrap_err();
        assert_eq!(duplicate.code(), ErrorCode::Internal);
        assert_eq!(s.journal.snapshot().changes.len(), 1);
        // Two writes in the store: none for the refusals.
        assert_eq!(s.store.calls().len(), 2);
    }

    /// The moves of the record lifecycle, the Tickets of the changing statuses, and the
    /// baseline leaving with the last change on its monitor.
    #[test]
    fn req_safe_01_status_moves_follow_the_lifecycle() {
        let s = setup();
        s.journal
            .open_change(change(CHANGE_1, state(1280, 960, 240)), Some(baseline()))
            .unwrap();
        s.journal.advance(CHANGE_1, ChangeStatus::Pending).unwrap();
        // Pending cannot go back to pending, and a changing status needs its own call.
        assert!(s.journal.advance(CHANGE_1, ChangeStatus::Pending).is_err());
        assert!(
            s.journal
                .advance(CHANGE_1, ChangeStatus::Reverting)
                .is_err()
        );
        s.journal.advance(CHANGE_1, ChangeStatus::Kept).unwrap();
        // Kept cannot be reverted; it is restored.
        assert!(s.journal.begin_revert(CHANGE_1).is_err());
        s.journal
            .open_change(change(CHANGE_2, state(1440, 1080, 240)), None)
            .unwrap();
        let tickets = s.journal.begin_restore(&[CHANGE_1, CHANGE_2]).unwrap();
        assert_eq!(
            tickets.iter().map(Ticket::change_id).collect::<Vec<_>>(),
            vec![CHANGE_1, CHANGE_2]
        );
        assert!(
            on_disk(&s.store)
                .changes
                .iter()
                .all(|record| record.status == ChangeStatus::Restoring)
        );
        s.journal.close(&[CHANGE_2]).unwrap();
        assert!(!s.open.is_open(CHANGE_2));
        assert!(s.journal.baseline_of(&lg()).is_some());
        s.journal.close(&[CHANGE_1]).unwrap();
        assert!(!s.open.is_open(CHANGE_1));
        let disk = on_disk(&s.store);
        assert!(disk.monitors.is_empty() && disk.changes.is_empty());
        assert!(!s.journal.needs_recovery());
        assert!(s.journal.close(&[CHANGE_1]).is_err());
        assert!(s.journal.begin_restore(&[]).unwrap().is_empty());
    }

    /// A pending change reverted: the reverting write gives the Ticket; a failed status write
    /// keeps the last status on disk and in memory.
    #[test]
    fn req_safe_01_revert_write_gives_the_ticket() {
        let s = setup();
        s.journal
            .open_change(change(CHANGE_1, state(1280, 960, 240)), Some(baseline()))
            .unwrap();
        s.journal.advance(CHANGE_1, ChangeStatus::Pending).unwrap();
        s.store.script(
            StoreFault::Write(StoreFile::Journal, WriteStage::Rename),
            When::Once,
        );
        let error = s.journal.begin_revert(CHANGE_1).unwrap_err();
        assert_eq!(error.code(), ErrorCode::JournalWriteFailed);
        assert_eq!(on_disk(&s.store).changes[0].status, ChangeStatus::Pending);
        assert_eq!(
            s.journal.change(CHANGE_1).map(|record| record.status),
            Some(ChangeStatus::Pending)
        );
        let ticket = s.journal.begin_revert(CHANGE_1).unwrap();
        s.open.require_open(&ticket);
        assert_eq!(on_disk(&s.store).changes[0].status, ChangeStatus::Reverting);
        // A second try after a failed put-back writes reverting again.
        s.journal.begin_revert(CHANGE_1).unwrap();
    }

    /// Tickets from the journal pass the fakes' check while the entry is open, and stop at
    /// the first changing call after it closed.
    #[test]
    #[should_panic(expected = "has no open journal entry")]
    fn req_safe_01_closed_change_ticket_is_refused() {
        let s = setup();
        let ticket = s
            .journal
            .open_change(change(CHANGE_1, state(1280, 960, 240)), Some(baseline()))
            .unwrap();
        s.open.require_open(&ticket);
        s.journal.close(&[CHANGE_1]).unwrap();
        s.open.require_open(&ticket);
    }

    /// INJ-D18: the process ends with an entry open. A new journal on the same store finds
    /// it, opens its ID for the fakes, and reports that recovery is needed.
    #[test]
    fn req_safe_06_inj_d18_reload_keeps_open_entries() {
        for status in [
            ChangeStatus::Intent,
            ChangeStatus::Pending,
            ChangeStatus::Kept,
            ChangeStatus::Reverting,
        ] {
            let s = setup();
            s.journal
                .open_change(change(CHANGE_1, state(1280, 960, 240)), Some(baseline()))
                .unwrap();
            match status {
                ChangeStatus::Pending => s.journal.advance(CHANGE_1, status).unwrap(),
                ChangeStatus::Kept => {
                    s.journal.advance(CHANGE_1, ChangeStatus::Pending).unwrap();
                    s.journal.advance(CHANGE_1, status).unwrap();
                }
                ChangeStatus::Reverting => {
                    s.journal.begin_revert(CHANGE_1).unwrap();
                }
                _ => {}
            }
            drop(s.journal);
            let (journal, open, error) = reload(&s.store);
            assert!(error.is_none(), "{status:?}");
            assert!(open.is_open(CHANGE_1), "{status:?}");
            assert!(journal.needs_recovery(), "{status:?}");
            assert_eq!(
                journal.change(CHANGE_1).map(|record| record.status),
                Some(status)
            );
            // Recovery restores it with a Ticket from the restoring write.
            let tickets = journal.begin_restore(&[CHANGE_1]).unwrap();
            open.require_open(&tickets[0]);
        }
    }

    /// INJ-D19, damaged: the file is renamed journal.damaged-<ms>.json and kept, a new
    /// journal starts, and RECOVERY_INCOMPLETE is returned for start-up to raise.
    #[test]
    fn req_safe_06_inj_d19_damaged_journal_is_kept_aside() {
        let store = Arc::new(MemoryStore::new());
        store.put(JOURNAL_FILE, b"{ \"schemaVersion\": 1, ");
        let (journal, open, error) = reload(&store);
        assert_eq!(
            error.as_ref().map(AppError::code),
            Some(ErrorCode::RecoveryIncomplete)
        );
        let aside = format!("journal.damaged-{FAKE_EPOCH_MS}.json");
        assert_eq!(store.names(), vec![aside.clone()]);
        assert_eq!(
            store.file(&aside),
            Some(b"{ \"schemaVersion\": 1, ".to_vec())
        );
        let detail = error.map(|error| error.to_ipc().detail).unwrap_or_default();
        assert!(
            detail.contains("not valid JSON") && detail.contains(&aside),
            "{detail}"
        );
        assert_eq!(journal.snapshot(), JournalFile::empty());
        assert!(journal.write_refused().is_none());
        // The new journal writes a new file and leaves the kept one alone.
        let ticket = journal
            .open_change(change(CHANGE_1, state(1280, 960, 240)), Some(baseline()))
            .unwrap();
        open.require_open(&ticket);
        assert_eq!(on_disk(&store).changes.len(), 1);
        assert_eq!(
            store.file(&aside),
            Some(b"{ \"schemaVersion\": 1, ".to_vec())
        );
    }

    /// INJ-D19, newer: never read for recovery and never overwritten.
    #[test]
    fn req_safe_06_inj_d19_newer_journal_is_kept_aside() {
        let store = Arc::new(MemoryStore::new());
        let mut newer: Value = serde_json::from_str(OPEN_CHANGE).unwrap();
        newer["schemaVersion"] = json!(2);
        newer["laterRecords"] = json!([]);
        let newer = newer.to_string().into_bytes();
        store.put(JOURNAL_FILE, &newer);
        let (journal, open, error) = reload(&store);
        assert_eq!(
            error.as_ref().map(AppError::code),
            Some(ErrorCode::RecoveryIncomplete)
        );
        let aside = format!("journal.newer-{FAKE_EPOCH_MS}.json");
        assert_eq!(store.file(&aside), Some(newer));
        assert!(!journal.needs_recovery());
        assert!(!open.is_open(CHANGE_1));
        let detail = error.map(|error| error.to_ipc().detail).unwrap_or_default();
        assert!(detail.contains("schema version 2 is newer"), "{detail}");
    }

    /// INJ-D19 with the rename refused: the file stays where it is, and every change is
    /// refused with JOURNAL_WRITE_FAILED so a new journal never overwrites it.
    #[test]
    fn req_safe_06_inj_d19_journal_that_cannot_move_aside_blocks_changes() {
        let store = Arc::new(MemoryStore::new());
        store.put(JOURNAL_FILE, b"[]");
        store.script(StoreFault::MoveAside(StoreFile::Journal), When::Once);
        let (journal, open, error) = reload(&store);
        assert_eq!(
            error.as_ref().map(AppError::code),
            Some(ErrorCode::RecoveryIncomplete)
        );
        assert!(journal.write_refused().is_some());
        let refused = journal
            .open_change(change(CHANGE_1, state(1280, 960, 240)), Some(baseline()))
            .unwrap_err();
        assert_eq!(refused.code(), ErrorCode::JournalWriteFailed);
        assert!(!open.is_open(CHANGE_1));
        assert_eq!(store.file(JOURNAL_FILE), Some(b"[]".to_vec()));
        assert!(
            !store
                .calls()
                .iter()
                .any(|call| matches!(call, StoreCall::Replace { .. }))
        );
    }

    /// The version 1 fixture of an unfinished change loads, and its summary lists the change.
    #[test]
    fn req_safe_06_fixture_with_an_open_change_loads() {
        let store = Arc::new(MemoryStore::new());
        store.put(JOURNAL_FILE, OPEN_CHANGE.as_bytes());
        let (journal, open, error) = reload(&store);
        assert!(error.is_none(), "{error:?}");
        assert!(open.is_open(CHANGE_1));
        assert!(journal.needs_recovery());
        let file = journal.snapshot();
        assert_eq!(file.changes[0].status, ChangeStatus::Intent);
        assert_eq!(file.monitors[0].baseline, state(1920, 1080, 240));
        let items = journal
            .items(|monitor| same_monitor(monitor, &lg()).then(|| String::from("LG ULTRAGEAR")));
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].kind, JournalItemKind::Mode);
        assert_eq!(items[0].monitor_name.as_deref(), Some("LG ULTRAGEAR"));
        assert_eq!(
            items[0].mode.as_ref().map(|mode| mode.label.as_str()),
            Some("1280 x 960 at 240 Hz")
        );
        // Written again, it is the same JSON.
        let fixture: Value = serde_json::from_str(OPEN_CHANGE).unwrap();
        assert_eq!(serde_json::to_value(&file).unwrap(), fixture);
    }

    /// Every kind of damage moves the file aside: too large, an unknown field at any depth,
    /// an ID with other characters, two records with one ID, a GDI flag on the CCD route, a
    /// change with no baseline, a zero denominator, a missing version.
    #[test]
    fn req_safe_06_every_kind_of_damage_is_reported() {
        let fixture: Value = serde_json::from_str(OPEN_CHANGE).unwrap();
        let mut cases: Vec<(Vec<u8>, &str)> = Vec::new();
        let size = usize::try_from(MAX_FILE_BYTES).unwrap() + 1;
        cases.push((vec![b' '; size], "1 MiB cap"));
        for pointer in [
            "",
            "/monitors/0",
            "/monitors/0/baseline",
            "/changes/0",
            "/changes/0/target/refresh",
        ] {
            let mut changed = fixture.clone();
            changed
                .pointer_mut(pointer)
                .and_then(Value::as_object_mut)
                .unwrap()
                .insert(String::from("unknownField"), json!(1));
            cases.push((changed.to_string().into_bytes(), "journal shape"));
        }
        let mut bad_id = fixture.clone();
        bad_id["changes"][0]["changeId"] = json!("..\\change");
        cases.push((bad_id.to_string().into_bytes(), "ID is empty"));
        let mut twice = fixture.clone();
        let copy = twice["changes"][0].clone();
        twice["changes"].as_array_mut().unwrap().push(copy);
        cases.push((twice.to_string().into_bytes(), "share an ID"));
        let mut flag = fixture.clone();
        flag["changes"][0]["gdiFlag"] = json!("zero");
        cases.push((flag.to_string().into_bytes(), "GDI flag"));
        let mut orphan = fixture.clone();
        orphan["monitors"] = json!([]);
        cases.push((orphan.to_string().into_bytes(), "no baseline"));
        let mut zero = fixture.clone();
        zero["changes"][0]["target"]["refresh"]["denominator"] = json!(0);
        cases.push((zero.to_string().into_bytes(), "denominator 0"));
        let mut no_version = fixture.clone();
        no_version.as_object_mut().unwrap().remove("schemaVersion");
        cases.push((
            no_version.to_string().into_bytes(),
            "schema version is missing",
        ));
        for (content, expected) in cases {
            let store = Arc::new(MemoryStore::new());
            store.put(JOURNAL_FILE, &content);
            let (journal, _, error) = reload(&store);
            let detail = error.map(|error| error.to_ipc().detail).unwrap_or_default();
            assert!(detail.contains(expected), "{expected}: {detail}");
            assert_eq!(store.file(JOURNAL_FILE), None, "{expected}");
            assert!(!journal.needs_recovery(), "{expected}");
        }
    }

    /// The window's summary: one item per open change with its target mode and monitor
    /// name, the layout once, and one item per operation in flight; own entries alone need
    /// no recovery.
    #[test]
    fn req_safe_01_items_summarise_what_is_still_changed() {
        let s = setup();
        assert!(s.journal.items(|_| None).is_empty());
        s.journal
            .open_change(change(CHANGE_1, state(1280, 960, 240)), Some(baseline()))
            .unwrap();
        let layout = LayoutRecord {
            virtual_mode_aware: false,
            adapters: Vec::new(),
            paths: Vec::new(),
            modes: Vec::new(),
            targets: vec![lg()],
            taken_ms: FAKE_EPOCH_MS,
        };
        assert!(s.journal.record_layout(layout.clone()).unwrap());
        let mut later = layout.clone();
        later.taken_ms += 1;
        assert!(!s.journal.record_layout(later).unwrap());
        assert_eq!(s.journal.snapshot().layout, Some(layout.clone()));
        // While a change is open, the snapshot is its baseline, not an item of its own.
        let items = s.journal.items(|_| Some(String::from("LG ULTRAGEAR")));
        assert_eq!(
            items.iter().map(|item| item.kind).collect::<Vec<_>>(),
            vec![JournalItemKind::Mode]
        );
        assert_eq!(items[0].monitor_name.as_deref(), Some("LG ULTRAGEAR"));
        // The last change takes the snapshot with it.
        s.journal.close(&[CHANGE_1]).unwrap();
        assert_eq!(s.journal.snapshot().layout, None);
        assert!(!s.journal.needs_recovery());
        // A snapshot left alone is listed as the layout.
        assert!(s.journal.record_layout(layout).unwrap());
        let items = s.journal.items(|_| None);
        assert_eq!(
            items.iter().map(|item| item.kind).collect::<Vec<_>>(),
            vec![JournalItemKind::Layout]
        );
        s.journal.clear_layout().unwrap();
        assert!(!s.journal.needs_recovery());
    }

    fn operation(id: &str) -> OperationRecord {
        OperationRecord {
            op_id: id.to_owned(),
            kind: OperationKind::NvModesWrite,
            adapter_instance_id: String::from("PCI\\VEN_10DE&DEV_2803&SYNTHETIC"),
            adapter_key: String::from("0001"),
            value_type: String::from("REG_MULTI_SZ"),
            before_value: vec![String::from("before")],
            expected_value: vec![String::from("after")],
            backup_file: Some(String::from("nv_modes-1794355200000.json")),
            status: OperationStatus::Intent,
            started_ms: FAKE_EPOCH_MS,
            updated_ms: FAKE_EPOCH_MS,
        }
    }

    /// Operations: the intent and toolRunning writes give Tickets, done removes the record;
    /// own entries are recorded and removed without counting as something to recover.
    #[test]
    fn req_safe_01_operations_and_own_entries() {
        let s = setup();
        let ticket = s.journal.open_operation(operation("op-1")).unwrap();
        s.open.require_open(&ticket);
        assert!(s.journal.needs_recovery());
        assert_eq!(
            s.journal.items(|_| None).first().map(|item| item.kind),
            Some(JournalItemKind::NvModes)
        );
        assert!(
            s.journal
                .advance_operation("op-1", OperationStatus::Written)
                .is_err()
        );
        let running = s
            .journal
            .advance_operation("op-1", OperationStatus::ToolRunning)
            .unwrap();
        assert_eq!(running.as_ref().map(Ticket::change_id), Some("op-1"));
        assert!(
            s.journal
                .advance_operation("op-1", OperationStatus::Written)
                .unwrap()
                .is_none()
        );
        s.journal
            .advance_operation("op-1", OperationStatus::RestartDue)
            .unwrap();
        s.journal
            .advance_operation("op-1", OperationStatus::Done)
            .unwrap();
        assert!(!s.open.is_open("op-1"));
        assert!(on_disk(&s.store).operations.is_empty());
        s.journal.open_operation(operation("op-2")).unwrap();
        s.journal.close_operation("op-2").unwrap();
        assert!(s.journal.close_operation("op-2").is_err());

        let entry = OwnEntryRecord {
            entry_id: String::from("nvmodes-1280x960"),
            source: CustomSource::NvModes,
            width: 1280,
            height: 960,
            raw_group: Some(String::from("1280x960x8,16,32,64=1F;")),
            adapter_instance_id: String::from("PCI\\VEN_10DE&DEV_2803&SYNTHETIC"),
            added_ms: FAKE_EPOCH_MS,
            backup_file: None,
        };
        s.journal.add_own_entry(entry.clone()).unwrap();
        assert!(s.journal.add_own_entry(entry).is_err());
        assert!(!s.journal.needs_recovery());
        assert_eq!(on_disk(&s.store).own_entries.len(), 1);
        s.journal.remove_own_entry("nvmodes-1280x960").unwrap();
        assert!(s.journal.remove_own_entry("nvmodes-1280x960").is_err());
    }

    #[test]
    fn req_safe_02_states_come_from_a_fresh_read() {
        let raw = RawMonitor {
            id: lg(),
            name: Some(String::from("LG ULTRAGEAR")),
            adapter_name: String::from("NVIDIA GeForce RTX 4060 Ti"),
            adapter_instance_id: String::from("PCI\\VEN_10DE&DEV_2803&SYNTHETIC"),
            adapter_device_path: String::from("fake-adapter-path:test-pc:rtx-4060-ti"),
            gdi_name: String::from(r"\\.\DISPLAY1"),
            position_x: -1920,
            position_y: 0,
            output_technology: 10,
            primary: true,
            active: true,
            native: None,
            current: Some(RawMode {
                width: 1920,
                height: 1080,
                refresh: Rational {
                    numerator: 239_760,
                    denominator: 1000,
                },
            }),
            scaling: Scaling::AspectRatio,
            modes: Vec::new(),
        };
        let record = MonitorRecord::baseline_of(&raw, FAKE_EPOCH_MS).unwrap();
        assert_eq!(record.identity, lg());
        assert_eq!(record.gdi_name_hint, r"\\.\DISPLAY1");
        assert_eq!(record.baseline.position_x, -1920);
        assert_eq!(record.baseline.scaling, Scaling::AspectRatio);
        assert!(record.baseline.shows(&RawMode {
            width: 1920,
            height: 1080,
            refresh: Rational {
                numerator: 29_970,
                denominator: 125,
            },
        }));
        assert!(!record.baseline.shows(&RawMode {
            width: 1920,
            height: 1080,
            refresh: Rational::hz(240),
        }));
        let inactive = RawMonitor {
            current: None,
            ..raw
        };
        assert!(MonitorRecord::baseline_of(&inactive, FAKE_EPOCH_MS).is_none());
    }

    #[test]
    fn req_safe_01_disk_round_trip() {
        let folder = TempFolder::new("journal");
        let clock: Arc<dyn Clock> = Arc::new(FakeClock::new());
        let open = OpenEntries::default();
        let (journal, error) = Journal::in_folder(folder.path(), clock.clone(), open.clone());
        assert!(error.is_none());
        journal
            .open_change(change(CHANGE_1, state(1280, 960, 240)), Some(baseline()))
            .unwrap();
        journal.advance(CHANGE_1, ChangeStatus::Applied).unwrap();
        assert!(folder.file("journal.json.tmp").is_none());
        assert!(folder.file("journal.json.bak").is_none());
        let reopened = OpenEntries::default();
        let (again, error) = Journal::in_folder(folder.path(), clock, reopened.clone());
        assert!(error.is_none());
        assert_eq!(again.snapshot(), journal.snapshot());
        assert!(reopened.is_open(CHANGE_1));
    }

    /// The first change carries the snapshot in its own write; a later change keeps the
    /// first snapshot, and the last close takes it away (REQ-SAFE-02).
    #[test]
    fn req_safe_02_first_change_carries_the_snapshot() {
        let s = setup();
        let layout = |taken_ms| LayoutRecord {
            virtual_mode_aware: false,
            adapters: Vec::new(),
            paths: Vec::new(),
            modes: Vec::new(),
            targets: vec![lg()],
            taken_ms,
        };
        s.journal
            .open_change_with(
                change(CHANGE_1, state(1280, 960, 240)),
                Some(baseline()),
                Some(layout(1)),
            )
            .unwrap();
        assert_eq!(on_disk(&s.store).layout, Some(layout(1)));
        s.journal
            .open_change_with(
                change(CHANGE_2, state(1024, 768, 240)),
                None,
                Some(layout(2)),
            )
            .unwrap();
        assert_eq!(on_disk(&s.store).layout, Some(layout(1)));
        s.journal.close(&[CHANGE_1]).unwrap();
        assert_eq!(on_disk(&s.store).layout, Some(layout(1)));
        s.journal.close(&[CHANGE_2]).unwrap();
        assert_eq!(on_disk(&s.store).layout, None);
    }

    /// A pending change that another mode replaced moves to applied: open, never fought.
    #[test]
    fn req_disp_23_pending_change_can_move_to_applied() {
        let s = setup();
        s.journal
            .open_change(change(CHANGE_1, state(1280, 960, 240)), Some(baseline()))
            .unwrap();
        s.journal.advance(CHANGE_1, ChangeStatus::Pending).unwrap();
        s.journal.advance(CHANGE_1, ChangeStatus::Applied).unwrap();
        assert_eq!(on_disk(&s.store).changes[0].status, ChangeStatus::Applied);
        assert!(s.open.is_open(CHANGE_1));
        // Applied cannot go back to pending.
        assert!(s.journal.advance(CHANGE_1, ChangeStatus::Pending).is_err());
    }
}
