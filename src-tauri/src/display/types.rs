//! Monitor identity, modes and scaling, shared by the core and the window.

#![forbid(unsafe_code)]

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// How ResSwap recognises a monitor across starts. Match on the device path first, then on
/// the EDID ids plus the connector instance; never on an adapter LUID or a GDI device name.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MonitorId {
    /// DISPLAYCONFIG_TARGET_DEVICE_NAME.monitorDevicePath.
    pub device_path: String,
    /// edidManufactureId; null unless the edidIdsValid flag is set.
    pub edid_manufacturer_id: Option<u16>,
    /// edidProductCodeId; null unless the edidIdsValid flag is set.
    pub edid_product_code: Option<u16>,
    /// connectorInstance; separates two identical monitors on one adapter.
    pub connector_instance: u32,
}

/// Exact refresh rate as Windows reports it (the target mode's vSyncFreq rational). A rate
/// known only from GDI is stored as hz / 1.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RefreshRate {
    pub numerator: u32,
    pub denominator: u32,
    /// Built by the core: hertz with at most two decimals and no trailing zeros, for
    /// example "240 Hz" or "59.94 Hz".
    pub label: String,
}

/// One display mode: its size, its exact refresh rate and a label the core builds.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Mode {
    pub width: u32,
    pub height: u32,
    pub refresh: RefreshRate,
    /// Built by the core, for example "1280 x 960 at 240 Hz".
    pub label: String,
}

/// Scaling as read back from the CCD path (DISPLAYCONFIG_PATH_TARGET_INFO.scaling). The
/// variant names follow the DISPLAYCONFIG_SCALING_* constants, so "centered" keeps the API
/// spelling.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum Scaling {
    Identity,
    Centered,
    Stretched,
    AspectRatio,
    Custom,
    Preferred,
    Unknown,
}

/// Scaling a player can ask for. Full screen requests stretched scaling; the driver may
/// ignore the request, which is reported as SCALING_NOT_APPLIED.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum ScalingChoice {
    Keep,
    FullScreen,
}

/// The aspect ratio of a stretched preset (REQ-DISP-09).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum Aspect {
    FourThree,
    FiveFour,
    SixteenTen,
}

/// A stretched size computed from the native height (REQ-DISP-09).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct Preset {
    pub aspect: Aspect,
    /// At the monitor's current refresh rate.
    pub mode: Mode,
    /// False when Windows does not list this size at that rate; on NVIDIA it can be added
    /// with add_custom_mode.
    pub offered: bool,
}

/// The vendor of a monitor's adapter, read from its device instance ID (REQ-DISP-02).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum GpuVendor {
    Nvidia,
    Other,
}

/// Why a monitor sits behind the show-hidden toggle (REQ-DISP-07).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum HiddenReason {
    IndirectConnector,
    OtherConnector,
    VirtualAdapter,
}

/// One monitor as the window shows it, built by the core from what a backend read.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct Monitor {
    pub id: MonitorId,
    /// Friendly name from the EDID; null when Windows has none, and the window then shows
    /// its own fallback text.
    pub name: Option<String>,
    /// Adapter name, for example the GPU's name.
    pub adapter: String,
    pub vendor: GpuVendor,
    pub primary: bool,
    /// False when the monitor is connected but has no active path.
    pub active: bool,
    /// True for virtual displays (REQ-DISP-07).
    pub hidden: bool,
    pub hidden_reason: Option<HiddenReason>,
    /// From DISPLAYCONFIG_DEVICE_INFO_GET_TARGET_PREFERRED_MODE; null if not reported.
    pub native: Option<Mode>,
    /// Null when the monitor is not active.
    pub current: Option<Mode>,
    pub scaling: Scaling,
    /// Every mode Windows offers, without duplicates.
    pub modes: Vec<Mode>,
    pub presets: Vec<Preset>,
}
