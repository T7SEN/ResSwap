//! Every display call ResSwap makes goes through `DisplayBackend` (REQ-DISP-01): the Win32
//! backend for the player's PC, the fake for every test and every fake-backend run.

pub mod fake;
pub mod modes;
pub mod monitors;
pub mod types;
pub mod win32;

use serde::{Deserialize, Serialize};

use crate::apply::Ticket;
use crate::apply::journal::LayoutRecord;
use crate::display::types::{MonitorId, Scaling, ScalingChoice};
use crate::error::AppError;

/// The apply route. The HW-P1 round picks the default; every journal entry records the
/// route its change used, so a revert takes the same one (REQ-DISP-14).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Route {
    /// SetDisplayConfig with SDC_APPLY and SDC_USE_SUPPLIED_DISPLAY_CONFIG.
    Ccd,
    /// ChangeDisplaySettingsExW.
    Gdi,
}

/// The temporary flag of the GDI route, picked by the HW-P1 round. Never
/// CDS_UPDATEREGISTRY, CDS_GLOBAL or CDS_NORESET (REQ-DISP-14).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum GdiFlag {
    /// CDS_FULLSCREEN.
    Fullscreen,
    /// dwflags 0.
    Zero,
}

/// The route and flag a backend applies with. `gdi_flag` is `Some` exactly when `route` is
/// `Gdi`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RouteChoice {
    pub route: Route,
    pub gdi_flag: Option<GdiFlag>,
}

impl RouteChoice {
    /// SetDisplayConfig with SDC_APPLY and SDC_USE_SUPPLIED_DISPLAY_CONFIG; no GDI flag.
    pub const fn ccd() -> Self {
        Self {
            route: Route::Ccd,
            gdi_flag: None,
        }
    }

    /// ChangeDisplaySettingsExW with `flag`.
    pub const fn gdi(flag: GdiFlag) -> Self {
        Self {
            route: Route::Gdi,
            gdi_flag: Some(flag),
        }
    }

    /// Whether the flag fits the route: a GDI flag with the GDI route and none with CCD. A
    /// choice that fails this never reaches a backend. Neither form can express a registry
    /// or database flag, so every change stays temporary (REQ-DISP-14).
    pub const fn is_consistent(self) -> bool {
        matches!(
            (self.route, self.gdi_flag),
            (Route::Ccd, None) | (Route::Gdi, Some(_))
        )
    }
}

/// An exact rate as Windows reports it, before the core builds its label.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Rational {
    pub numerator: u32,
    pub denominator: u32,
}

impl Rational {
    /// A whole number of hertz, as a GDI-only rate is stored.
    pub const fn hz(hz: u32) -> Self {
        Self {
            numerator: hz,
            denominator: 1,
        }
    }
}

/// A mode as a backend reports it: sizes in physical pixels and the exact rate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RawMode {
    pub width: u32,
    pub height: u32,
    pub refresh: Rational,
}

/// One monitor as a backend reads it. Plain data, never a Win32 struct, so the core and the
/// fake share it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RawMonitor {
    pub id: MonitorId,
    /// The EDID friendly name, when Windows reports one (friendlyNameFromEdid set). The core
    /// turns an empty or blank name into `None` as well (REQ-DISP-03).
    pub name: Option<String>,
    pub adapter_name: String,
    /// The adapter's device instance ID, for example one that starts with `PCI\VEN_10DE&`.
    pub adapter_instance_id: String,
    /// DISPLAYCONFIG_ADAPTER_NAME.adapterDevicePath. The journal keeps it to map a saved
    /// adapter LUID to this boot's LUID (REQ-SAFE-02; LUIDs last one boot).
    pub adapter_device_path: String,
    /// The GDI device name of the source, for example `\\.\DISPLAY1`. For the GDI route and
    /// the log only, never for matching a monitor (REQ-DISP-20).
    pub gdi_name: String,
    /// Desktop position of the source's top-left corner.
    pub position_x: i32,
    pub position_y: i32,
    /// The CCD outputTechnology value, widened from its i32 with `i64::from`, so the
    /// constants in display/monitors.rs compare with it (REQ-DISP-07).
    pub output_technology: i64,
    pub primary: bool,
    pub active: bool,
    pub native: Option<RawMode>,
    pub current: Option<RawMode>,
    pub scaling: Scaling,
    pub modes: Vec<RawMode>,
}

/// What one monitor should show after a change.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TargetConfig {
    pub monitor: MonitorId,
    pub mode: RawMode,
    pub scaling: ScalingChoice,
}

/// A display configuration to test or apply. The layout fields join it with the
/// multi-monitor work.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DisplayConfig {
    pub targets: Vec<TargetConfig>,
}

/// What a snapshot restore did: the saved targets it left out because their monitor is not
/// connected, or its path reports targetAvailable false (REQ-SAFE-09).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SnapshotRestore {
    pub left_out: Vec<MonitorId>,
}

/// Every display call ResSwap makes. Reads take no Ticket. A call that changes the PC takes
/// one, so it cannot run before its journal entry is on disk (REQ-SAFE-01).
pub trait DisplayBackend: Send + Sync {
    /// Reads every active monitor. Changes nothing. A target whose targetAvailable is false
    /// is left out, as an unplugged monitor is, so the core reports it as MONITOR_GONE
    /// (REQ-SAFE-09). The native mode is the preferred mode from
    /// DISPLAYCONFIG_DEVICE_INFO_GET_TARGET_PREFERRED_MODE, or `None` (REQ-DISP-04).
    fn enumerate(&self) -> Result<Vec<RawMonitor>, AppError>;
    /// The route and GDI flag `apply_config` uses, so the journal records it and a revert
    /// takes the same one. Changes nothing.
    fn route(&self) -> RouteChoice;
    /// Asks Windows whether `config` would apply (CDS_TEST or SDC_VALIDATE). Changes nothing.
    fn test_config(&self, config: &DisplayConfig) -> Result<(), AppError>;
    /// Applies `config` as a temporary change. Changes the PC.
    fn apply_config(&self, ticket: &Ticket, config: &DisplayConfig) -> Result<(), AppError>;
    /// SetDisplayConfig with SDC_APPLY and SDC_USE_DATABASE_CURRENT: puts back the
    /// configuration Windows keeps for the connected monitors, the last fallback of Restore
    /// everything (REQ-DISP-22). Changes the PC.
    fn apply_database_current(&self, ticket: &Ticket) -> Result<(), AppError>;
    /// The previous state the journal keeps (REQ-SAFE-02): the path and mode arrays of one
    /// QueryDisplayConfig call, each adapter's device path, and the identity of each path's
    /// target, in path order. Changes nothing.
    fn snapshot(&self) -> Result<LayoutRecord, AppError>;
    /// Applies a saved snapshot with SDC_APPLY and SDC_USE_SUPPLIED_DISPLAY_CONFIG, after
    /// mapping each saved adapter LUID to this boot's LUID by its device path and leaving out
    /// every target whose monitor is not connected or reports targetAvailable false
    /// (REQ-DISP-22). An error from the call is MODE_REJECTED. Changes the PC.
    fn restore_snapshot(
        &self,
        ticket: &Ticket,
        snapshot: &LayoutRecord,
    ) -> Result<SnapshotRestore, AppError>;
}
