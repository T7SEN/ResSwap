//! Vendor extras behind one interface. NVIDIA is the first; the display core never depends
//! on them (REQ-DISP-02). This file holds the `VendorExtras` trait, the adapter values the
//! enumeration reads, and the custom-entry types the window sees.

pub mod fake;
pub mod nvidia;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::apply::Ticket;
use crate::display::RawMode;
use crate::display::types::{MonitorId, RefreshRate};
use crate::error::{AppError, IpcError};
use crate::ipc::types::GpuInfo;

/// Where a custom resolution entry lives.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum CustomSource {
    NvModes,
    NvapiCustomDisplay,
}

/// Who added a custom resolution entry (REQ-NV-10).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum Origin {
    /// Listed in ResSwap's own record of what it added.
    Own,
    /// Everything else: other tools, NVIDIA Control Panel, the driver.
    Other,
}

/// One custom resolution entry as list_custom_entries shows it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct CustomEntry {
    /// Opaque; the core finds the entry again in a fresh listing before acting.
    pub id: String,
    pub source: CustomSource,
    pub origin: Origin,
    pub width: u32,
    pub height: u32,
    /// NVAPI custom displays carry a timing; NV_Modes groups do not.
    pub refresh: Option<RefreshRate>,
    /// The entry exactly as found, for the details view.
    pub raw: String,
    pub adapter: String,
    /// For own entries, from ResSwap's record (milliseconds since the Unix epoch).
    pub added_ms: Option<u64>,
}

/// Every custom resolution entry of the NVIDIA adapter, with the sources that failed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct CustomEntryList {
    pub adapter: Option<String>,
    pub entries: Vec<CustomEntry>,
    /// Sources that could not be read; one unreadable source never hides the other.
    pub problems: Vec<IpcError>,
}

/// The display class GUID that names every display adapter's driver key.
pub const DISPLAY_CLASS: &str = "{4d36e968-e325-11ce-bfc1-08002be10318}";

/// How an NVIDIA adapter's device instance ID starts (PCI vendor 10DE).
pub const NVIDIA_INSTANCE_PREFIX: &str = r"PCI\VEN_10DE&";

/// The four-digit display class subkey of an NVIDIA adapter, read fresh from that adapter's
/// own driver property. Never from settings, IPC, the journal or a constant, so the key of
/// another adapter is never written (REQ-NV-15, REQ-SEC-01).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AdapterKey(String);

impl AdapterKey {
    /// From a driver property of the form `{4d36e968-e325-11ce-bfc1-08002be10318}\NNNN`,
    /// with exactly four ASCII digits after the class; `None` for anything else.
    pub(in crate::vendor) fn from_driver_property(value: &str) -> Option<Self> {
        let (class, digits) = value.split_once('\\')?;
        (class.eq_ignore_ascii_case(DISPLAY_CLASS)
            && digits.len() == 4
            && digits.bytes().all(|byte| byte.is_ascii_digit()))
        .then(|| Self(digits.to_owned()))
    }

    /// The four digits, for example "0001".
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// The device instance ID of an NVIDIA adapter, from ResSwap's own device enumeration. It
/// starts with `PCI\VEN_10DE&`, holds only ASCII letters, digits, backslashes, ampersands and
/// underscores, and does not end with a backslash, so it can never carry a quote, a switch
/// or a second argument (REQ-SEC-01).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InstanceId(String);

impl InstanceId {
    /// From an ID the adapter enumeration read; `None` when it fails any rule above.
    pub(in crate::vendor) fn from_enumeration(id: &str) -> Option<Self> {
        let allowed =
            |byte: u8| byte.is_ascii_alphanumeric() || matches!(byte, b'\\' | b'&' | b'_');
        (id.starts_with(NVIDIA_INSTANCE_PREFIX) && !id.ends_with('\\') && id.bytes().all(allowed))
            .then(|| Self(id.to_owned()))
    }

    /// The ID as Windows lists it.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// The NVIDIA adapter that drives one monitor, as the adapter enumeration found it. Its
/// values are read again right before each use, never kept from an earlier read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NvidiaAdapter {
    /// The adapter's name, for messages.
    pub name: String,
    pub instance_id: InstanceId,
    pub class_key: AdapterKey,
}

/// Every vendor-specific call ResSwap makes. Reads take no Ticket; a call that changes the
/// PC takes one, so it cannot run before its journal entry is on disk (REQ-SAFE-01).
pub trait VendorExtras: Send + Sync {
    /// The graphics adapters and the state of each one's driver interface. Changes nothing.
    fn adapters(&self) -> Result<Vec<GpuInfo>, AppError>;

    /// The NVIDIA adapter that drives `monitor`: NOT_NVIDIA when another adapter drives it,
    /// MONITOR_GONE when no connected monitor has that identity (REQ-NV-01). Changes nothing.
    fn nvidia_adapter(&self, monitor: &MonitorId) -> Result<NvidiaAdapter, AppError>;

    /// The custom displays the NVIDIA driver lists. Changes nothing.
    fn custom_displays(&self) -> Result<Vec<CustomEntry>, AppError>;

    /// Shows `mode` on `monitor` as a custom display trial. Changes the PC.
    fn try_custom_display(
        &self,
        ticket: &Ticket,
        monitor: &MonitorId,
        mode: RawMode,
    ) -> Result<(), AppError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn req_nv_15_adapter_key_is_four_digits_of_the_display_class() {
        let key = AdapterKey::from_driver_property(r"{4d36e968-e325-11ce-bfc1-08002be10318}\0001");
        assert_eq!(key.map(|key| key.as_str().to_owned()), Some("0001".into()));
        for refused in [
            r"{4d36e968-e325-11ce-bfc1-08002be10318}\001",
            r"{4d36e968-e325-11ce-bfc1-08002be10318}\00010",
            r"{4d36e968-e325-11ce-bfc1-08002be10318}\00a1",
            r"{4d36e968-e325-11ce-bfc1-08002be10318}\0001\",
            r"{4d36e967-e325-11ce-bfc1-08002be10318}\0001",
            r"{4d36e968-e325-11ce-bfc1-08002be10318}\0001 /f",
            "0001",
            "",
        ] {
            assert!(
                AdapterKey::from_driver_property(refused).is_none(),
                "{refused}"
            );
        }
    }

    #[test]
    fn req_sec_01_inj_e04_instance_id_refuses_hostile_values() {
        let id = InstanceId::from_enumeration(r"PCI\VEN_10DE&DEV_2803&SUBSYS_0&REV_A1");
        assert!(id.is_some());
        for refused in [
            r"ROOT\DISPLAY\0000",
            r"PCI\VEN_1002&DEV_73BF",
            r"PCI\VEN_10DE&DEV_2803\",
            r#"PCI\VEN_10DE&DEV_2803" /f"#,
            r"PCI\VEN_10DE&DEV_2803%PATH%",
            "PCI\\VEN_10DE&DEV_2803\nX",
            "PCI\\VEN_10DE&DEV_2803\u{e9}",
            r"PCI\VEN_10DE&DEV_2803 extra",
            r"pci\ven_10de&dev_2803",
            "",
        ] {
            assert!(InstanceId::from_enumeration(refused).is_none(), "{refused}");
        }
    }
}
