//! The fake vendor extras: the fixture's adapters and custom displays in memory, a call log
//! and scripted faults. No driver call, so no test touches the graphics driver
//! (REQ-DISP-01).

#![forbid(unsafe_code)]

use std::sync::Mutex;

use crate::apply::{OpenEntries, Ticket};
use crate::display::fake::{CallLog, FaultScript, FixtureProfile, When, known_monitors, lock};
use crate::display::modes::to_refresh;
use crate::display::monitors::vendor_of;
use crate::display::types::{GpuVendor, MonitorId};
use crate::display::{Rational, RawMode, RawMonitor};
use crate::error::{AppError, ErrorContext, Failure};
use crate::ipc::types::{GpuInfo, NvapiStatus};
use crate::vendor::{
    AdapterKey, CustomEntry, CustomSource, DISPLAY_CLASS, InstanceId, NvidiaAdapter, Origin,
    VendorExtras,
};

/// The NVIDIA adapter's display class subkey on the test PC, as read with reg query on
/// 11 Nov 2026.
const TEST_PC_NVIDIA_KEY: &str = "0001";

/// A failure a test scripts into the fake.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VendorFault {
    /// The adapter enumeration fails: INTERNAL.
    AdaptersFail,
    /// nvapi64.dll is missing or NvAPI_Initialize fails: NVAPI_MISSING.
    NvapiMissing,
    /// An NVAPI call returns this status, for example -137: NVAPI_CALL_FAILED.
    CallFails(i32),
    /// TryCustomDisplay refuses the timing with this status: CUSTOM_TIMING_REFUSED.
    TimingRefused(i32),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Op {
    Adapters,
    CustomDisplays,
    Try,
}

impl VendorFault {
    fn fires_on(self, op: Op) -> bool {
        match self {
            Self::AdaptersFail => op == Op::Adapters,
            Self::NvapiMissing | Self::CallFails(_) => op != Op::Adapters,
            Self::TimingRefused(_) => op == Op::Try,
        }
    }
}

/// One call the fake received, in order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum VendorCall {
    Adapters,
    NvidiaAdapter(MonitorId),
    CustomDisplays,
    TryCustomDisplay {
        change_id: String,
        monitor: MonitorId,
        mode: RawMode,
    },
}

/// The vendor extras on the fixture's adapters, for every test and every fake-backend run.
pub struct FakeVendor {
    profile: FixtureProfile,
    journal: OpenEntries,
    monitors: Vec<RawMonitor>,
    trial: Mutex<Option<(MonitorId, RawMode)>>,
    log: CallLog<VendorCall>,
    script: FaultScript<VendorFault>,
}

impl FakeVendor {
    /// The fake for `profile`, checking every Ticket against `journal`.
    pub fn new(profile: FixtureProfile, journal: OpenEntries) -> Self {
        Self {
            profile,
            journal,
            monitors: known_monitors(profile),
            trial: Mutex::new(None),
            log: CallLog::default(),
            script: FaultScript::default(),
        }
    }

    /// Adds a fault to the script.
    pub fn script(&self, fault: VendorFault, when: When) {
        self.script.push(fault, when);
    }

    /// Every call so far, in order.
    pub fn calls(&self) -> Vec<VendorCall> {
        self.log.calls()
    }

    /// Whether any call so far changed the fake PC.
    pub fn changed_anything(&self) -> bool {
        self.log
            .any(|call| matches!(call, VendorCall::TryCustomDisplay { .. }))
    }

    /// The custom display trial on screen, if any.
    pub fn trial(&self) -> Option<(MonitorId, RawMode)> {
        lock(&self.trial).clone()
    }

    fn fault(&self, op: Op, function: &str) -> Result<(), AppError> {
        let Some(fault) = self.script.due(|fault| fault.fires_on(op)) else {
            return Ok(());
        };
        let context = ErrorContext {
            nvapi_function: Some(function.to_owned()),
            ..ErrorContext::default()
        };
        Err(match fault {
            VendorFault::AdaptersFail => {
                AppError::internal("scripted fault: the fake adapter enumeration failed")
            }
            VendorFault::NvapiMissing => AppError::NvapiMissing(Box::new(Failure::new(
                "scripted fault: nvapi64.dll did not load",
            ))),
            VendorFault::CallFails(status) => AppError::NvapiCallFailed(Box::new(
                Failure::new(format!("scripted fault: {function} returned {status}")).with_context(
                    ErrorContext {
                        nvapi_status: Some(status),
                        ..context
                    },
                ),
            )),
            VendorFault::TimingRefused(status) => AppError::CustomTimingRefused(Box::new(
                Failure::new(format!("scripted fault: {function} returned {status}")).with_context(
                    ErrorContext {
                        nvapi_status: Some(status),
                        ..context
                    },
                ),
            )),
        })
    }

