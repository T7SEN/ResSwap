//! The fake display backend: fixture monitors in memory, a call log and scripted faults.
//! No OS call, so every test and every fake-backend run leaves the PC as it is
//! (REQ-DISP-01). The fixture profiles, the fault script and the call log here are shared by
//! every other fake.

#![forbid(unsafe_code)]

use std::sync::{Mutex, MutexGuard, PoisonError};

use crate::apply::journal::{
    AdapterRecord, LayoutRecord, LuidRecord, ModeInfoRecord, ModeRecord, PathRecord,
    PathSourceRecord, PathTargetRecord, PointRecord, RegionRecord, SourceModeRecord,
    TargetModeRecord,
};
use crate::apply::{OpenEntries, Ticket};
use crate::display::modes::{same_mode, scaling_from_raw, scaling_to_raw};
use crate::display::monitors::match_monitor;
use crate::display::types::{MonitorId, Scaling, ScalingChoice};
use crate::display::{
    DisplayBackend, DisplayConfig, GdiFlag, Rational, RawMode, RawMonitor, Route, RouteChoice,
    SnapshotRestore,
};
use crate::error::{AppError, ErrorContext, Failure};

// The plain numbers the fake's snapshot records use, copied from windows 0.62.2
// (Win32::Devices::Display and Win32::Graphics::Gdi), because a fake holds no Win32 type.
/// DISPLAYCONFIG_MODE_INFO_TYPE_SOURCE.
const MODE_INFO_SOURCE: i32 = 1;
/// DISPLAYCONFIG_MODE_INFO_TYPE_TARGET.
const MODE_INFO_TARGET: i32 = 2;
/// DISPLAYCONFIG_PIXELFORMAT_32BPP.
const PIXEL_FORMAT_32BPP: i32 = 4;
/// DISPLAYCONFIG_ROTATION_IDENTITY.
const ROTATION_IDENTITY: i32 = 1;
/// DISPLAYCONFIG_SCANLINE_ORDERING_PROGRESSIVE.
const SCAN_LINE_PROGRESSIVE: i32 = 1;
/// DISPLAYCONFIG_PATH_ACTIVE.
const PATH_ACTIVE: u32 = 1;

/// The fixture a fake starts from. A debug run picks one with RESSWAP_FAKE_PROFILE.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FixtureProfile {
    /// T7SEN's PC as recorded on 11 Nov 2026: the LG on the RTX 4060 Ti, the Parsec virtual
    /// display, and the Samsung known to Windows but not connected.
    #[default]
    TestPc,
    /// One monitor on an adapter that is not NVIDIA.
    NotNvidia,
    /// The test PC with the Samsung connected to the right of the LG.
    TwoMonitors,
}

impl FixtureProfile {
    /// The profile named by `name`, as RESSWAP_FAKE_PROFILE spells it.
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "test-pc" => Some(Self::TestPc),
            "not-nvidia" => Some(Self::NotNvidia),
            "two-monitors" => Some(Self::TwoMonitors),
            _ => None,
        }
    }
}

/// When a scripted fault fires.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum When {
    /// On the next call of its operation, then never again.
    Once,
    /// On every call of its operation.
    Always,
    /// On the nth call of its operation, counted from 1.
    Nth(u32),
}

struct Scripted<F> {
    fault: F,
    when: When,
    calls_seen: u32,
}

/// The faults a test scripts into one fake, each with when it fires. Every fake keeps one,
/// so a test scripts each of them the same way.
pub struct FaultScript<F> {
    entries: Mutex<Vec<Scripted<F>>>,
}

impl<F> Default for FaultScript<F> {
    fn default() -> Self {
        Self {
            entries: Mutex::new(Vec::new()),
        }
    }
}

impl<F: Copy> FaultScript<F> {
    /// Adds a fault to the script.
    pub fn push(&self, fault: F, when: When) {
        lock(&self.entries).push(Scripted {
            fault,
            when,
            calls_seen: 0,
        });
    }

    /// The fault that fires on this call of one operation, if any. `belongs` picks the
    /// faults of that operation; each of them counts the call, and the first one due fires.
    pub fn due(&self, belongs: impl Fn(F) -> bool) -> Option<F> {
        let mut entries = lock(&self.entries);
        let mut fired = None;
        let mut spent = None;
        for (index, scripted) in entries.iter_mut().enumerate() {
            if !belongs(scripted.fault) {
                continue;
            }
            scripted.calls_seen += 1;
            let fires = match scripted.when {
                When::Once | When::Always => true,
                When::Nth(n) => scripted.calls_seen == n,
            };
            if fires && fired.is_none() {
                fired = Some(scripted.fault);
                if scripted.when == When::Once {
                    spent = Some(index);
                }
            }
        }
        if let Some(index) = spent {
            entries.remove(index);
        }
        fired
    }
}

/// The calls one fake received, in order. Tests assert on it.
pub struct CallLog<C> {
    calls: Mutex<Vec<C>>,
}

impl<C> Default for CallLog<C> {
    fn default() -> Self {
        Self {
            calls: Mutex::new(Vec::new()),
        }
    }
}

impl<C: Clone> CallLog<C> {
    /// Appends one call.
    pub fn record(&self, call: C) {
        lock(&self.calls).push(call);
    }

    /// Every call so far, in order.
    pub fn calls(&self) -> Vec<C> {
        lock(&self.calls).clone()
    }

    /// Whether any call so far passes `test`.
    pub fn any(&self, test: impl Fn(&C) -> bool) -> bool {
        lock(&self.calls).iter().any(test)
    }
}

/// A failure a test scripts into the fake. At most one fault fires per call: the first one
/// due for that operation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DisplayFault {
    /// Enumeration fails (INJ-D01): INTERNAL.
    EnumerateFails,
    /// Enumeration panics, as a bug inside a command would.
    EnumeratePanics,
    /// The test call refuses the configuration with this DISP_CHANGE value or
    /// SetDisplayConfig error (INJ-D06): MODE_TEST_FAILED.
    TestRefuses(i32),
    /// The configuration's monitors are unplugged just before the test call, which then
    /// finds them gone: MONITOR_GONE (INJ-D02 inside one command).
    UnplugBeforeTest,
    /// The apply call fails with this value (INJ-D08): MODE_REJECTED. Scripted for a later
    /// call, it is the failed put-back of INJ-D13 or the failed snapshot restore of INJ-D20.
    ApplyFails(i32),
    /// The apply call reports success but changes nothing (INJ-D10).
    ApplyChangesNothing,
    /// The apply call reports success but sets this size at the requested rate (INJ-D09).
    ApplySetsOtherSize { width: u32, height: u32 },
    /// The apply call reports success but sets the requested size at this rate (INJ-D11).
    ApplySetsOtherRate(Rational),
    /// The apply call sets the mode, but the path scaling reads back as this value when the
    /// source and target sizes differ: the driver ignored the request (INJ-D12).
    ApplyIgnoresScaling(Scaling),
    /// The configuration's monitors are unplugged just before the apply call, which then
    /// finds them gone and changes nothing: MONITOR_GONE.
    UnplugBeforeApply,
    /// The apply call sets the mode, then the monitors are unplugged, so the read-back
    /// finds them gone.
    UnplugAfterApply,
    /// The SDC_USE_DATABASE_CURRENT call fails with this SetDisplayConfig value (INJ-D20):
    /// MODE_REJECTED.
    DatabaseCurrentFails(i32),
    /// The SDC_USE_DATABASE_CURRENT call reports success but changes nothing (INJ-D20).
    DatabaseCurrentChangesNothing,
    /// The snapshot read fails: INTERNAL, and nothing is written.
    SnapshotFails,
    /// The snapshot restore fails with this SetDisplayConfig value (INJ-D20): MODE_REJECTED.
    RestoreSnapshotFails(i32),
    /// The snapshot restore reports success but changes nothing (INJ-D20).
    RestoreSnapshotChangesNothing,
}

