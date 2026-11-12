//! The Win32 display backend for the player's PC. This build reads nothing and changes
//! nothing through it: every call returns INTERNAL with the reason in the detail, and the
//! CCD and GDI routes replace these bodies with the display core.

pub mod ccd;
pub mod gdi;

use crate::apply::Ticket;
use crate::display::{DisplayBackend, DisplayConfig, RawMonitor};
use crate::error::AppError;

/// Why every call fails in this build.
const NOT_IN_THIS_BUILD: &str =
    "the Win32 display backend is not part of this build; it read nothing and changed nothing";

/// The display backend on the player's PC; in this build every call answers INTERNAL.
#[derive(Debug, Default)]
pub struct Win32Display;

impl DisplayBackend for Win32Display {
    fn enumerate(&self) -> Result<Vec<RawMonitor>, AppError> {
        Err(AppError::internal(NOT_IN_THIS_BUILD))
    }

    fn test_config(&self, _config: &DisplayConfig) -> Result<(), AppError> {
        Err(AppError::internal(NOT_IN_THIS_BUILD))
    }

    fn apply_config(&self, _ticket: &Ticket, _config: &DisplayConfig) -> Result<(), AppError> {
        Err(AppError::internal(NOT_IN_THIS_BUILD))
    }
}
