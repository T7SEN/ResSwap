//! The fake registry reader: the display class subkeys of the fixture in memory, a call log
//! and scripted faults. It reads no registry, so no test depends on the PC's values
//! (REQ-DISP-01).

#![forbid(unsafe_code)]

use std::collections::BTreeMap;
use std::sync::Mutex;

use crate::display::fake::{CallLog, FaultScript, FixtureProfile, When, lock};
use crate::error::{AppError, Failure};
use crate::vendor::AdapterKey;
use crate::vendor::nvidia::nv_modes::{
    NvModesValue, RawKind, RawValue, RegistryReader, check_nv_modes,
};

/// NV_Modes on the test PC's NVIDIA adapter, subkey 0001, byte for byte as read with
/// reg query on 11 Nov 2026: REG_MULTI_SZ holding this one string of 404 characters. The
/// last group was added by an earlier tool, not by the driver.
pub const TEST_PC_NV_MODES: &str = "{*}S 640x480x32,64 720x480x32,64 800x600x32,64 1024x768x32,64 1152x864x32,64 1280x720x32,64 1280x768x32,64 1280x800x32,64 1280x960x32,64 1280x1024x32,64 1360x768x32,64 1366x768x32,64 1440x1080x32,64 1600x900x32,64 1600x1024x32,64 1600x1200x32,64 1680x1050x32,64 1920x1080x32,64 1920x1200x32,64 1920x1440x32,64 2048x1536x32,64 2560x1440x32,64 2560x1600x32,64=1; 720x576x32,64=8032; 1280x882x8,16,32,64=1F;";

/// A failure a test scripts into the fake.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RegistryFault {
    /// Every read fails (INJ-E01, INJ-E14): REGISTRY_READ_FAILED.
    ReadFails,
}

/// One call the fake received, in order, with the subkey it read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RegistryCall {
    NvModes(String),
    MatchingDeviceId(String),
}

#[derive(Clone, Debug, Default)]
struct ClassKey {
    nv_modes: Option<RawValue>,
    matching_device_id: Option<String>,
}

/// The registry reader on the fixture's display class subkeys, for every test and every
/// fake-backend run.
pub struct FakeRegistry {
    keys: Mutex<BTreeMap<String, ClassKey>>,
    log: CallLog<RegistryCall>,
    script: FaultScript<RegistryFault>,
}

impl FakeRegistry {
    /// The fake for `profile`.
    pub fn new(profile: FixtureProfile) -> Self {
        Self {
            keys: Mutex::new(fixture(profile)),
            log: CallLog::default(),
            script: FaultScript::default(),
        }
    }

    /// Adds a fault to the script.
    pub fn script(&self, fault: RegistryFault, when: When) {
        self.script.push(fault, when);
    }

    /// Every call so far, in order.
    pub fn calls(&self) -> Vec<RegistryCall> {
        self.log.calls()
    }

    /// Sets or removes NV_Modes under the subkey with these four digits, as an elevated
    /// write would. Tests use it to place a damaged value; the runner fake uses it for
    /// reg.exe.
    pub fn set_nv_modes(&self, key: &str, value: Option<RawValue>) {
        lock(&self.keys).entry(key.to_owned()).or_default().nv_modes = value;
    }

    fn fault(&self) -> Result<(), AppError> {
        match self.script.due(|_| true) {
            Some(RegistryFault::ReadFails) => Err(AppError::RegistryReadFailed(Box::new(
                Failure::new("scripted fault: the fake registry read failed"),
            ))),
            None => Ok(()),
        }
    }
}

impl RegistryReader for FakeRegistry {
    fn nv_modes(&self, key: &AdapterKey) -> Result<Option<NvModesValue>, AppError> {
        self.log
            .record(RegistryCall::NvModes(key.as_str().to_owned()));
        self.fault()?;
        let raw = lock(&self.keys)
            .get(key.as_str())
            .and_then(|class_key| class_key.nv_modes.clone());
        raw.map(|raw| check_nv_modes(&raw)).transpose()
    }

