//! The revert countdown for a mode that monitor has not confirmed (REQ-SAFE-03, REQ-SAFE-05).
//! It runs in Rust on the core's clock: one countdown-tick per second from
//! Settings.revertCountdownSecs down to 0, AppState.pending.secondsLeft kept in step, then the
//! revert with reason Timeout, whether or not the keep-mode window opened. A revert stops the
//! countdown only once it has written its reverting status: one that could not write it
//! leaves the countdown running, whose own revert is tried again one second later. On a clock
//! that moves by itself a thread waits for each tick; on the still fake clock of the tests
//! nothing runs until a test moves the clock and calls `run_due_timers`.

#![forbid(unsafe_code)]

use std::sync::atomic::Ordering;
use std::time::Duration;

use crate::error::ErrorCode;
use crate::ipc::events::emit_countdown_tick;
use crate::ipc::types::{CountdownTick, RevertReason};
use crate::{Core, guard};

use super::error_source_for;

/// The shortest wait of the countdown thread, so it never spins.
const MIN_WAIT_MS: u64 = 10;
/// The longest wait of the countdown thread, so a stopped countdown ends it within a second.
const MAX_WAIT_MS: u64 = 1_000;
/// How long after a revert that did not start, or while the PC suspends, the countdown's
/// revert is tried again.
const RETRY_MS: u64 = 1_000;

/// The countdown of the one pending change.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Countdown {
    change_id: String,
    /// The clock's monotonic time when the change became pending.
    started_ms: u64,
    total_secs: u32,
    /// The seconds of the last countdown-tick sent.
    last_sent: u32,
    /// When the revert is due once the tick with 0 is sent: the end of the countdown, or one
    /// second after a try that did not start.
    revert_due_ms: u64,
    /// The tries of the countdown's own revert that did not start.
    failed_reverts: u32,
}

impl Countdown {
    fn new(change_id: &str, started_ms: u64, total_secs: u32) -> Self {
        Self {
            change_id: change_id.to_owned(),
            started_ms,
            total_secs,
            last_sent: total_secs,
            revert_due_ms: started_ms.saturating_add(u64::from(total_secs).saturating_mul(1_000)),
            failed_reverts: 0,
        }
    }

    /// The whole seconds left at `now_ms`.
    fn seconds_left(&self, now_ms: u64) -> u32 {
        let elapsed = now_ms.saturating_sub(self.started_ms) / 1_000;
        self.total_secs
            .saturating_sub(u32::try_from(elapsed).unwrap_or(u32::MAX))
    }

    /// When the next tick, or after the tick with 0 the next try of the revert, is due, in
    /// the clock's monotonic time.
    fn next_due_ms(&self) -> u64 {
        if self.last_sent == 0 {
            return self.revert_due_ms;
        }
        let sent = u64::from(self.total_secs.saturating_sub(self.last_sent));
        self.started_ms
            .saturating_add(sent.saturating_add(1).saturating_mul(1_000))
    }
}

impl Core {
    /// Starts the countdown of `change_id`: the first tick at once, then one per second. On a
    /// clock that moves by itself a thread waits for each tick.
    pub(crate) fn start_countdown(&self, change_id: &str, secs: u32) {
        let countdown = Countdown::new(change_id, self.inner.clock.mono_ms(), secs);
        *guard(&self.inner.countdown) = Some(countdown);
        self.send_tick(change_id, secs);
        if self.inner.clock.follows_real_time() {
            let core = self.clone();
            let id = change_id.to_owned();
            let started = std::thread::Builder::new()
                .name(String::from("resswap-countdown"))
                .spawn(move || {
                    while let Some(wait) = core.countdown_wait_ms(&id) {
                        std::thread::sleep(Duration::from_millis(
                            wait.clamp(MIN_WAIT_MS, MAX_WAIT_MS),
                        ));
                        tauri::async_runtime::block_on(core.run_due_timers());
                    }
                });
            if let Err(error) = started {
                // Without the thread the countdown cannot end by itself; Restore everything
                // and Revert still work.
                log::error!("the countdown thread did not start: {error}");
            }
        }
    }

    /// Stops the countdown of `change_id`, if it runs.
    pub(crate) fn stop_countdown(&self, change_id: &str) {
        let mut countdown = guard(&self.inner.countdown);
        if countdown
            .as_ref()
            .is_some_and(|running| running.change_id == change_id)
        {
            *countdown = None;
        }
    }

    /// Moves the revert of `change_id`'s countdown to one second from now, when that
    /// countdown still runs. `failed` counts the move as a try that did not start. Returns
    /// how many tries have failed so far, or `None` once that countdown ended.
    pub(crate) fn retry_revert_later(&self, change_id: &str, failed: bool) -> Option<u32> {
        let now = self.inner.clock.mono_ms();
        let mut countdown = guard(&self.inner.countdown);
        let running = countdown
            .as_mut()
            .filter(|running| running.change_id == change_id)?;
        running.revert_due_ms = running.revert_due_ms.max(now.saturating_add(RETRY_MS));
        if failed {
            running.failed_reverts = running.failed_reverts.saturating_add(1);
        }
        Some(running.failed_reverts)
    }

    /// How long until the next tick of `change_id` is due; `None` once that countdown ended.
    fn countdown_wait_ms(&self, change_id: &str) -> Option<u64> {
        let countdown = guard(&self.inner.countdown);
        let running = countdown
            .as_ref()
            .filter(|running| running.change_id == change_id)?;
        Some(
            running
                .next_due_ms()
                .saturating_sub(self.inner.clock.mono_ms()),
        )
    }

