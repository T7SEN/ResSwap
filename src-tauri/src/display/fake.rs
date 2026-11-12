//! The fake display backend: fixture monitors in memory, a call log and scripted faults.
//! No OS call, so every test and every fake-backend run leaves the PC as it is
//! (REQ-DISP-01). The fixture profiles, the fault script and the call log here are shared by
//! every other fake.

#![forbid(unsafe_code)]

use std::sync::{Mutex, MutexGuard, PoisonError};

use crate::apply::{OpenEntries, Ticket};
use crate::display::types::{MonitorId, Scaling, ScalingChoice};
use crate::display::{DisplayBackend, DisplayConfig, Rational, RawMode, RawMonitor};
use crate::error::{AppError, ErrorContext, Failure};

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

/// A failure a test scripts into the fake.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DisplayFault {
    /// Enumeration fails (INJ-D01): INTERNAL.
    EnumerateFails,
    /// Enumeration panics, as a bug inside a command would.
    EnumeratePanics,
    /// The test call refuses the configuration with this DISP_CHANGE value or
    /// SetDisplayConfig error (INJ-D06): MODE_TEST_FAILED.
    TestRefuses(i32),
    /// The apply call fails with this value (INJ-D08): MODE_REJECTED.
    ApplyFails(i32),
    /// The apply call reports success but changes nothing (INJ-D10).
    ApplyChangesNothing,
}

impl DisplayFault {
    fn op(self) -> Op {
        match self {
            Self::EnumerateFails | Self::EnumeratePanics => Op::Enumerate,
            Self::TestRefuses(_) => Op::Test,
            Self::ApplyFails(_) | Self::ApplyChangesNothing => Op::Apply,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Op {
    Enumerate,
    Test,
    Apply,
}

/// One call the fake received, in order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DisplayCall {
    Enumerate,
    Test(DisplayConfig),
    Apply {
        change_id: String,
        config: DisplayConfig,
    },
}

/// One target the fixture knows. An unavailable one is known to Windows but not connected,
/// so enumeration leaves it out.
#[derive(Clone, Debug)]
struct FakeTarget {
    monitor: RawMonitor,
    available: bool,
}

/// The display backend on fixture monitors, for every test and every fake-backend run.
pub struct FakeDisplay {
    journal: OpenEntries,
    targets: Mutex<Vec<FakeTarget>>,
    log: CallLog<DisplayCall>,
    script: FaultScript<DisplayFault>,
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
            targets: Mutex::new(fixture(profile)),
            log: CallLog::default(),
            script: FaultScript::default(),
        }
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
        self.log
            .any(|call| matches!(call, DisplayCall::Apply { .. }))
    }

    fn record(&self, call: DisplayCall) {
        self.log.record(call);
    }

    /// The fault that fires on this call of `op`, if any.
    fn due(&self, op: Op) -> Option<DisplayFault> {
        self.script.due(|fault| fault.op() == op)
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

    fn test_config(&self, config: &DisplayConfig) -> Result<(), AppError> {
        self.record(DisplayCall::Test(config.clone()));
        if let Some(DisplayFault::TestRefuses(status)) = self.due(Op::Test) {
            return Err(AppError::ModeTestFailed(Box::new(Failure::new(format!(
                "scripted fault: the test call returned {status}"
            )))));
        }
        let targets = lock(&self.targets);
        for target in &config.targets {
            let Some(found) = find(&targets, &target.monitor) else {
                return Err(AppError::MonitorGone(Box::new(Failure::new(
                    "the fake has no connected monitor with this identity",
                ))));
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
        self.record(DisplayCall::Apply {
            change_id: ticket.change_id().to_owned(),
            config: config.clone(),
        });
        match self.due(Op::Apply) {
            Some(DisplayFault::ApplyFails(status)) => {
                return Err(AppError::ModeRejected(Box::new(Failure::new(format!(
                    "scripted fault: the apply call returned {status}"
                )))));
            }
            Some(_) => return Ok(()),
            None => {}
        }
        let mut targets = lock(&self.targets);
        for target in &config.targets {
            let Some(found) = targets
                .iter_mut()
                .find(|known| known.available && known.monitor.id == target.monitor)
            else {
                return Err(AppError::MonitorGone(Box::new(Failure::new(
                    "the fake has no connected monitor with this identity",
                ))));
            };
            found.monitor.current = Some(target.mode);
            found.monitor.scaling = scaling_after(&found.monitor, target);
        }
        Ok(())
    }
}

fn find<'a>(targets: &'a [FakeTarget], id: &MonitorId) -> Option<&'a FakeTarget> {
    targets
        .iter()
        .find(|target| target.available && target.monitor.id == *id)
}

fn same_mode(a: RawMode, b: RawMode) -> bool {
    a.width == b.width && a.height == b.height && super::modes::rational_eq(a.refresh, b.refresh)
}

/// What the fake reads back as scaling after a change. Same sizes give identity; a smaller
/// source with FullScreen gives stretched; Keep leaves the path's scaling as it was.
fn scaling_after(monitor: &RawMonitor, target: &crate::display::TargetConfig) -> Scaling {
    let native_size = monitor.native.is_some_and(|native| {
        native.width == target.mode.width && native.height == target.mode.height
    });
    if native_size {
        return Scaling::Identity;
    }
    match target.scaling {
        ScalingChoice::FullScreen => Scaling::Stretched,
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
    let target = |monitor: RawMonitor, available: bool| FakeTarget { monitor, available };
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

    fn stretched_config(display: &FakeDisplay) -> DisplayConfig {
        let lg = display.enumerate().unwrap().remove(0);
        DisplayConfig {
            targets: vec![TargetConfig {
                monitor: lg.id,
                mode: mode(1280, 960, 240),
                scaling: ScalingChoice::FullScreen,
            }],
        }
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
                DisplayCall::Test(_),
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
    fn req_disp_01_scripted_faults_fire_as_scripted() {
        let journal = OpenEntries::default();
        let display = FakeDisplay::new(FixtureProfile::TestPc, journal.clone());
        let config = stretched_config(&display);
        display.script(DisplayFault::TestRefuses(-2), When::Once);
        let refused = display.test_config(&config).unwrap_err();
        assert_eq!(refused.code(), crate::error::ErrorCode::ModeTestFailed);
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
            vec![DisplayCall::Enumerate, DisplayCall::Test(config)]
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
        assert_eq!(refused.code(), crate::error::ErrorCode::ModeTestFailed);
        assert!(!display.applied_anything());
    }
}