impl DisplayFault {
    fn op(self) -> Op {
        match self {
            Self::EnumerateFails | Self::EnumeratePanics => Op::Enumerate,
            Self::TestRefuses(_) | Self::UnplugBeforeTest => Op::Test,
            Self::ApplyFails(_)
            | Self::ApplyChangesNothing
            | Self::ApplySetsOtherSize { .. }
            | Self::ApplySetsOtherRate(_)
            | Self::ApplyIgnoresScaling(_)
            | Self::UnplugBeforeApply
            | Self::UnplugAfterApply => Op::Apply,
            Self::DatabaseCurrentFails(_) | Self::DatabaseCurrentChangesNothing => {
                Op::DatabaseCurrent
            }
            Self::SnapshotFails => Op::Snapshot,
            Self::RestoreSnapshotFails(_) | Self::RestoreSnapshotChangesNothing => {
                Op::RestoreSnapshot
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Op {
    Enumerate,
    Test,
    Apply,
    DatabaseCurrent,
    Snapshot,
    RestoreSnapshot,
}

/// One call the fake received, in order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DisplayCall {
    Enumerate,
    /// The test call: CDS_TEST on the GDI route, SDC_VALIDATE on the CCD route.
    Test {
        route: RouteChoice,
        config: DisplayConfig,
    },
    /// A changing call with the route and GDI flag it used, so a test can prove that every
    /// change is temporary (REQ-DISP-14) and that no flag-0 call cancels an open
    /// CDS_FULLSCREEN change (REQ-DISP-15).
    Apply {
        change_id: String,
        route: RouteChoice,
        config: DisplayConfig,
    },
    /// SetDisplayConfig with SDC_APPLY and SDC_USE_DATABASE_CURRENT.
    ApplyDatabaseCurrent {
        change_id: String,
    },
    /// The snapshot read.
    Snapshot,
    /// A saved snapshot applied with SDC_APPLY and SDC_USE_SUPPLIED_DISPLAY_CONFIG: the
    /// targets it set, and how many saved adapter LUIDs it had to map to new ones.
    RestoreSnapshot {
        change_id: String,
        targets: Vec<MonitorId>,
        remapped: usize,
    },
}

/// How a change made outside ResSwap lands, for the database-current call.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OutsideChange {
    /// As a game or another program makes it: temporary, so the configuration Windows keeps
    /// for the monitor stays as it was.
    Temporary,
    /// As Windows display settings make it: the configuration Windows keeps follows it.
    Saved,
}

/// One target the fixture knows. An unavailable one is known to Windows but not connected,
/// so enumeration leaves it out.
#[derive(Clone, Debug)]
struct FakeTarget {
    monitor: RawMonitor,
    available: bool,
    /// The mode the database-current call puts back: the configuration Windows keeps.
    stored_mode: Option<RawMode>,
    stored_scaling: Scaling,
}

/// The display backend on fixture monitors, for every test and every fake-backend run.
pub struct FakeDisplay {
    journal: OpenEntries,
    route: Mutex<RouteChoice>,
    targets: Mutex<Vec<FakeTarget>>,
    log: CallLog<DisplayCall>,
    script: FaultScript<DisplayFault>,
    /// Change IDs applied with CDS_FULLSCREEN.
    fullscreen_changes: Mutex<Vec<String>>,
    /// Change IDs of flag-0 applies made while a CDS_FULLSCREEN change was open.
    cancelled_fullscreen: Mutex<Vec<String>>,
    /// The high part of every adapter LUID this boot; `change_adapter_luids` moves it, as a
    /// Windows restart would (INJ-D21).
    luid_boot: Mutex<i32>,
}

/// A fake's data stays valid after a panic in a test, so a poisoned lock is recovered.
pub(crate) fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

impl FakeDisplay {
    /// The fake for `profile`, checking every Ticket against `journal`.
    pub fn new(profile: FixtureProfile, journal: OpenEntries) -> Self {
        Self {
            journal,
            // GDI with CDS_FULLSCREEN, the recommendation of the setup research, until the
            // HW-P1 round picks the route; a test picks another with set_route.
            route: Mutex::new(RouteChoice::gdi(GdiFlag::Fullscreen)),
            targets: Mutex::new(fixture(profile)),
            log: CallLog::default(),
            script: FaultScript::default(),
            fullscreen_changes: Mutex::new(Vec::new()),
            cancelled_fullscreen: Mutex::new(Vec::new()),
            luid_boot: Mutex::new(0),
        }
    }

    /// The route and flag `route()` reports from now on. A flag that does not fit the route
    /// is a bug in the test, so it panics.
    pub fn set_route(&self, route: RouteChoice) {
        assert!(
            route.is_consistent(),
            "route and GDI flag do not fit: {route:?}"
        );
        *lock(&self.route) = route;
    }

    /// Adds a fault to the script.
    pub fn script(&self, fault: DisplayFault, when: When) {
        self.script.push(fault, when);
    }

    /// Every call so far, in order.
    pub fn calls(&self) -> Vec<DisplayCall> {
        self.log.calls()
    }

    /// Whether any call so far changed the fake PC.
    pub fn applied_anything(&self) -> bool {
        self.log.any(|call| {
            matches!(
                call,
                DisplayCall::Apply { .. }
                    | DisplayCall::ApplyDatabaseCurrent { .. }
                    | DisplayCall::RestoreSnapshot { .. }
            )
        })
    }

    /// The change IDs of every flag-0 GDI apply made while a change applied with
    /// CDS_FULLSCREEN still had an open journal entry. Such a call cancels the restore on
    /// exit, so this stays empty in every passing pipeline test (REQ-DISP-15).
    pub fn flag_zero_while_fullscreen_open(&self) -> Vec<String> {
        lock(&self.cancelled_fullscreen).clone()
    }

    /// The live state of a fixture monitor, connected or not.
    pub fn monitor(&self, id: &MonitorId) -> Option<RawMonitor> {
        lock(&self.targets)
            .iter()
            .find(|target| target.monitor.id == *id)
            .map(|target| target.monitor.clone())
    }

    /// Takes a monitor away as an unplug, or a path whose targetAvailable turned false,
    /// would: enumeration leaves it out and every call naming it finds it gone (INJ-D02,
    /// INJ-D03, REQ-SAFE-09). Its state is kept for `plug`. False when no connected monitor
    /// has this identity.
    pub fn unplug(&self, id: &MonitorId) -> bool {
        let mut targets = lock(&self.targets);
        let Some(target) = targets
            .iter_mut()
            .find(|target| target.available && target.monitor.id == *id)
        else {
            return false;
        };
        target.available = false;
        true
    }

    /// Connects a known monitor again. It comes back active with the mode it had, or with
    /// its preferred mode when it had none, such as the Samsung on the test PC. False when
    /// no unplugged monitor has this identity.
    pub fn plug(&self, id: &MonitorId) -> bool {
        let mut targets = lock(&self.targets);
        let Some(target) = targets
            .iter_mut()
            .find(|target| !target.available && target.monitor.id == *id)
        else {
            return false;
        };
        target.available = true;
        target.monitor.active = true;
        if target.monitor.current.is_none() {
            target.monitor.current = target.monitor.native;
        }
        if target.stored_mode.is_none() {
            target.stored_mode = target.monitor.current;
        }
        true
    }

    /// Changes a connected monitor's mode and scaling as Windows or a game would, with no
    /// call into the fake, so the call log stays as it is (WM_DISPLAYCHANGE and recovery
    /// tests). False when no connected monitor has this identity.
    pub fn change_from_outside(
        &self,
        id: &MonitorId,
        mode: RawMode,
        scaling: Scaling,
        how: OutsideChange,
    ) -> bool {
        let mut targets = lock(&self.targets);
        let Some(target) = targets
            .iter_mut()
            .find(|target| target.available && target.monitor.id == *id)
        else {
            return false;
        };
        target.monitor.current = Some(mode);
        target.monitor.scaling = scaling;
        if how == OutsideChange::Saved {
            target.stored_mode = Some(mode);
            target.stored_scaling = scaling;
        }
        true
    }

    /// Edits a fixture monitor's live state, connected or not, for a case the other helpers
    /// do not cover, such as a new entry in its mode list. False when the fixture has no
    /// monitor with this identity.
    pub fn edit_monitor(&self, id: &MonitorId, edit: impl FnOnce(&mut RawMonitor)) -> bool {
        let mut targets = lock(&self.targets);
        let Some(target) = targets.iter_mut().find(|target| target.monitor.id == *id) else {
            return false;
        };
        edit(&mut target.monitor);
        true
    }

    /// Gives every adapter a new LUID, as a Windows restart may: a snapshot taken before keeps
    /// the old numbers and must be mapped by adapter device path (INJ-D21).
    pub fn change_adapter_luids(&self) {
        let mut boot = lock(&self.luid_boot);
        *boot = boot.saturating_add(1);
    }

    /// The LUID an adapter has this boot: its place among the fixture's adapters, plus the
    /// boot number. Synthetic.
    fn luid_of(&self, targets: &[FakeTarget], adapter_device_path: &str) -> LuidRecord {
        let mut paths: Vec<&str> = Vec::new();
        for target in targets {
            let path = target.monitor.adapter_device_path.as_str();
            if !paths.contains(&path) {
                paths.push(path);
            }
        }
        let place = paths
            .iter()
            .position(|known| *known == adapter_device_path)
            .unwrap_or(paths.len());
        LuidRecord {
            low_part: u32::try_from(place).unwrap_or(u32::MAX).saturating_add(1),
            high_part: *lock(&self.luid_boot),
        }
    }

    fn record(&self, call: DisplayCall) {
        self.log.record(call);
    }

    /// The fault that fires on this call of `op`, if any.
    fn due(&self, op: Op) -> Option<DisplayFault> {
        self.script.due(|fault| fault.op() == op)
    }

    /// Keeps the bookkeeping for REQ-DISP-15: which changes hold a CDS_FULLSCREEN mode, and
    /// which flag-0 calls came while one of them was still open in the journal.
    fn note_flag(&self, change_id: &str, route: RouteChoice) {
        match (route.route, route.gdi_flag) {
            (Route::Gdi, Some(GdiFlag::Fullscreen)) => {
                let mut changes = lock(&self.fullscreen_changes);
                if !changes.iter().any(|known| known == change_id) {
                    changes.push(change_id.to_owned());
                }
            }
            (Route::Gdi, Some(GdiFlag::Zero)) => {
                let open = lock(&self.fullscreen_changes)
                    .iter()
                    .any(|known| self.journal.is_open(known));
                if open {
                    lock(&self.cancelled_fullscreen).push(change_id.to_owned());
                }
            }
            _ => {}
        }
    }

    /// Marks every monitor of `config` as unplugged.
    fn unplug_targets(&self, config: &DisplayConfig) {
        for target in &config.targets {
            self.unplug(&target.monitor);
        }
    }
}

impl DisplayBackend for FakeDisplay {
    fn enumerate(&self) -> Result<Vec<RawMonitor>, AppError> {
        self.record(DisplayCall::Enumerate);
        match self.due(Op::Enumerate) {
            Some(DisplayFault::EnumeratePanics) => {
                panic!("scripted panic in the fake display's enumerate")
            }
            Some(_) => {
                return Err(AppError::internal(
                    "scripted fault: the fake display's enumeration failed",
                ));
            }
            None => {}
        }
        Ok(lock(&self.targets)
            .iter()
            .filter(|target| target.available)
            .map(|target| target.monitor.clone())
            .collect())
    }

    fn route(&self) -> RouteChoice {
        *lock(&self.route)
    }

    fn test_config(&self, config: &DisplayConfig) -> Result<(), AppError> {
        self.record(DisplayCall::Test {
            route: self.route(),
            config: config.clone(),
        });
        match self.due(Op::Test) {
            Some(DisplayFault::TestRefuses(status)) => {
                return Err(AppError::ModeTestFailed(Box::new(Failure::new(format!(
                    "scripted fault: the test call returned {status}"
                )))));
            }
            Some(_) => self.unplug_targets(config),
            None => {}
        }
        let targets = lock(&self.targets);
        for target in &config.targets {
            let Some(found) = find(&targets, &target.monitor) else {
                return Err(gone());
            };
            let listed = found
                .monitor
                .modes
                .iter()
                .any(|mode| same_mode(*mode, target.mode));
            if !listed {
                let context = ErrorContext {
                    monitor: found.monitor.name.clone(),
                    ..ErrorContext::default()
                };
                return Err(AppError::ModeTestFailed(Box::new(
                    Failure::new("the fake refuses a mode its list does not hold")
                        .with_context(context),
                )));
            }
        }
        Ok(())
    }

    fn apply_config(&self, ticket: &Ticket, config: &DisplayConfig) -> Result<(), AppError> {
        // A change with no open journal entry is a pipeline bug (pc_changes_need_ticket).
        self.journal.require_open(ticket);
        let route = self.route();
        self.note_flag(ticket.change_id(), route);
        self.record(DisplayCall::Apply {
            change_id: ticket.change_id().to_owned(),
            route,
            config: config.clone(),
        });
        let fault = self.due(Op::Apply);
        match fault {
            Some(DisplayFault::ApplyFails(status)) => {
                return Err(AppError::ModeRejected(Box::new(Failure::new(format!(
                    "scripted fault: the apply call returned {status}"
                )))));
            }
            Some(DisplayFault::ApplyChangesNothing) => return Ok(()),
            Some(DisplayFault::UnplugBeforeApply) => self.unplug_targets(config),
            _ => {}
        }
        let mut targets = lock(&self.targets);
        // Every monitor is checked first, so a configuration naming a gone monitor changes
        // nothing at all.
        if config
            .targets
            .iter()
            .any(|target| find(&targets, &target.monitor).is_none())
        {
            return Err(gone());
        }
        for target in &config.targets {
            let Some(found) = targets
                .iter_mut()
                .find(|known| known.available && known.monitor.id == target.monitor)
            else {
                continue;
            };
            let set = match fault {
                Some(DisplayFault::ApplySetsOtherSize { width, height }) => RawMode {
                    width,
                    height,
                    refresh: target.mode.refresh,
                },
                Some(DisplayFault::ApplySetsOtherRate(refresh)) => RawMode {
                    refresh,
                    ..target.mode
                },
                _ => target.mode,
            };
            found.monitor.scaling = scaling_after(&found.monitor, set, target.scaling, fault);
            found.monitor.current = Some(set);
            if fault == Some(DisplayFault::UnplugAfterApply) {
                found.available = false;
            }
        }
        Ok(())
    }

    fn apply_database_current(&self, ticket: &Ticket) -> Result<(), AppError> {
        self.journal.require_open(ticket);
        self.record(DisplayCall::ApplyDatabaseCurrent {
            change_id: ticket.change_id().to_owned(),
        });
        match self.due(Op::DatabaseCurrent) {
            Some(DisplayFault::DatabaseCurrentFails(status)) => {
                return Err(AppError::ModeRejected(Box::new(Failure::new(format!(
                    "scripted fault: SetDisplayConfig with SDC_USE_DATABASE_CURRENT returned \
                     {status}"
                )))));
            }
            Some(_) => return Ok(()),
            None => {}
        }
        // Every connected monitor goes back to the configuration Windows keeps for it.
        let mut targets = lock(&self.targets);
        for target in targets.iter_mut().filter(|known| known.available) {
            target.monitor.current = target.stored_mode;
            target.monitor.scaling = target.stored_scaling;
        }
        Ok(())
    }

    fn snapshot(&self) -> Result<LayoutRecord, AppError> {
        self.record(DisplayCall::Snapshot);
        if self.due(Op::Snapshot).is_some() {
            return Err(AppError::internal(
                "scripted fault: the fake display's snapshot read failed",
            ));
        }
        let targets = lock(&self.targets);
        let mut record = LayoutRecord {
            virtual_mode_aware: false,
            adapters: Vec::new(),
            paths: Vec::new(),
            modes: Vec::new(),
            targets: Vec::new(),
            taken_ms: 0,
        };
        for (index, target) in targets.iter().filter(|known| known.available).enumerate() {
            let monitor = &target.monitor;
            let Some(current) = monitor.current else {
                continue;
            };
            let luid = self.luid_of(&targets, &monitor.adapter_device_path);
            if !record
                .adapters
                .iter()
                .any(|adapter| adapter.adapter_device_path == monitor.adapter_device_path)
            {
                record.adapters.push(AdapterRecord {
                    luid,
                    adapter_device_path: monitor.adapter_device_path.clone(),
                });
            }
            let id = u32::try_from(index).unwrap_or(u32::MAX);
            let source_index = u32::try_from(record.modes.len()).unwrap_or(u32::MAX);
            record.modes.push(ModeRecord {
                info_type: MODE_INFO_SOURCE,
                id,
                adapter_id: luid,
                info: ModeInfoRecord::Source(SourceModeRecord {
                    width: current.width,
                    height: current.height,
                    pixel_format: PIXEL_FORMAT_32BPP,
                    position: PointRecord {
                        x: monitor.position_x,
                        y: monitor.position_y,
                    },
                }),
            });
            // Synthetic timing: the native size as the active size, at the current rate.
            let timing = monitor.native.unwrap_or(current);
            let target_index = u32::try_from(record.modes.len()).unwrap_or(u32::MAX);
            record.modes.push(ModeRecord {
                info_type: MODE_INFO_TARGET,
                id,
                adapter_id: luid,
                info: ModeInfoRecord::Target(TargetModeRecord {
                    pixel_rate: 0,
                    h_sync_freq: Rational::hz(0),
                    v_sync_freq: current.refresh,
                    active_size: RegionRecord {
                        cx: timing.width,
                        cy: timing.height,
                    },
                    total_size: RegionRecord {
                        cx: timing.width,
                        cy: timing.height,
                    },
                    video_standard: 0,
                    scan_line_ordering: SCAN_LINE_PROGRESSIVE,
                }),
            });
            record.paths.push(PathRecord {
                source_info: PathSourceRecord {
                    adapter_id: luid,
                    id,
                    mode_info_idx: source_index,
                    status_flags: 0,
                },
                target_info: PathTargetRecord {
                    adapter_id: luid,
                    id,
                    mode_info_idx: target_index,
                    output_technology: i32::try_from(monitor.output_technology).unwrap_or(-1),
                    rotation: ROTATION_IDENTITY,
                    scaling: scaling_to_raw(monitor.scaling),
                    refresh_rate: current.refresh,
                    scan_line_ordering: SCAN_LINE_PROGRESSIVE,
                    target_available: 1,
                    status_flags: 0,
                },
                flags: PATH_ACTIVE,
            });
            record.targets.push(monitor.id.clone());
        }
        Ok(record)
    }

    fn restore_snapshot(
        &self,
        ticket: &Ticket,
        snapshot: &LayoutRecord,
    ) -> Result<SnapshotRestore, AppError> {
        self.journal.require_open(ticket);
        let mut targets = lock(&self.targets);
        let remapped = snapshot
            .adapters
            .iter()
            .filter(|adapter| self.luid_of(&targets, &adapter.adapter_device_path) != adapter.luid)
            .count();
        let fault = self.due(Op::RestoreSnapshot);
        if let Some(DisplayFault::RestoreSnapshotFails(status)) = fault {
            self.record(DisplayCall::RestoreSnapshot {
                change_id: ticket.change_id().to_owned(),
                targets: Vec::new(),
                remapped,
            });
            return Err(AppError::ModeRejected(Box::new(Failure::new(format!(
                "scripted fault: SetDisplayConfig with the saved snapshot returned {status}"
            )))));
        }
        let live: Vec<MonitorId> = targets
            .iter()
            .filter(|known| known.available)
            .map(|known| known.monitor.id.clone())
            .collect();
        let live_refs: Vec<&MonitorId> = live.iter().collect();
        let mut set = Vec::new();
        let mut left_out = Vec::new();
        for (path, identity) in snapshot.paths.iter().zip(&snapshot.targets) {
            let found = match_monitor(identity, &live_refs).and_then(|index| live.get(index));
            let (Some(found), Some(source), Some(timing)) = (
                found,
                source_of(snapshot, path.source_info.mode_info_idx),
                target_of(snapshot, path.target_info.mode_info_idx),
            ) else {
                left_out.push(identity.clone());
                continue;
            };
            set.push(found.clone());
            if fault == Some(DisplayFault::RestoreSnapshotChangesNothing) {
                continue;
            }
            if let Some(target) = targets
                .iter_mut()
                .find(|known| known.available && known.monitor.id == *found)
            {
                target.monitor.current = Some(RawMode {
                    width: source.width,
                    height: source.height,
                    refresh: timing.v_sync_freq,
                });
                target.monitor.scaling = scaling_from_raw(path.target_info.scaling);
                target.monitor.position_x = source.position.x;
                target.monitor.position_y = source.position.y;
            }
        }
        self.record(DisplayCall::RestoreSnapshot {
            change_id: ticket.change_id().to_owned(),
            targets: set,
            remapped,
        });
        Ok(SnapshotRestore { left_out })
    }
}

/// The source mode a saved path names.
fn source_of(snapshot: &LayoutRecord, index: u32) -> Option<SourceModeRecord> {
    match snapshot.modes.get(usize::try_from(index).ok()?)?.info {
        ModeInfoRecord::Source(source) => Some(source),
        _ => None,
    }
}

/// The target mode a saved path names.
fn target_of(snapshot: &LayoutRecord, index: u32) -> Option<TargetModeRecord> {
    match snapshot.modes.get(usize::try_from(index).ok()?)?.info {
        ModeInfoRecord::Target(target) => Some(target),
        _ => None,
    }
}

fn find<'a>(targets: &'a [FakeTarget], id: &MonitorId) -> Option<&'a FakeTarget> {
    targets
        .iter()
        .find(|target| target.available && target.monitor.id == *id)
}

