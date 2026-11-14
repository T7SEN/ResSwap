//! The clock behind the revert countdown, the watcher interval and the restore delay, behind
//! a trait so the fakes can hold time still.

#![forbid(unsafe_code)]

use std::time::{Instant, SystemTime, UNIX_EPOCH};

/// Every time reading the core makes.
pub trait Clock: Send + Sync {
    /// Milliseconds since this clock started. Never goes back, so a change to the Windows
    /// clock cannot bend a timer.
    fn mono_ms(&self) -> u64;

    /// Milliseconds since the Unix epoch, for times shown to the player.
    fn epoch_ms(&self) -> u64;

    /// Whether time moves by itself, so a timer thread can wait for it. False only for the
    /// still fake clock of the tests, whose timers run when a test moves the clock.
    fn follows_real_time(&self) -> bool {
        true
    }
}

/// The system clock.
#[derive(Debug)]
pub struct SystemClock {
    started: Instant,
}

impl SystemClock {
    /// A clock whose monotonic time starts now.
    pub fn new() -> Self {
        Self {
            started: Instant::now(),
        }
    }
}

impl Default for SystemClock {
    fn default() -> Self {
        Self::new()
    }
}

impl Clock for SystemClock {
    fn mono_ms(&self) -> u64 {
        u64::try_from(self.started.elapsed().as_millis()).unwrap_or(u64::MAX)
    }

    fn epoch_ms(&self) -> u64 {
        epoch_ms_now()
    }
}

/// Milliseconds since the Unix epoch from the system clock; 0 when the clock reads a time
/// before 1970.
pub fn epoch_ms_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|since| u64::try_from(since.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn req_disp_01_system_clock_moves_forward() {
        let clock = SystemClock::new();
        let first = clock.mono_ms();
        assert!(clock.mono_ms() >= first);
        // 1 Jan 2026 00:00:00 UTC: any PC running these tests is later.
        assert!(clock.epoch_ms() > 1_767_225_600_000);
    }
}