    /// Sends one countdown-tick and keeps AppState.pending.secondsLeft in step. The seconds
    /// change without a new revision, so a late get_state shows them and the window keeps
    /// following the ticks.
    fn send_tick(&self, change_id: &str, seconds_left: u32) {
        if let Some(pending) = self
            .lock()
            .pending
            .as_mut()
            .filter(|pending| pending.change_id == change_id)
        {
            pending.seconds_left = seconds_left;
        }
        let sink = self.sink();
        if sink.has_listeners() {
            emit_countdown_tick(
                sink.as_ref(),
                &CountdownTick {
                    change_id: change_id.to_owned(),
                    seconds_left,
                },
            );
        }
    }

    /// Sends every tick that is due by the clock, in order, and returns the change ID once
    /// the tick with 0 seconds has been sent and the revert is due.
    fn send_due_ticks(&self) -> Option<String> {
        let now = self.inner.clock.mono_ms();
        let (change_id, ticks, finished) = {
            let mut countdown = guard(&self.inner.countdown);
            let running = countdown.as_mut()?;
            let left = running.seconds_left(now);
            let ticks: Vec<u32> = (left..running.last_sent).rev().collect();
            running.last_sent = running.last_sent.min(left);
            let finished = left == 0 && now >= running.revert_due_ms;
            (running.change_id.clone(), ticks, finished)
        };
        for seconds in ticks {
            self.send_tick(&change_id, seconds);
        }
        finished.then_some(change_id)
    }

    /// Runs the countdown work that is due: the ticks, then, after the tick with 0, the
    /// revert with reason Timeout (REQ-SAFE-05). While the PC is suspending the revert waits
    /// for the resume, which reverts at once. A revert that could not write its reverting
    /// status leaves the countdown running and is tried again one second later; the first
    /// such try is raised, and every one is logged. A put-back that failed is raised.
    pub async fn run_due_timers(&self) {
        let Some(change_id) = self.send_due_ticks() else {
            return;
        };
        if self.inner.suspended.load(Ordering::Relaxed) {
            // Checked again once a second, so the thread does not spin until the resume.
            self.retry_revert_later(&change_id, false);
            return;
        }
        let _lock = self.lock_changes_for(None).await;
        let source = self
            .lock()
            .pending
            .as_ref()
            .filter(|pending| pending.change_id == change_id)
            .map(|pending| pending.source);
        let Some(source) = source else {
            // The change ended some other way while this waited for the change lock.
            self.stop_countdown(&change_id);
            return;
        };
        if let Err(error) = self.revert_locked(&change_id, RevertReason::Timeout) {
            match self.retry_revert_later(&change_id, true) {
                // Already raised: the same error once a second would only repeat it.
                Some(tries) if tries > 1 => {}
                _ => self.raise(error_source_for(source), &error),
            }
        }
    }

    /// The player closed the keep-mode window with its title-bar button: the pending change
    /// goes back in Rust, reason KeepWindowClosed, with no command. Runs on the main thread's
    /// event handler, so the revert is queued on a task. When the reverting status cannot be
    /// written, the countdown still runs and reverts at its end; the window is not opened
    /// again, because the player closed it.
    pub fn keep_window_closed(&self) {
        let core = self.clone();
        tauri::async_runtime::spawn(async move {
            let _lock = core.lock_changes_for(None).await;
            let pending = core.lock().pending.clone();
            if let Some(pending) = pending
                && let Err(error) =
                    core.revert_locked(&pending.change_id, RevertReason::KeepWindowClosed)
            {
                core.raise(error_source_for(pending.source), &error);
            }
        });
    }

    /// Before the process exits: a pending change goes back, reason Quit. What Quit does with
    /// an active change waits for T7SEN (REQ-SHELL-15); until then active changes stay, and
    /// recovery at the next start finds them. No later second comes to try a revert again,
    /// so one that could not write its reverting status is reported as RECOVERY_INCOMPLETE
    /// and its entry stays for start-up recovery (REQ-SAFE-06).
    pub async fn prepare_quit(&self) {
        let _lock = self.lock_changes_for(None).await;
        let pending = self.lock().pending.clone();
        if let Some(pending) = pending
            && let Err(error) = self.revert_locked(&pending.change_id, RevertReason::Quit)
        {
            self.stop_countdown(&pending.change_id);
            let error = if error.code() == ErrorCode::RecoveryIncomplete {
                error
            } else {
                self.recovery_incomplete(&format!(
                    "the pending change did not go back before the exit ({error}); start-up \
                     recovery puts it back"
                ))
            };
            log::error!("the pending change did not go back before the exit: {error}");
            self.raise(error_source_for(pending.source), &error);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn req_safe_05_countdown_counts_whole_seconds() {
        let countdown = Countdown::new("change-1", 5_000, 10);
        assert_eq!(countdown.seconds_left(5_000), 10);
        assert_eq!(countdown.seconds_left(5_999), 10);
        assert_eq!(countdown.seconds_left(6_000), 9);
        assert_eq!(countdown.seconds_left(15_000), 0);
        assert_eq!(countdown.seconds_left(99_000), 0);
        // A clock read before the start counts as the start.
        assert_eq!(countdown.seconds_left(0), 10);
        assert_eq!(countdown.next_due_ms(), 6_000);
        let later = Countdown {
            last_sent: 7,
            ..countdown.clone()
        };
        assert_eq!(later.next_due_ms(), 9_000);
        // After the tick with 0, the next wake is the revert: at the end, then one second
        // after each try that did not start.
        let ended = Countdown {
            last_sent: 0,
            ..countdown
        };
        assert_eq!(ended.next_due_ms(), 15_000);
        let retried = Countdown {
            revert_due_ms: 16_000,
            failed_reverts: 1,
            ..ended
        };
        assert_eq!(retried.next_due_ms(), 16_000);
    }
}