    fn connected(&self, monitor: &MonitorId) -> Result<&RawMonitor, AppError> {
        self.monitors
            .iter()
            .find(|known| known.active && known.id == *monitor)
            .ok_or_else(|| {
                AppError::MonitorGone(Box::new(Failure::new(
                    "the fake has no connected monitor with this identity",
                )))
            })
    }

    fn has_nvidia(&self) -> bool {
        self.monitors
            .iter()
            .any(|monitor| vendor_of(&monitor.adapter_instance_id) == GpuVendor::Nvidia)
    }
}

/// NOT_NVIDIA for an adapter that is not NVIDIA's.
fn not_nvidia(adapter: &str) -> AppError {
    AppError::NotNvidia(Box::new(
        Failure::new("the fake adapter is not an NVIDIA adapter").with_context(ErrorContext {
            adapter: Some(adapter.to_owned()),
            ..ErrorContext::default()
        }),
    ))
}

impl VendorExtras for FakeVendor {
    fn adapters(&self) -> Result<Vec<GpuInfo>, AppError> {
        self.log.record(VendorCall::Adapters);
        self.fault(Op::Adapters, "adapter enumeration")?;
        let mut adapters: Vec<GpuInfo> = Vec::new();
        for monitor in &self.monitors {
            if adapters.iter().any(|gpu| gpu.name == monitor.adapter_name) {
                continue;
            }
            adapters.push(gpu_info(self.profile, monitor));
        }
        Ok(adapters)
    }

    fn nvidia_adapter(&self, monitor: &MonitorId) -> Result<NvidiaAdapter, AppError> {
        self.log.record(VendorCall::NvidiaAdapter(monitor.clone()));
        let found = self.connected(monitor)?;
        if vendor_of(&found.adapter_instance_id) != GpuVendor::Nvidia {
            return Err(not_nvidia(&found.adapter_name));
        }
        let property = format!(r"{DISPLAY_CLASS}\{TEST_PC_NVIDIA_KEY}");
        match (
            InstanceId::from_enumeration(&found.adapter_instance_id),
            AdapterKey::from_driver_property(&property),
        ) {
            (Some(instance_id), Some(class_key)) => Ok(NvidiaAdapter {
                name: found.adapter_name.clone(),
                instance_id,
                class_key,
            }),
            _ => Err(AppError::internal(
                "the fake fixture holds an adapter value the enumeration refuses",
            )),
        }
    }

    fn custom_displays(&self) -> Result<Vec<CustomEntry>, AppError> {
        self.log.record(VendorCall::CustomDisplays);
        if !self.has_nvidia() {
            let adapter = self
                .monitors
                .first()
                .map_or("", |monitor| monitor.adapter_name.as_str());
            return Err(not_nvidia(adapter));
        }
        self.fault(Op::CustomDisplays, "NvAPI_DISP_EnumCustomDisplay")?;
        Ok(vec![CustomEntry {
            // Synthetic: the opaque ID the listing hands out.
            id: String::from("fake-nvapi-custom-display-1"),
            source: CustomSource::NvapiCustomDisplay,
            origin: Origin::Other,
            width: 1280,
            height: 960,
            refresh: Some(to_refresh(Rational::hz(240))),
            // The text the CustomDisplay value on the test PC holds, read with reg query on
            // 11 Nov 2026; its source size there is 1280 x 882.
            raw: String::from("CUST:1280x960x240.000Hz"),
            adapter: String::from("NVIDIA GeForce RTX 4060 Ti"),
            added_ms: None,
        }])
    }

    fn try_custom_display(
        &self,
        ticket: &Ticket,
        monitor: &MonitorId,
        mode: RawMode,
    ) -> Result<(), AppError> {
        // A change with no open journal entry is a pipeline bug (pc_changes_need_ticket).
        self.journal.require_open(ticket);
        self.log.record(VendorCall::TryCustomDisplay {
            change_id: ticket.change_id().to_owned(),
            monitor: monitor.clone(),
            mode,
        });
        let found = self.connected(monitor)?;
        if vendor_of(&found.adapter_instance_id) != GpuVendor::Nvidia {
            return Err(not_nvidia(&found.adapter_name));
        }
        self.fault(Op::Try, "NvAPI_DISP_TryCustomDisplay")?;
        *lock(&self.trial) = Some((monitor.clone(), mode));
        Ok(())
    }
}