fn gone() -> AppError {
    AppError::MonitorGone(Box::new(Failure::new(
        "the fake has no connected monitor with this identity",
    )))
}

/// What the fake reads back as path scaling after a change. Windows sets identity when the
/// source and target sizes match, whatever was asked; otherwise a scripted fault gives the
/// read-back, FullScreen gives stretched, and Keep leaves the path's scaling as it was.
fn scaling_after(
    monitor: &RawMonitor,
    set: RawMode,
    choice: ScalingChoice,
    fault: Option<DisplayFault>,
) -> Scaling {
    let native_size = monitor
        .native
        .is_some_and(|native| native.width == set.width && native.height == set.height);
    if native_size {
        return Scaling::Identity;
    }
    if let Some(DisplayFault::ApplyIgnoresScaling(read_back)) = fault {
        return read_back;
    }
    match choice {
        ScalingChoice::FullScreen => Scaling::Stretched,
        // Synthetic: identity cannot hold once the sizes differ, so the fake reads back the
        // aspect-ratio default that Windows 7 features give the adapter.
        ScalingChoice::Keep if monitor.scaling == Scaling::Identity => Scaling::AspectRatio,
        ScalingChoice::Keep => monitor.scaling,
    }
}

fn mode(width: u32, height: u32, hz: u32) -> RawMode {
    RawMode {
        width,
        height,
        refresh: Rational::hz(hz),
    }
}