    fn matching_device_id(&self, key: &AdapterKey) -> Result<Option<String>, AppError> {
        self.log
            .record(RegistryCall::MatchingDeviceId(key.as_str().to_owned()));
        self.fault()?;
        Ok(lock(&self.keys)
            .get(key.as_str())
            .and_then(|class_key| class_key.matching_device_id.clone()))
    }
}

fn fixture(profile: FixtureProfile) -> BTreeMap<String, ClassKey> {
    let mut keys = BTreeMap::new();
    match profile {
        FixtureProfile::TestPc | FixtureProfile::TwoMonitors => {
            // Parsec's adapter: no NV_Modes value, as read on the test PC.
            keys.insert(
                String::from("0000"),
                ClassKey {
                    nv_modes: None,
                    // Synthetic until the NVIDIA work reads MatchingDeviceId on the test PC.
                    matching_device_id: Some(String::from(r"root\display")),
                },
            );
            keys.insert(
                String::from("0001"),
                ClassKey {
                    nv_modes: Some(RawValue {
                        kind: RawKind::MultiSz,
                        strings: vec![String::from(TEST_PC_NV_MODES)],
                    }),
                    // Synthetic until the NVIDIA work reads MatchingDeviceId on the test PC.
                    matching_device_id: Some(String::from(r"pci\ven_10de&dev_2803")),
                },
            );
        }
        FixtureProfile::NotNvidia => {
            // Synthetic: one adapter that is not NVIDIA's, with no NV_Modes value.
            keys.insert(
                String::from("0000"),
                ClassKey {
                    nv_modes: None,
                    matching_device_id: Some(String::from(r"pci\ven_0000&dev_0000")),
                },
            );
        }
    }
    keys
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::ErrorCode;

    fn key(digits: &str) -> AdapterKey {
        AdapterKey::from_driver_property(&format!(r"{}\{digits}", crate::vendor::DISPLAY_CLASS))
            .unwrap()
    }

    #[test]
    fn req_nv_13_test_pc_value_byte_for_byte() {
        assert_eq!(TEST_PC_NV_MODES.len(), 404);
        let registry = FakeRegistry::new(FixtureProfile::TestPc);
        let value = registry.nv_modes(&key("0001")).unwrap().unwrap();
        assert_eq!(value.data.as_str(), TEST_PC_NV_MODES);
        assert_eq!(value.kind.reg_type(), "REG_MULTI_SZ");
        // Parsec's key has no NV_Modes value.
        assert_eq!(registry.nv_modes(&key("0000")).unwrap(), None);
    }

    #[test]
    fn req_disp_01_registry_call_log_records_calls() {
        let registry = FakeRegistry::new(FixtureProfile::TestPc);
        registry.nv_modes(&key("0001")).unwrap();
        registry.matching_device_id(&key("0000")).unwrap();
        assert_eq!(
            registry.calls(),
            vec![
                RegistryCall::NvModes(String::from("0001")),
                RegistryCall::MatchingDeviceId(String::from("0000")),
            ]
        );
    }

    // Also INJ-E01, INJ-E14.
    #[test]
    fn req_nv_14_inj_e02_scripted_registry_faults() {
        let registry = FakeRegistry::new(FixtureProfile::TestPc);
        registry.script(RegistryFault::ReadFails, When::Once);
        let failed = registry.nv_modes(&key("0001")).unwrap_err();
        assert_eq!(failed.code(), ErrorCode::RegistryReadFailed);

        registry.set_nv_modes(
            "0001",
            Some(RawValue {
                kind: RawKind::MultiSz,
                strings: vec![String::from("{*}S 640x480x32,64=1;\"")],
            }),
        );
        let damaged = registry.nv_modes(&key("0001")).unwrap_err();
        assert_eq!(damaged.code(), ErrorCode::NvModesParseFailed);
    }
}