/// The fixture's view of one adapter. Driver versions as recorded for the test PC on
/// 11 Nov 2026; the adapter of the not-nvidia profile is synthetic.
fn gpu_info(profile: FixtureProfile, monitor: &RawMonitor) -> GpuInfo {
    let vendor = vendor_of(&monitor.adapter_instance_id);
    let driver_version = match (profile, vendor) {
        (FixtureProfile::NotNvidia, _) => None,
        (_, GpuVendor::Nvidia) => Some(String::from("32.0.16.2101")),
        (_, GpuVendor::Other) => Some(String::from("0.45.0.0")),
    };
    GpuInfo {
        name: monitor.adapter_name.clone(),
        vendor,
        driver_version,
        nvapi: match vendor {
            GpuVendor::Nvidia => NvapiStatus::Available,
            GpuVendor::Other => NvapiStatus::NotApplicable,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::ErrorCode;

    fn lg(vendor: &FakeVendor) -> MonitorId {
        vendor.monitors[0].id.clone()
    }

    fn stretched() -> RawMode {
        RawMode {
            width: 1280,
            height: 960,
            refresh: Rational::hz(240),
        }
    }

    #[test]
    fn req_disp_01_vendor_call_log_records_calls() {
        let journal = OpenEntries::default();
        let vendor = FakeVendor::new(FixtureProfile::TestPc, journal.clone());
        let monitor = lg(&vendor);
        vendor.adapters().unwrap();
        vendor.custom_displays().unwrap();
        let ticket = journal.open_for_tests("change-1");
        vendor
            .try_custom_display(&ticket, &monitor, stretched())
            .unwrap();
        assert_eq!(
            vendor.calls(),
            vec![
                VendorCall::Adapters,
                VendorCall::CustomDisplays,
                VendorCall::TryCustomDisplay {
                    change_id: String::from("change-1"),
                    monitor: monitor.clone(),
                    mode: stretched(),
                },
            ]
        );
        assert_eq!(vendor.trial(), Some((monitor, stretched())));
        assert!(vendor.changed_anything());
    }

    #[test]
    #[should_panic(expected = "has no open journal entry")]
    fn req_disp_01_vendor_change_needs_ticket() {
        let journal = OpenEntries::default();
        let vendor = FakeVendor::new(FixtureProfile::TestPc, journal.clone());
        let monitor = lg(&vendor);
        let ticket = journal.open_for_tests("change-1");
        journal.close_for_tests(&ticket);
        let _ = vendor.try_custom_display(&ticket, &monitor, stretched());
    }

    #[test]
    fn req_nv_01_adapters_and_vendor_check() {
        let vendor = FakeVendor::new(FixtureProfile::TestPc, OpenEntries::default());
        let adapters = vendor.adapters().unwrap();
        assert_eq!(adapters.len(), 2);
        assert_eq!(adapters[0].vendor, GpuVendor::Nvidia);
        assert_eq!(adapters[1].nvapi, NvapiStatus::NotApplicable);

        let nvidia = vendor.nvidia_adapter(&lg(&vendor)).unwrap();
        assert_eq!(nvidia.class_key.as_str(), "0001");
        assert!(nvidia.instance_id.as_str().starts_with(r"PCI\VEN_10DE&"));
        // The Parsec virtual display is never treated as NVIDIA.
        let parsec = vendor.monitors[1].id.clone();
        let refused = vendor.nvidia_adapter(&parsec).unwrap_err();
        assert_eq!(refused.code(), ErrorCode::NotNvidia);

        let other = FakeVendor::new(FixtureProfile::NotNvidia, OpenEntries::default());
        let refused = other.custom_displays().unwrap_err();
        assert_eq!(refused.code(), ErrorCode::NotNvidia);
    }

    #[test]
    fn req_nv_09_scripted_vendor_faults() {
        let journal = OpenEntries::default();
        let vendor = FakeVendor::new(FixtureProfile::TestPc, journal.clone());
        let monitor = lg(&vendor);
        vendor.script(VendorFault::CallFails(-137), When::Once);
        let failed = vendor.custom_displays().unwrap_err();
        assert_eq!(failed.code(), ErrorCode::NvapiCallFailed);
        assert_eq!(failed.to_ipc().context.nvapi_status, Some(-137));

        vendor.script(VendorFault::TimingRefused(-187), When::Once);
        let ticket = journal.open_for_tests("change-1");
        let refused = vendor
            .try_custom_display(&ticket, &monitor, stretched())
            .unwrap_err();
        assert_eq!(refused.code(), ErrorCode::CustomTimingRefused);
        assert_eq!(vendor.trial(), None);

        vendor.script(VendorFault::NvapiMissing, When::Always);
        assert_eq!(
            vendor.custom_displays().unwrap_err().code(),
            ErrorCode::NvapiMissing
        );
        vendor.script(VendorFault::AdaptersFail, When::Once);
        assert_eq!(vendor.adapters().unwrap_err().code(), ErrorCode::Internal);
    }
}
