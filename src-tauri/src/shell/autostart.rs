//! Start with Windows: one REG_SZ value named ResSwap under the HKCU Run key, holding the
//! quoted exe path and `--autostart`; never HKLM (REQ-SHELL-07, REQ-SHELL-08, D-003).

use crate::apply::Ticket;
use crate::error::AppError;

/// The value name under HKCU\Software\Microsoft\Windows\CurrentVersion\Run.
pub const RUN_VALUE_NAME: &str = "ResSwap";

/// Every read and write of ResSwap's Run value. Writes take a Ticket, so they run only after
/// their journal entry is on disk.
pub trait RunKey: Send + Sync {
    /// The data of the value ResSwap, `None` when it is absent. Changes nothing.
    fn read(&self) -> Result<Option<String>, AppError>;

    /// Writes the value ResSwap with `data`. Changes the PC.
    fn write(&self, ticket: &Ticket, data: &str) -> Result<(), AppError>;

    /// Deletes the value ResSwap; a value that is already absent counts as success. Changes
    /// the PC.
    fn delete(&self, ticket: &Ticket) -> Result<(), AppError>;
}
