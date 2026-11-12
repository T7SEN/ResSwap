//! The fake clock and the fake process list: time that moves only when a test moves it, and
//! a process list that holds only the exe names a test adds. Neither reads the PC
//! (REQ-DISP-01).

#![forbid(unsafe_code)]

use std::sync::Mutex;

use crate::display::fake::{CallLog, FaultScript, When, lock};
use crate::error::AppError;
use crate::system::clock::Clock;
use crate::system::processes::{ProcessEntry, ProcessList};

/// Where the fake clock's epoch time starts: 11 Nov 2026 00:00:00 UTC. Synthetic: any fixed
/// time serves, and this one matches the day the fixtures were recorded.
pub const FAKE_EPOCH_MS: u64 = 1_794_355_200_000;

/// One reading or step of the fake clock, in order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClockCall {
    Mono,
    Epoch,
    Advance(u64),
}

/// A clock that stands still until a test advances it, so countdown, restore delay and
/// watcher tests take no real time.
pub struct FakeClock {
    elapsed_ms: Mutex<u64>,
    log: CallLog<ClockCall>,
}

impl FakeClock {
    /// A clock at monotonic time 0 and epoch time `FAKE_EPOCH_MS`.
    pub fn new() -> Self {
        Self {
            elapsed_ms: Mutex::new(0),
            log: CallLog::default(),
        }
    }

    /// Moves time forward by `ms`.
    pub fn advance(&self, ms: u64) {
        self.log.record(ClockCall::Advance(ms));
        let mut elapsed = lock(&self.elapsed_ms);
        *elapsed = elapsed.saturating_add(ms);
    }

    /// Every reading and step so far, in order.
    pub fn calls(&self) -> Vec<ClockCall> {
        self.log.calls()
    }
}

impl Default for FakeClock {
    fn default() -> Self {
        Self::new()
    }
}

impl Clock for FakeClock {
    fn mono_ms(&self) -> u64 {
        self.log.record(ClockCall::Mono);
        *lock(&self.elapsed_ms)
    }

    fn epoch_ms(&self) -> u64 {
        self.log.record(ClockCall::Epoch);
        FAKE_EPOCH_MS.saturating_add(*lock(&self.elapsed_ms))
    }
}

/// A failure a test scripts into the fake process list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProcessFault {
    /// The snapshot fails: INTERNAL, and the watcher keeps its state.
    SnapshotFails,
}

/// One call the fake received.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProcessCall {
    Snapshot,
}

struct Processes {
    entries: Vec<ProcessEntry>,
    next_pid: u32,
}

/// A process list that starts empty; tests add and remove exe names.
pub struct FakeProcessList {
    processes: Mutex<Processes>,
    log: CallLog<ProcessCall>,
    script: FaultScript<ProcessFault>,
}

impl FakeProcessList {
    /// An empty list, as every fixture profile starts.
    pub fn new() -> Self {
        Self {
            processes: Mutex::new(Processes {
                entries: Vec::new(),
                // Synthetic process IDs, counted up from here.
                next_pid: 1000,
            }),
            log: CallLog::default(),
            script: FaultScript::default(),
        }
    }

    /// Adds a process named `exe` and returns its process ID.
    pub fn add(&self, exe: &str) -> u32 {
        let mut processes = lock(&self.processes);
        let pid = processes.next_pid;
        processes.next_pid = pid.saturating_add(4);
        processes.entries.push(ProcessEntry {
            pid,
            exe: exe.to_owned(),
        });
        pid
    }

    /// Removes the process with `pid`, as if it exited.
    pub fn remove(&self, pid: u32) {
        lock(&self.processes)
            .entries
            .retain(|entry| entry.pid != pid);
    }

    /// Adds a fault to the script.
    pub fn script(&self, fault: ProcessFault, when: When) {
        self.script.push(fault, when);
    }

    /// Every call so far, in order.
    pub fn calls(&self) -> Vec<ProcessCall> {
        self.log.calls()
    }
}

impl Default for FakeProcessList {
    fn default() -> Self {
        Self::new()
    }
}

impl ProcessList for FakeProcessList {
    fn snapshot(&self) -> Result<Vec<ProcessEntry>, AppError> {
        self.log.record(ProcessCall::Snapshot);
        if let Some(ProcessFault::SnapshotFails) = self.script.due(|_| true) {
            return Err(AppError::internal(
                "scripted fault: the fake process snapshot failed",
            ));
        }
        Ok(lock(&self.processes).entries.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn req_disp_01_clock_call_log_records_calls() {
        let clock = FakeClock::new();
        assert_eq!(clock.mono_ms(), 0);
        clock.advance(10_000);
        assert_eq!(clock.mono_ms(), 10_000);
        assert_eq!(clock.epoch_ms(), FAKE_EPOCH_MS + 10_000);
        assert_eq!(
            clock.calls(),
            vec![
                ClockCall::Mono,
                ClockCall::Advance(10_000),
                ClockCall::Mono,
                ClockCall::Epoch,
            ]
        );
    }

    #[test]
    fn req_watch_03_process_call_log_records_calls() {
        let list = FakeProcessList::new();
        assert_eq!(list.snapshot().unwrap(), Vec::new());
        let pid = list.add("game.exe");
        assert_eq!(
            list.snapshot().unwrap(),
            vec![ProcessEntry {
                pid,
                exe: String::from("game.exe"),
            }]
        );
        list.script(ProcessFault::SnapshotFails, When::Once);
        assert!(list.snapshot().is_err());
        list.remove(pid);
        assert!(list.snapshot().unwrap().is_empty());
        assert_eq!(list.calls(), vec![ProcessCall::Snapshot; 4]);
    }
}
