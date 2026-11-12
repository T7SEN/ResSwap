//! Every display call ResSwap makes goes through `DisplayBackend` (REQ-DISP-01): the Win32
//! backend for the player's PC, the fake for every test and every fake-backend run.

pub mod fake;
pub mod modes;
pub mod monitors;
pub mod types;
pub mod win32;

use crate::apply::Ticket;
use crate::display::types::{MonitorId, Scaling, ScalingChoice};
use crate::error::AppError;

/// An exact rate as Windows reports it, before the core builds its label.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
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
    /// The EDID friendly name, when Windows reports one.
    pub name: Option<String>,
    pub adapter_name: String,
    /// The adapter's device instance ID, for example one that starts with `PCI\VEN_10DE&`.
    pub adapter_instance_id: String,
    /// The CCD outputTechnology value.
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

/// Every display call ResSwap makes. Reads take no Ticket. A call that changes the PC takes
/// one, so it cannot run before its journal entry is on disk (REQ-SAFE-01).
pub trait DisplayBackend: Send + Sync {
    /// Reads every active monitor. Changes nothing.
    fn enumerate(&self) -> Result<Vec<RawMonitor>, AppError>;
    /// Asks Windows whether `config` would apply (CDS_TEST or SDC_VALIDATE). Changes nothing.
    fn test_config(&self, config: &DisplayConfig) -> Result<(), AppError>;
    /// Applies `config` as a temporary change. Changes the PC.
    fn apply_config(&self, ticket: &Ticket, config: &DisplayConfig) -> Result<(), AppError>;
}