/// The LG ULTRAGEAR on the RTX 4060 Ti, as recorded for the test PC on 11 Nov 2026. Values
/// the diagnostics tool has not read from the PC are marked synthetic and replaced at P1.
fn lg_ultragear() -> RawMonitor {
    // Rates at 1920 x 1080 as recorded for the test PC, each n / 1 until P1 reads the
    // exact rationals (HW-P1-02).
    let mut modes: Vec<RawMode> = [240, 144, 120, 100, 75, 60, 48]
        .into_iter()
        .map(|hz| mode(1920, 1080, hz))
        .collect();
    // 1280 x 882 at 240 Hz is listed, as recorded for the test PC on 11 Nov 2026.
    modes.push(mode(1280, 882, 240));
    // Synthetic: further sizes until P1 copies the full list.
    for (width, height) in [(1280, 1024), (1280, 960), (1024, 768), (800, 600)] {
        modes.push(mode(width, height, 240));
        modes.push(mode(width, height, 60));
    }
    RawMonitor {
        id: MonitorId {
            // Synthetic device path and EDID ids until HW-P1-01 records the real ones.
            device_path: String::from("fake-device-path:test-pc:lg-ultragear"),
            edid_manufacturer_id: Some(0x1E6D),
            edid_product_code: Some(0x5B8E),
            connector_instance: 0,
        },
        name: Some(String::from("LG ULTRAGEAR")),
        adapter_name: String::from("NVIDIA GeForce RTX 4060 Ti"),
        // The PCI\VEN_10DE&DEV_2803& start is recorded; the suffix is synthetic.
        adapter_instance_id: String::from(r"PCI\VEN_10DE&DEV_2803&SYNTHETIC_SUFFIX"),
        // Synthetic until the HW-P1 round records the real values.
        adapter_device_path: String::from("fake-adapter-path:test-pc:rtx-4060-ti"),
        gdi_name: String::from(r"\\.\DISPLAY1"),
        position_x: 0,
        position_y: 0,
        // Synthetic: DISPLAYCONFIG_OUTPUT_TECHNOLOGY_DISPLAYPORT_EXTERNAL.
        output_technology: 10,
        primary: true,
        active: true,
        native: Some(mode(1920, 1080, 240)),
        current: Some(mode(1920, 1080, 240)),
        // Synthetic: identity, because source and target sizes match.
        scaling: Scaling::Identity,
        modes,
    }
}

/// The Parsec virtual display: adapter ROOT\DISPLAY\0000 as recorded; the rest is synthetic
/// until HW-P1-03.
fn parsec_virtual_display() -> RawMonitor {
    RawMonitor {
        id: MonitorId {
            device_path: String::from("fake-device-path:test-pc:parsec-virtual-display"),
            edid_manufacturer_id: None,
            edid_product_code: None,
            connector_instance: 0,
        },
        name: None,
        adapter_name: String::from("Parsec Virtual Display Adapter"),
        adapter_instance_id: String::from(r"ROOT\DISPLAY\0000"),
        // Synthetic until the HW-P1 round records the real values.
        adapter_device_path: String::from("fake-adapter-path:test-pc:parsec"),
        gdi_name: String::from(r"\\.\DISPLAY2"),
        position_x: 1920,
        position_y: 0,
        // Synthetic: DISPLAYCONFIG_OUTPUT_TECHNOLOGY_INDIRECT_VIRTUAL.
        output_technology: 17,
        primary: false,
        active: true,
        native: Some(mode(1920, 1080, 60)),
        current: Some(mode(1920, 1080, 60)),
        scaling: Scaling::Identity,
        modes: vec![mode(1920, 1080, 60)],
    }
}

/// The Samsung LF24T35 on the RTX 4060 Ti. Every value but the name is synthetic until the
/// HW-P6 round.
fn samsung_lf24t35(connected: bool) -> RawMonitor {
    RawMonitor {
        id: MonitorId {
            device_path: String::from("fake-device-path:test-pc:samsung-lf24t35"),
            edid_manufacturer_id: None,
            edid_product_code: None,
            connector_instance: 1,
        },
        name: Some(String::from("Samsung LF24T35")),
        adapter_name: String::from("NVIDIA GeForce RTX 4060 Ti"),
        adapter_instance_id: String::from(r"PCI\VEN_10DE&DEV_2803&SYNTHETIC_SUFFIX"),
        // Synthetic until the HW-P6 round records the real values.
        adapter_device_path: String::from("fake-adapter-path:test-pc:rtx-4060-ti"),
        gdi_name: String::from(r"\\.\DISPLAY3"),
        position_x: -1920,
        position_y: 0,
        output_technology: 5,
        primary: false,
        active: connected,
        native: Some(mode(1920, 1080, 60)),
        current: connected.then(|| mode(1920, 1080, 60)),
        scaling: Scaling::Identity,
        modes: vec![
            mode(1920, 1080, 60),
            mode(1280, 1024, 60),
            mode(1024, 768, 60),
        ],
    }
}

/// A monitor on an adapter that is not NVIDIA. Every value is synthetic.
fn other_vendor_monitor() -> RawMonitor {
    RawMonitor {
        id: MonitorId {
            device_path: String::from("fake-device-path:not-nvidia:monitor"),
            edid_manufacturer_id: None,
            edid_product_code: None,
            connector_instance: 0,
        },
        name: Some(String::from("Generic monitor")),
        adapter_name: String::from("Other graphics adapter"),
        adapter_instance_id: String::from(r"PCI\VEN_0000&DEV_0000&SYNTHETIC_SUFFIX"),
        // Synthetic until the HW-P1 round records the real values.
        adapter_device_path: String::from("fake-adapter-path:not-nvidia:adapter"),
        gdi_name: String::from(r"\\.\DISPLAY1"),
        position_x: 0,
        position_y: 0,
        output_technology: 5,
        primary: true,
        active: true,
        native: Some(mode(1920, 1080, 60)),
        current: Some(mode(1920, 1080, 60)),
        scaling: Scaling::Identity,
        modes: vec![
            mode(1920, 1080, 60),
            mode(1440, 1080, 60),
            mode(1280, 960, 60),
        ],
    }
}

/// Every monitor the fixture knows, connected or not, as the other fakes read their
/// adapters from it.
pub fn known_monitors(profile: FixtureProfile) -> Vec<RawMonitor> {
    fixture(profile)
        .into_iter()
        .map(|target| target.monitor)
        .collect()
}

fn fixture(profile: FixtureProfile) -> Vec<FakeTarget> {
    // What Windows keeps for each monitor starts as the fixture's own mode and scaling.
    let target = |monitor: RawMonitor, available: bool| FakeTarget {
        stored_mode: monitor.current,
        stored_scaling: monitor.scaling,
        monitor,
        available,
    };
    match profile {
        FixtureProfile::TestPc => vec![
            target(lg_ultragear(), true),
            target(parsec_virtual_display(), true),
            // Known to Windows but not connected: no active path, targetAvailable false.
            target(samsung_lf24t35(false), false),
        ],
        FixtureProfile::NotNvidia => vec![target(other_vendor_monitor(), true)],
        FixtureProfile::TwoMonitors => vec![
            target(lg_ultragear(), true),
            target(parsec_virtual_display(), true),
            target(samsung_lf24t35(true), true),
        ],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::display::TargetConfig;
    use crate::error::ErrorCode;

    fn lg_id() -> MonitorId {
        known_monitors(FixtureProfile::TestPc).remove(0).id
    }

    fn config_for(monitor: &MonitorId, set: RawMode) -> DisplayConfig {
        DisplayConfig {
            targets: vec![TargetConfig {
                monitor: monitor.clone(),
                mode: set,
                scaling: ScalingChoice::FullScreen,
            }],
        }
    }

    fn stretched_config(display: &FakeDisplay) -> DisplayConfig {
        let lg = display.enumerate().unwrap().remove(0);
        config_for(&lg.id, mode(1280, 960, 240))
    }

    fn applied_routes(display: &FakeDisplay) -> Vec<RouteChoice> {
        display
            .calls()
            .into_iter()
            .filter_map(|call| match call {
                DisplayCall::Apply { route, .. } => Some(route),
                _ => None,
            })
            .collect()
    }

    fn live(display: &FakeDisplay, id: &MonitorId) -> RawMonitor {
        display.monitor(id).unwrap()
    }

    #[test]
    fn req_disp_01_test_pc_fixture() {
        let display = FakeDisplay::new(FixtureProfile::TestPc, OpenEntries::default());
        let monitors = display.enumerate().unwrap();
        let names: Vec<Option<&str>> = monitors.iter().map(|m| m.name.as_deref()).collect();
        // The Samsung is not connected, so only the LG and the virtual display are read.
        assert_eq!(names, vec![Some("LG ULTRAGEAR"), None]);
        assert_eq!(monitors[0].native, Some(mode(1920, 1080, 240)));
        assert_eq!(
            FakeDisplay::new(FixtureProfile::TwoMonitors, OpenEntries::default())
                .enumerate()
                .unwrap()
                .len(),
            3
        );
        assert_eq!(
            FixtureProfile::from_name("not-nvidia"),
            Some(FixtureProfile::NotNvidia)
        );
        assert_eq!(FixtureProfile::from_name("Test-PC"), None);
    }

    #[test]
    fn req_disp_01_ticket_for_an_open_entry_applies() {
        let journal = OpenEntries::default();
        let display = FakeDisplay::new(FixtureProfile::TestPc, journal.clone());
        let config = stretched_config(&display);
        display.test_config(&config).unwrap();
        let ticket = journal.open_for_tests("change-1");
        display.apply_config(&ticket, &config).unwrap();
        let lg = display.enumerate().unwrap().remove(0);
        assert_eq!(lg.current, Some(mode(1280, 960, 240)));
        assert_eq!(lg.scaling, Scaling::Stretched);
        assert!(matches!(
            display.calls().as_slice(),
            [
                DisplayCall::Enumerate,
                DisplayCall::Test { .. },
                DisplayCall::Apply { .. },
                DisplayCall::Enumerate
            ]
        ));
    }

    #[test]
    #[should_panic(expected = "has no open journal entry")]
    fn req_disp_01_pc_changes_need_ticket() {
        let journal = OpenEntries::default();
        let display = FakeDisplay::new(FixtureProfile::TestPc, journal.clone());
        let config = stretched_config(&display);
        let ticket = journal.open_for_tests("change-1");
        journal.close_for_tests(&ticket);
        // The entry is closed, so this change has nothing on disk to undo it.
        let _ = display.apply_config(&ticket, &config);
    }

    #[test]
    #[should_panic(expected = "has no open journal entry")]
    fn req_disp_01_database_current_needs_ticket() {
        let journal = OpenEntries::default();
        let display = FakeDisplay::new(FixtureProfile::TestPc, journal.clone());
        let ticket = journal.open_for_tests("change-1");
        journal.close_for_tests(&ticket);
        let _ = display.apply_database_current(&ticket);
    }

    #[test]
    fn req_disp_01_scripted_faults_fire_as_scripted() {
        let journal = OpenEntries::default();
        let display = FakeDisplay::new(FixtureProfile::TestPc, journal.clone());
        let config = stretched_config(&display);
        display.script(DisplayFault::TestRefuses(-2), When::Once);
        let refused = display.test_config(&config).unwrap_err();
        assert_eq!(refused.code(), ErrorCode::ModeTestFailed);
        display.test_config(&config).unwrap();

        display.script(DisplayFault::ApplyChangesNothing, When::Nth(2));
        let ticket = journal.open_for_tests("change-1");
        display.apply_config(&ticket, &config).unwrap();
        let back = DisplayConfig {
            targets: vec![TargetConfig {
                mode: mode(1920, 1080, 240),
                ..config.targets[0].clone()
            }],
        };
        display.apply_config(&ticket, &back).unwrap();
        // The second apply changed nothing, so the stretched mode is still on.
        let lg = display.enumerate().unwrap().remove(0);
        assert_eq!(lg.current, Some(mode(1280, 960, 240)));

        display.script(DisplayFault::EnumerateFails, When::Always);
        assert!(display.enumerate().is_err());
        assert!(display.enumerate().is_err());
    }

    #[test]
    fn req_disp_01_display_call_log_records_calls() {
        let display = FakeDisplay::new(FixtureProfile::TestPc, OpenEntries::default());
        let config = stretched_config(&display);
        display.test_config(&config).unwrap();
        assert_eq!(
            display.calls(),
            vec![
                DisplayCall::Enumerate,
                DisplayCall::Test {
                    route: RouteChoice::gdi(GdiFlag::Fullscreen),
                    config
                }
            ]
        );
        assert!(!display.applied_anything());
    }

    #[test]
    fn req_disp_01_fault_script_counts_calls_per_operation() {
        let script: FaultScript<DisplayFault> = FaultScript::default();
        script.push(DisplayFault::ApplyFails(-1), When::Nth(2));
        script.push(DisplayFault::EnumerateFails, When::Once);
        let apply = |fault: DisplayFault| fault.op() == Op::Apply;
        assert_eq!(script.due(apply), None);
        assert_eq!(script.due(apply), Some(DisplayFault::ApplyFails(-1)));
        assert_eq!(script.due(apply), None);
        let enumerate = |fault: DisplayFault| fault.op() == Op::Enumerate;
        assert_eq!(script.due(enumerate), Some(DisplayFault::EnumerateFails));
        // Once is spent after it fired.
        assert_eq!(script.due(enumerate), None);
    }

    #[test]
    fn req_disp_01_test_refuses_unlisted_modes() {
        let display = FakeDisplay::new(FixtureProfile::TestPc, OpenEntries::default());
        let mut config = stretched_config(&display);
        config.targets[0].mode = mode(1440, 1080, 240);
        let refused = display.test_config(&config).unwrap_err();
        assert_eq!(refused.code(), ErrorCode::ModeTestFailed);
        assert!(!display.applied_anything());
    }

    #[test]
    fn req_disp_14_log_shows_only_temporary_routes() {
        let journal = OpenEntries::default();
        let display = FakeDisplay::new(FixtureProfile::TestPc, journal.clone());
        let config = stretched_config(&display);
        let ticket = journal.open_for_tests("change-1");
        let routes = [
            RouteChoice::ccd(),
            RouteChoice::gdi(GdiFlag::Fullscreen),
            RouteChoice::gdi(GdiFlag::Zero),
        ];
        for route in routes {
            display.set_route(route);
            assert_eq!(display.route(), route);
            display.test_config(&config).unwrap();
            display.apply_config(&ticket, &config).unwrap();
        }
        let applied = applied_routes(&display);
        assert_eq!(applied, routes);
        // Each logged route is one of the temporary forms; none can carry a registry or
        // database flag.
        assert!(applied.iter().all(|route| route.is_consistent()));
        assert!(
            !RouteChoice {
                route: Route::Ccd,
                gdi_flag: Some(GdiFlag::Zero)
            }
            .is_consistent()
        );
        assert!(
            !RouteChoice {
                route: Route::Gdi,
                gdi_flag: None
            }
            .is_consistent()
        );
        // Each test call shows the route it tested on.
        let tested: Vec<RouteChoice> = display
            .calls()
            .into_iter()
            .filter_map(|call| match call {
                DisplayCall::Test { route, .. } => Some(route),
                _ => None,
            })
            .collect();
        assert_eq!(tested, routes);
    }

    #[test]
    #[should_panic(expected = "route and GDI flag do not fit")]
    fn req_disp_14_route_with_a_flag_that_does_not_fit_is_refused() {
        let display = FakeDisplay::new(FixtureProfile::TestPc, OpenEntries::default());
        display.set_route(RouteChoice {
            route: Route::Ccd,
            gdi_flag: Some(GdiFlag::Fullscreen),
        });
    }

    #[test]
    fn req_disp_15_flag_zero_while_fullscreen_open_is_logged() {
        let journal = OpenEntries::default();
        let display = FakeDisplay::new(FixtureProfile::TestPc, journal.clone());
        let lg = lg_id();
        let stretched = config_for(&lg, mode(1280, 960, 240));
        let other = config_for(&lg, mode(1024, 768, 240));
        // The default route: GDI with CDS_FULLSCREEN.
        let first = journal.open_for_tests("change-1");
        display.apply_config(&first, &stretched).unwrap();
        // A revert on the same route and flag keeps the safety net.
        display.apply_config(&first, &other).unwrap();
        assert!(display.flag_zero_while_fullscreen_open().is_empty());

        display.set_route(RouteChoice::gdi(GdiFlag::Zero));
        let second = journal.open_for_tests("change-2");
        display.apply_config(&second, &stretched).unwrap();
        assert_eq!(
            display.flag_zero_while_fullscreen_open(),
            vec![String::from("change-2")]
        );

        // Once the CDS_FULLSCREEN change is closed, a flag-0 call cancels nothing.
        journal.close_for_tests(&first);
        let third = journal.open_for_tests("change-3");
        display.apply_config(&third, &other).unwrap();
        assert_eq!(
            display.flag_zero_while_fullscreen_open(),
            vec![String::from("change-2")]
        );
        // CCD calls carry no GDI flag and are never counted.
        display.set_route(RouteChoice::ccd());
        display.apply_config(&third, &stretched).unwrap();
        assert_eq!(display.flag_zero_while_fullscreen_open().len(), 1);
        assert_eq!(
            applied_routes(&display),
            vec![
                RouteChoice::gdi(GdiFlag::Fullscreen),
                RouteChoice::gdi(GdiFlag::Fullscreen),
                RouteChoice::gdi(GdiFlag::Zero),
                RouteChoice::gdi(GdiFlag::Zero),
                RouteChoice::ccd(),
            ]
        );
    }

    #[test]
    fn req_safe_17_apply_faults_change_what_reads_back() {
        let journal = OpenEntries::default();
        let display = FakeDisplay::new(FixtureProfile::TestPc, journal.clone());
        let lg = lg_id();
        let stretched = config_for(&lg, mode(1280, 960, 240));
        let native = config_for(&lg, mode(1920, 1080, 240));
        let ticket = journal.open_for_tests("change-1");

        // INJ-D08: an error, and nothing changed.
        display.script(DisplayFault::ApplyFails(-1), When::Once);
        let refused = display.apply_config(&ticket, &stretched).unwrap_err();
        assert_eq!(refused.code(), ErrorCode::ModeRejected);
        assert_eq!(live(&display, &lg).current, Some(mode(1920, 1080, 240)));

        // INJ-D09: success reported, another size set.
        display.script(
            DisplayFault::ApplySetsOtherSize {
                width: 1024,
                height: 768,
            },
            When::Once,
        );
        display.apply_config(&ticket, &stretched).unwrap();
        assert_eq!(live(&display, &lg).current, Some(mode(1024, 768, 240)));

        // INJ-D11: success reported, another rate set.
        display.script(
            DisplayFault::ApplySetsOtherRate(Rational::hz(60)),
            When::Once,
        );
        display.apply_config(&ticket, &stretched).unwrap();
        assert_eq!(live(&display, &lg).current, Some(mode(1280, 960, 60)));

        // INJ-D12: the mode is set, the scaling request is not.
        display.script(
            DisplayFault::ApplyIgnoresScaling(Scaling::AspectRatio),
            When::Always,
        );
        display.apply_config(&ticket, &stretched).unwrap();
        let now = live(&display, &lg);
        assert_eq!(now.current, Some(mode(1280, 960, 240)));
        assert_eq!(now.scaling, Scaling::AspectRatio);
        // Same source and target sizes give identity whatever was asked.
        display.apply_config(&ticket, &native).unwrap();
        assert_eq!(live(&display, &lg).scaling, Scaling::Identity);
    }

    #[test]
    fn req_safe_17_keep_scaling_never_reads_back_identity_at_another_size() {
        let journal = OpenEntries::default();
        let display = FakeDisplay::new(FixtureProfile::TestPc, journal.clone());
        let lg = lg_id();
        let mut keep = config_for(&lg, mode(1280, 960, 240));
        keep.targets[0].scaling = ScalingChoice::Keep;
        let ticket = journal.open_for_tests("change-1");
        display.apply_config(&ticket, &keep).unwrap();
        assert_eq!(live(&display, &lg).scaling, Scaling::AspectRatio);
    }

    #[test]
    fn req_safe_09_unplug_and_plug() {
        let display = FakeDisplay::new(FixtureProfile::TestPc, OpenEntries::default());
        let lg = lg_id();
        assert!(display.unplug(&lg));
        assert!(!display.unplug(&lg));
        let names: Vec<Option<String>> = display
            .enumerate()
            .unwrap()
            .into_iter()
            .map(|monitor| monitor.name)
            .collect();
        assert_eq!(names, vec![None]);
        let refused = display
            .test_config(&config_for(&lg, mode(1280, 960, 240)))
            .unwrap_err();
        assert_eq!(refused.code(), ErrorCode::MonitorGone);
        assert!(!display.change_from_outside(
            &lg,
            mode(1024, 768, 60),
            Scaling::Stretched,
            OutsideChange::Temporary
        ));

        assert!(display.plug(&lg));
        assert!(!display.plug(&lg));
        let back = display.enumerate().unwrap().remove(0);
        assert_eq!(back.id, lg);
        assert_eq!(back.current, Some(mode(1920, 1080, 240)));

        // Known to Windows but never connected: it comes up at its preferred mode.
        let samsung = known_monitors(FixtureProfile::TestPc).remove(2).id;
        assert!(display.plug(&samsung));
        let connected = display
            .enumerate()
            .unwrap()
            .into_iter()
            .find(|monitor| monitor.id == samsung)
            .unwrap();
        assert!(connected.active);
        assert_eq!(connected.current, Some(mode(1920, 1080, 60)));
    }

    #[test]
    fn req_safe_09_unplug_faults_fire_between_calls() {
        let journal = OpenEntries::default();
        let display = FakeDisplay::new(FixtureProfile::TestPc, journal.clone());
        let lg = lg_id();
        let stretched = config_for(&lg, mode(1280, 960, 240));
        let ticket = journal.open_for_tests("change-1");

        display.script(DisplayFault::UnplugBeforeTest, When::Once);
        let refused = display.test_config(&stretched).unwrap_err();
        assert_eq!(refused.code(), ErrorCode::MonitorGone);
        assert!(display.plug(&lg));

        display.script(DisplayFault::UnplugBeforeApply, When::Once);
        let refused = display.apply_config(&ticket, &stretched).unwrap_err();
        assert_eq!(refused.code(), ErrorCode::MonitorGone);
        assert_eq!(live(&display, &lg).current, Some(mode(1920, 1080, 240)));
        assert!(display.plug(&lg));

        display.script(DisplayFault::UnplugAfterApply, When::Once);
        display.apply_config(&ticket, &stretched).unwrap();
        assert!(
            display
                .enumerate()
                .unwrap()
                .iter()
                .all(|monitor| monitor.id != lg)
        );
        // The mode was set before the monitor went.
        assert_eq!(live(&display, &lg).current, Some(mode(1280, 960, 240)));
    }

    #[test]
    fn req_disp_22_database_current_puts_back_what_windows_keeps() {
        let journal = OpenEntries::default();
        let display = FakeDisplay::new(FixtureProfile::TestPc, journal.clone());
        let lg = lg_id();
        let stretched = config_for(&lg, mode(1280, 960, 240));
        let ticket = journal.open_for_tests("change-1");

        display.apply_config(&ticket, &stretched).unwrap();
        display.apply_database_current(&ticket).unwrap();
        let now = live(&display, &lg);
        assert_eq!(now.current, Some(mode(1920, 1080, 240)));
        assert_eq!(now.scaling, Scaling::Identity);

        display.apply_config(&ticket, &stretched).unwrap();
        display.script(DisplayFault::DatabaseCurrentChangesNothing, When::Once);
        display.apply_database_current(&ticket).unwrap();
        assert_eq!(live(&display, &lg).current, Some(mode(1280, 960, 240)));
        display.script(DisplayFault::DatabaseCurrentFails(1), When::Once);
        let refused = display.apply_database_current(&ticket).unwrap_err();
        assert_eq!(refused.code(), ErrorCode::ModeRejected);
        assert_eq!(live(&display, &lg).current, Some(mode(1280, 960, 240)));

        // A mode saved in Windows display settings is what the database holds from then on.
        assert!(display.change_from_outside(
            &lg,
            mode(1920, 1080, 144),
            Scaling::Identity,
            OutsideChange::Saved
        ));
        display.apply_config(&ticket, &stretched).unwrap();
        display.apply_database_current(&ticket).unwrap();
        assert_eq!(live(&display, &lg).current, Some(mode(1920, 1080, 144)));
        assert!(matches!(
            display.calls().last(),
            Some(DisplayCall::ApplyDatabaseCurrent { change_id }) if change_id == "change-1"
        ));
    }

    #[test]
    fn req_disp_23_outside_change_shows_on_the_next_read() {
        let journal = OpenEntries::default();
        let display = FakeDisplay::new(FixtureProfile::TestPc, journal.clone());
        let lg = lg_id();
        let before = display.calls().len();
        // A game sets its own mode: no call reaches the fake.
        assert!(display.change_from_outside(
            &lg,
            mode(1024, 768, 60),
            Scaling::Stretched,
            OutsideChange::Temporary
        ));
        assert_eq!(display.calls().len(), before);
        assert!(!display.applied_anything());
        let read = display.enumerate().unwrap().remove(0);
        assert_eq!(read.current, Some(mode(1024, 768, 60)));
        assert_eq!(read.scaling, Scaling::Stretched);
        // A temporary change leaves the kept configuration alone.
        let ticket = journal.open_for_tests("change-1");
        display.apply_database_current(&ticket).unwrap();
        assert_eq!(live(&display, &lg).current, Some(mode(1920, 1080, 240)));
    }

    #[test]
    fn req_disp_06_new_mode_shows_on_the_next_read() {
        let display = FakeDisplay::new(FixtureProfile::TestPc, OpenEntries::default());
        let lg = lg_id();
        let added = config_for(&lg, mode(1440, 1080, 240));
        assert!(display.test_config(&added).is_err());
        assert!(display.edit_monitor(&lg, |monitor| {
            monitor.modes.push(mode(1440, 1080, 240));
        }));
        display.test_config(&added).unwrap();
        assert!(
            display.enumerate().unwrap()[0]
                .modes
                .contains(&mode(1440, 1080, 240))
        );
        let stranger = MonitorId {
            device_path: String::from("no-such-monitor"),
            ..lg
        };
        assert!(!display.edit_monitor(&stranger, |_| {}));
        assert!(display.monitor(&stranger).is_none());
    }

    #[test]
    fn req_safe_02_snapshot_holds_every_connected_monitor() {
        let display = FakeDisplay::new(FixtureProfile::TestPc, OpenEntries::default());
        let snapshot = display.snapshot().unwrap();
        // The LG and the virtual display; the Samsung is not connected.
        assert_eq!(snapshot.targets.len(), 2);
        assert_eq!(snapshot.targets[0], lg_id());
        assert_eq!(snapshot.paths.len(), 2);
        assert_eq!(snapshot.modes.len(), 4);
        assert_eq!(snapshot.adapters.len(), 2);
        assert_eq!(
            source_of(&snapshot, snapshot.paths[0].source_info.mode_info_idx).map(|s| s.width),
            Some(1920)
        );
        assert_eq!(
            target_of(&snapshot, snapshot.paths[0].target_info.mode_info_idx)
                .map(|t| t.v_sync_freq),
            Some(Rational::hz(240))
        );
        assert!(display.calls().contains(&DisplayCall::Snapshot));
        assert!(!display.applied_anything());
        display.script(DisplayFault::SnapshotFails, When::Once);
        assert_eq!(display.snapshot().unwrap_err().code(), ErrorCode::Internal);
    }

    #[test]
    fn req_disp_22_snapshot_restore_puts_the_saved_modes_back() {
        let journal = OpenEntries::default();
        let display = FakeDisplay::new(FixtureProfile::TestPc, journal.clone());
        let lg = lg_id();
        let snapshot = display.snapshot().unwrap();
        let ticket = journal.open_for_tests("change-1");
        display
            .apply_config(&ticket, &config_for(&lg, mode(1280, 960, 240)))
            .unwrap();
        // A restart gave the adapters new LUIDs: the restore maps them by device path.
        display.change_adapter_luids();
        let done = display.restore_snapshot(&ticket, &snapshot).unwrap();
        assert!(done.left_out.is_empty());
        let now = live(&display, &lg);
        assert_eq!(now.current, Some(mode(1920, 1080, 240)));
        assert_eq!(now.scaling, Scaling::Identity);
        assert!(matches!(
            display.calls().last(),
            Some(DisplayCall::RestoreSnapshot { remapped: 2, .. })
        ));

        // A monitor that is gone is left out; the others still go back.
        display
            .apply_config(&ticket, &config_for(&lg, mode(1280, 960, 240)))
            .unwrap();
        let parsec = known_monitors(FixtureProfile::TestPc).remove(1).id;
        assert!(display.unplug(&parsec));
        let done = display.restore_snapshot(&ticket, &snapshot).unwrap();
        assert_eq!(done.left_out, vec![parsec]);
        assert_eq!(live(&display, &lg).current, Some(mode(1920, 1080, 240)));

        // INJ-D20: the call fails, or reports success and changes nothing.
        display
            .apply_config(&ticket, &config_for(&lg, mode(1280, 960, 240)))
            .unwrap();
        display.script(DisplayFault::RestoreSnapshotFails(31), When::Once);
        let refused = display.restore_snapshot(&ticket, &snapshot).unwrap_err();
        assert_eq!(refused.code(), ErrorCode::ModeRejected);
        display.script(DisplayFault::RestoreSnapshotChangesNothing, When::Once);
        display.restore_snapshot(&ticket, &snapshot).unwrap();
        assert_eq!(live(&display, &lg).current, Some(mode(1280, 960, 240)));
    }

    #[test]
    #[should_panic(expected = "has no open journal entry")]
    fn req_disp_01_snapshot_restore_needs_ticket() {
        let journal = OpenEntries::default();
        let display = FakeDisplay::new(FixtureProfile::TestPc, journal.clone());
        let snapshot = display.snapshot().unwrap();
        let ticket = journal.open_for_tests("change-1");
        journal.close_for_tests(&ticket);
        let _ = display.restore_snapshot(&ticket, &snapshot);
    }
}
