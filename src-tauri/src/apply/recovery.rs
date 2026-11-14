//! Putting journaled monitors back to their baselines: Restore everything's restore
//! (REQ-DISP-22), start-up recovery (REQ-SAFE-06, REQ-SAFE-07), and the reaction to display
//! changes, suspend and resume while a change is open (REQ-DISP-23, REQ-SAFE-08,
//! REQ-SAFE-09). Recovery never opens a countdown, never runs an elevated tool and never
//! shows a UAC prompt.

#![forbid(unsafe_code)]

use std::sync::atomic::Ordering;

use crate::display::monitors::{find_monitor, same_monitor, to_monitor};
use crate::display::types::{MonitorId, Scaling};
use crate::display::{DisplayConfig, RawMonitor, TargetConfig};
use crate::error::{AppError, ErrorCode, JournalItemKind, StepKind};
use crate::ipc::types::{
    BackendKind, BusyAction, ChangeSource, ErrorSource, RecoveryState, RecoveryStatus,
    RestoreOutcome, RevertReason,
};
use crate::system::notify::SystemEvent;
use crate::{Core, CoreState, guard};

use super::journal::{ChangeRecord, ChangeStatus, DisplayState, MonitorRecord};
use super::{Steps, keep_active, monitor_gone, new_change_id, shows_state};

/// What putting journaled monitors back did.
pub(crate) struct Putback {
    /// The monitors that verified back on their baselines; their entries are closed.
    pub(crate) restored: Vec<MonitorId>,
    pub(crate) steps: Steps,
    /// The monitors still changed or not connected; their entries stay open.
    pub(crate) left: Vec<MonitorId>,
}

/// Which full-screen request puts back a baseline's scaling.
fn scaling_choice(state: &DisplayState) -> crate::display::types::ScalingChoice {
    if state.scaling == Scaling::Stretched {
        crate::display::types::ScalingChoice::FullScreen
    } else {
        crate::display::types::ScalingChoice::Keep
    }
}

/// Recovery is clean again: nothing journaled is left changed, and no moved-aside journal is
/// waiting for its fallback. The RECOVERY_INCOMPLETE errors raised for any change describe a
/// state that no longer exists, so they leave the list too, and the window and the tray stop
/// pointing at it (D-034).
pub(crate) fn settle_recovery(state: &mut CoreState) {
    state.recovery.status = RecoveryStatus::Clean;
    state.recovery.still_changed.clear();
    state
        .errors
        .retain(|raised| raised.error.code != ErrorCode::RecoveryIncomplete);
}

impl Core {
    /// Puts the baselines of `which` back (all journaled monitors when `None`), under the
    /// change lock (REQ-DISP-22, D-024): status restoring on their open changes, which gives
    /// the Tickets; the saved snapshot when `use_snapshot` and one is kept, otherwise each
    /// baseline mode on the apply route; a fresh read; SDC_USE_DATABASE_CURRENT for what did
    /// not verify; another read; then the entries of every verified monitor close. A pending
    /// change among them is taken over once its restoring status is on disk: its countdown
    /// stops and it shows as an active change until its monitor verifies.
    #[allow(
        clippy::too_many_lines,
        reason = "the restore steps read best in their documented order"
    )]
    pub(crate) fn restore_baselines(
        &self,
        which: Option<&[MonitorId]>,
        use_snapshot: bool,
    ) -> Result<Putback, AppError> {
        let file = self.inner.journal.snapshot();
        let wanted =
            |id: &MonitorId| which.is_none_or(|set| set.iter().any(|w| same_monitor(w, id)));
        let records: Vec<MonitorRecord> = file
            .monitors
            .iter()
            .filter(|record| wanted(&record.identity))
            .cloned()
            .collect();
        let changes: Vec<&ChangeRecord> = file
            .changes
            .iter()
            .filter(|change| match &change.monitor {
                Some(monitor) => wanted(monitor),
                None => which.is_none(),
            })
            .collect();
        let mut steps = Steps::default();
        if changes.is_empty() {
            return Ok(Putback {
                restored: Vec::new(),
                steps,
                left: Vec::new(),
            });
        }
        self.set_busy(BusyAction::Restoring, StepKind::Journal);
        let ids: Vec<&str> = changes
            .iter()
            .map(|change| change.change_id.as_str())
            .collect();
        let tickets = match self.inner.journal.begin_restore(&ids) {
            Ok(tickets) => tickets,
            Err(error) => {
                steps.failed(StepKind::Journal);
                self.clear_busy();
                return Err(error.with_steps(steps.list()));
            }
        };
        steps.done(StepKind::Journal);
        self.take_over_pending(&ids);
        let Some(first) = tickets.first() else {
            self.clear_busy();
            return Err(AppError::internal("a restore with no Ticket"));
        };
        let ticket_for = |monitor: &MonitorId| {
            changes
                .iter()
                .position(|change| {
                    change
                        .monitor
                        .as_ref()
                        .is_some_and(|on| same_monitor(on, monitor))
                })
                .and_then(|index| tickets.get(index))
                .unwrap_or(first)
        };
        let live = match self.inner.display.enumerate() {
            Ok(live) => live,
            Err(error) => {
                self.clear_busy();
                return Err(error.with_steps(steps.list()));
            }
        };
        // The saved snapshot first, when every monitor it covers is being restored.
        let snapshot = file.layout.as_ref().filter(|_| use_snapshot);
        let mut restore_ok = true;
        self.set_busy(BusyAction::Restoring, StepKind::Restore);
        if let Some(layout) = snapshot {
            match self.inner.display.restore_snapshot(first, layout) {
                Ok(done) => {
                    for gone in &done.left_out {
                        log::warn!(
                            "the snapshot restore left out a monitor that is not connected \
                             (MONITOR_GONE): {}",
                            self.monitor_name(gone, &live).unwrap_or_default()
                        );
                    }
                }
                Err(error) => {
                    log::warn!("the snapshot restore failed: {error}");
                    restore_ok = false;
                }
            }
        } else {
            for record in &records {
                if find_monitor(&record.identity, &live).is_none() {
                    continue;
                }
                let config = DisplayConfig {
                    targets: vec![TargetConfig {
                        monitor: record.identity.clone(),
                        mode: record.baseline.raw_mode(),
                        scaling: scaling_choice(&record.baseline),
                        exact_scaling: Some(record.baseline.scaling),
                    }],
                };
                if let Err(error) = self
                    .inner
                    .display
                    .apply_config(ticket_for(&record.identity), &config)
                {
                    log::warn!("a baseline did not go back: {error}");
                    restore_ok = false;
                }
            }
        }
        let verified = |after: &[RawMonitor]| -> Vec<MonitorId> {
            records
                .iter()
                .filter_map(|record| {
                    find_monitor(&record.identity, after)
                        .filter(|live| shows_state(live, &record.baseline))
                        .map(|live| live.id.clone())
                })
                .collect()
        };
        let present = |after: &[RawMonitor]| {
            records
                .iter()
                .filter(|record| find_monitor(&record.identity, after).is_some())
                .count()
        };
        self.set_busy(BusyAction::Restoring, StepKind::Verify);
        let mut after = self.inner.display.enumerate().unwrap_or_default();
        let mut back = verified(&after);
        if restore_ok && back.len() == present(&after) {
            steps.done(StepKind::Restore);
            steps.done(StepKind::Verify);
        } else {
            steps.failed(StepKind::Restore);
            steps.failed(StepKind::Verify);
            // The last fallback: the configuration Windows keeps (REQ-DISP-22, REQ-MM-04).
            self.set_busy(BusyAction::Restoring, StepKind::Restore);
            match self.inner.display.apply_database_current(first) {
                Ok(()) => {
                    self.set_busy(BusyAction::Restoring, StepKind::Verify);
                    after = self.inner.display.enumerate().unwrap_or_default();
                    back = verified(&after);
                    if back.len() == present(&after) {
                        steps.done(StepKind::Restore);
                        steps.done(StepKind::Verify);
                    } else {
                        steps.done(StepKind::Restore);
                        steps.failed(StepKind::Verify);
                    }
                }
                Err(error) => {
                    log::warn!("SDC_USE_DATABASE_CURRENT failed: {error}");
                    steps.failed(StepKind::Restore);
                }
            }
        }
        // Close what verified; monitors not connected keep their entries (REQ-SAFE-09).
        let mut close: Vec<&str> = changes
            .iter()
            .filter(|change| {
                change
                    .monitor
                    .as_ref()
                    .is_some_and(|on| back.iter().any(|done| same_monitor(done, on)))
            })
            .map(|change| change.change_id.as_str())
            .collect();
        let left: Vec<MonitorId> = records
            .iter()
            .filter(|record| !back.iter().any(|done| same_monitor(done, &record.identity)))
            .map(|record| record.identity.clone())
            .collect();
        if left.is_empty() {
            close.extend(
                changes
                    .iter()
                    .filter(|change| change.monitor.is_none())
                    .map(|change| change.change_id.as_str()),
            );
        }
        if let Err(error) = self.inner.journal.close(&close) {
            log::warn!("restored entries were not closed: {error}");
        }
        for gone in &left {
            if find_monitor(gone, &after).is_none() {
                self.mark_missing(gone);
            }
        }
        let monitors: Vec<_> = after.iter().map(to_monitor).collect();
        self.update(|state| {
            if !monitors.is_empty() {
                state.monitors = monitors;
            }
            state.busy = None;
            for done in &back {
                keep_active(state, done, None);
            }
        });
        log::info!(
            "restore of {} monitor(s): {}; {} left",
            records.len(),
            steps.summary(),
            left.len()
        );
        Ok(Putback {
            restored: back,
            steps,
            left,
        })
    }

    /// After status restoring was written on `ids`: a pending change among them now belongs
    /// to this restore, as when its own revert could not write its status. Its countdown
    /// stops, so it never tries to revert an entry the restore moved on (REQ-SAFE-05), the
    /// keep-mode window closes, the changes waiting behind it may run once the change lock
    /// is free, and it shows as an active change until its monitor verifies back.
    fn take_over_pending(&self, ids: &[&str]) {
        let Some(pending) = self
            .lock()
            .pending
            .clone()
            .filter(|pending| ids.contains(&pending.change_id.as_str()))
        else {
            return;
        };
        self.stop_countdown(&pending.change_id);
        let change = self.pending_as_active(&pending, pending.to.clone(), pending.scaling);
        self.update(|state| {
            state.pending = None;
            keep_active(state, &pending.monitor, Some(change));
        });
        self.wake_waiters();
        self.keep_mode().close();
        log::info!(
            "change {}: the restore took over the pending change",
            pending.change_id
        );
    }

    /// Restore everything with nothing in the journal while recovery is incomplete, as after
    /// a damaged or newer journal.json was moved aside: only the configuration Windows keeps
    /// can go back (D-023). A journal record of kind layout covers the call, so it still has
    /// its Ticket.
    pub(crate) fn restore_without_journal(
        &self,
        source: ChangeSource,
    ) -> Result<RestoreOutcome, AppError> {
        let live = self.inner.display.enumerate()?;
        let Some(state) = live.iter().find_map(DisplayState::of_monitor) else {
            return Err(monitor_gone(None, "no connected monitor shows a mode"));
        };
        let now_ms = self.inner.clock.epoch_ms();
        let route = self.inner.display.route();
        let change_id = new_change_id();
        let record = ChangeRecord {
            change_id: change_id.clone(),
            kind: JournalItemKind::Layout,
            monitor: None,
            previous: state.clone(),
            target: state,
            scaling_requested: crate::display::types::ScalingChoice::Keep,
            route: route.route,
            gdi_flag: route.gdi_flag,
            status: ChangeStatus::Intent,
            source,
            profile_id: None,
            started_ms: now_ms,
            updated_ms: now_ms,
        };
        let mut steps = Steps::default();
        self.set_busy(BusyAction::Restoring, StepKind::Journal);
        let ticket = match self.inner.journal.open_change(record, None) {
            Ok(ticket) => ticket,
            Err(error) => {
                steps.failed(StepKind::Journal);
                self.clear_busy();
                return Err(error.with_steps(steps.list()));
            }
        };
        steps.done(StepKind::Journal);
        self.set_busy(BusyAction::Restoring, StepKind::Restore);
        let applied = self.inner.display.apply_database_current(&ticket);
        self.set_busy(BusyAction::Restoring, StepKind::Verify);
        let after = self.inner.display.enumerate();
        let result = match (applied, after) {
            (Ok(()), Ok(after)) => {
                steps.done(StepKind::Restore);
                steps.done(StepKind::Verify);
                if let Err(error) = self.inner.journal.close(&[change_id.as_str()]) {
                    log::warn!("the restore record was not closed: {error}");
                }
                let restored = after.iter().map(|monitor| monitor.id.clone()).collect();
                let monitors = after.iter().map(to_monitor).collect();
                self.inner.journal_lost.store(false, Ordering::Relaxed);
                self.update(|state| {
                    state.monitors = monitors;
                    state.busy = None;
                    settle_recovery(state);
                });
                Ok(RestoreOutcome {
                    restored,
                    steps: steps.list(),
                })
            }
            (Err(error), _) | (_, Err(error)) => {
                steps.failed(StepKind::Restore);
                self.clear_busy();
                Err(self
                    .recovery_incomplete(&format!(
                        "the configuration Windows keeps did not go back: {error}"
                    ))
                    .with_steps(steps.list()))
            }
        };
        log::info!("Restore everything with no journal: {}", steps.summary());
        result
    }

    /// Start-up recovery, before anything that can change the PC or react to a game. A
    /// release build and a debug run on the fake backend put back every unfinished change at
    /// once (REQ-SAFE-06; until the process-list wait of D-018 arrives with P4). A debug build
    /// on the Win32 backend changes nothing on the display (REQ-SAFE-07): it closes the
    /// entries Windows already put back and reports only what still differs, offering
    /// Restore everything (D-033).
    pub async fn recover_at_start(&self) {
        let _lock = self.lock_changes_for(None).await;
        let journal_lost = self.inner.journal_lost.load(Ordering::Relaxed);
        if !self.inner.journal.needs_recovery() {
            let status = if journal_lost {
                RecoveryStatus::Incomplete
            } else {
                RecoveryStatus::Clean
            };
            self.set_recovery(status, Vec::new());
            return;
        }
        let report_only = self.inner.build.debug && self.inner.build.backend == BackendKind::Real;
        let live = match self.inner.display.enumerate() {
            Ok(live) => live,
            Err(error) => {
                let error = self.recovery_incomplete(&format!("the display read failed: {error}"));
                self.set_recovery(
                    RecoveryStatus::Incomplete,
                    error.to_ipc().context.still_changed,
                );
                self.raise(ErrorSource::Startup, &error);
                return;
            }
        };
        let file = self.inner.journal.snapshot();
        let mut close: Vec<String> = Vec::new();
        let mut restore: Vec<MonitorId> = Vec::new();
        let mut gone: Vec<MonitorId> = Vec::new();
        let mut changed_outside = false;
        for record in &file.monitors {
            let changes: Vec<&ChangeRecord> = file.changes_on(&record.identity).collect();
            let ids = changes.iter().map(|change| change.change_id.clone());
            let Some(monitor) = find_monitor(&record.identity, &live) else {
                log::warn!("recovery: a journaled monitor is not connected (MONITOR_GONE)");
                gone.push(record.identity.clone());
                continue;
            };
            let current = monitor.current;
            let baseline_mode = current.is_some_and(|now| record.baseline.shows(&now));
            if baseline_mode && monitor.scaling == record.baseline.scaling {
                // Windows already put it back, for example on the process exit.
                close.extend(ids);
            } else if baseline_mode {
                // Only the scaling still differs from the baseline: put it back.
                restore.push(record.identity.clone());
            } else if changes
                .iter()
                .any(|change| current.is_some_and(|now| change.target.shows(&now)))
            {
                restore.push(record.identity.clone());
            } else {
                // Changed outside ResSwap: never fight it.
                log::info!("recovery: a journaled monitor was changed outside ResSwap");
                changed_outside = true;
                close.extend(ids);
            }
        }
        close.extend(
            file.changes
                .iter()
                .filter(|change| change.monitor.is_none())
                .map(|change| change.change_id.clone()),
        );
        let refs: Vec<&str> = close.iter().map(String::as_str).collect();
        if let Err(error) = self.inner.journal.close(&refs) {
            log::warn!("recovery: entries were not closed: {error}");
        }
        // The saved snapshot still holds the mode that monitor had before ResSwap's change, so
        // a later Restore everything would put it back over the outside change. Without the
        // snapshot every monitor still listed goes back on its own route (D-036).
        if changed_outside
            && self.inner.journal.needs_recovery()
            && let Err(error) = self.inner.journal.clear_layout()
        {
            log::warn!("recovery: the saved snapshot was not dropped: {error}");
        }
        if report_only {
            // A debug build on the Win32 backend changes nothing on the display at start
            // (REQ-SAFE-07). Entries Windows already put back, or that were changed outside
            // ResSwap, closed above, a journal write only; what still differs is reported
            // for Restore everything (D-033). A monitor that is not connected is not marked
            // missing, so its return does not put anything back by itself.
            if restore.is_empty() && gone.is_empty() && !self.inner.journal.needs_recovery() {
                let status = if journal_lost {
                    RecoveryStatus::Incomplete
                } else {
                    RecoveryStatus::Clean
                };
                self.set_recovery(status, Vec::new());
                log::info!("recovery: every open change was already back; nothing to report");
                return;
            }
            let error = self.recovery_incomplete(
                "a debug build on the Win32 backend puts nothing back at start; Restore \
                 everything puts these back",
            );
            self.set_recovery(
                RecoveryStatus::Incomplete,
                error.to_ipc().context.still_changed,
            );
            self.raise(ErrorSource::Startup, &error);
            return;
        }
        let mut restored_any = false;
        let mut left = gone.clone();
        if !restore.is_empty() {
            // The snapshot sets every monitor, so it is used only when every journaled
            // monitor needs its baseline back.
            let use_snapshot = gone.is_empty() && restore.len() == file.monitors.len();
            match self.restore_baselines(Some(&restore), use_snapshot) {
                Ok(putback) => {
                    restored_any = !putback.restored.is_empty();
                    left.extend(putback.left);
                }
                Err(error) => {
                    log::warn!("recovery: the restore failed: {error}");
                    left.extend(restore);
                }
            }
        }
        for monitor in &gone {
            self.mark_missing(monitor);
        }
        if left.is_empty() && !self.inner.journal.needs_recovery() {
            let status = if journal_lost {
                RecoveryStatus::Incomplete
            } else if restored_any {
                RecoveryStatus::Recovered
            } else {
                RecoveryStatus::Clean
            };
            self.set_recovery(status, Vec::new());
            log::info!("recovery: {status:?}");
            return;
        }
        let error = self.recovery_incomplete("start-up recovery left something changed");
        self.set_recovery(
            RecoveryStatus::Incomplete,
            error.to_ipc().context.still_changed,
        );
        self.raise(ErrorSource::Startup, &error);
    }

    fn set_recovery(&self, status: RecoveryStatus, still_changed: Vec<crate::error::JournalItem>) {
        self.update(|state| {
            state.recovery = RecoveryState {
                status,
                still_changed,
            };
        });
    }

    /// A notice from the hidden top-level window (REQ-DISP-23, REQ-SAFE-08), or from a test.
    /// A suspend only flushes: every journal write is flushed when it is made, so nothing
    /// more is written and the countdown waits for the resume. A display change is re-read
    /// once however many notices arrive while one waits for the change lock.
    pub async fn on_system_event(&self, event: SystemEvent) {
        match event {
            SystemEvent::Suspending => {
                self.inner.suspended.store(true, Ordering::Relaxed);
                log::info!("suspend notice: the journal is already on disk");
            }
            SystemEvent::Resumed => {
                self.inner.suspended.store(false, Ordering::Relaxed);
                let _lock = self.lock_changes_for(None).await;
                self.after_resume();
            }
            SystemEvent::DisplayChanged => {
                if self
                    .inner
                    .display_change_queued
                    .swap(true, Ordering::AcqRel)
                {
                    return;
                }
                let _lock = self.lock_changes_for(None).await;
                self.inner
                    .display_change_queued
                    .store(false, Ordering::Release);
                self.after_display_change();
            }
        }
    }

    /// After a resume: a pending change goes back at once, reason Timeout, because the player
    /// could not answer during sleep; entries whose monitor shows its baseline close; then
    /// the state matches the screen (REQ-SAFE-08). A revert that could not write its
    /// reverting status leaves the countdown running, which tries again one second later.
    fn after_resume(&self) {
        let pending = self.lock().pending.clone();
        if let Some(pending) = pending
            && let Err(error) = self.revert_locked(&pending.change_id, RevertReason::Timeout)
        {
            // Counted as the countdown's first failed try, so it is raised once, here.
            self.retry_revert_later(&pending.change_id, true);
            self.raise(ErrorSource::Resume, &error);
        }
        let live = match self.inner.display.enumerate() {
            Ok(live) => live,
            Err(error) => {
                self.raise(ErrorSource::Resume, &error);
                return;
            }
        };
        self.close_shown_baselines(&live);
        let monitors: Vec<_> = live.iter().map(to_monitor).collect();
        self.update(|state| state.monitors = monitors);
    }

    /// Closes the entries of every journaled monitor that shows its baseline in `live`.
    fn close_shown_baselines(&self, live: &[RawMonitor]) {
        for record in self.inner.journal.snapshot().monitors {
            if find_monitor(&record.identity, live)
                .is_some_and(|monitor| shows_state(monitor, &record.baseline))
            {
                self.close_monitor_entries(&record.identity);
                self.update(|state| keep_active(state, &record.identity, None));
            }
        }
    }

    /// After WM_DISPLAYCHANGE (REQ-DISP-23, D-025): never re-apply anything.
    /// A pending change replaced by another mode stops its countdown; it closes when the
    /// screen shows the state before it, and otherwise stays open as an active change, so
    /// Restore still works. An active change replaced by a game or Windows records the new
    /// mode. A journaled monitor that went away is raised as MONITOR_GONE once and keeps its
    /// entries; when it returns, its baseline goes back (REQ-SAFE-09).
    #[allow(
        clippy::too_many_lines,
        reason = "the cases of one display change read best together"
    )]
    fn after_display_change(&self) {
        let live = match self.inner.display.enumerate() {
            Ok(live) => live,
            Err(error) => {
                self.raise(ErrorSource::DisplayChange, &error);
                return;
            }
        };
        let pending = self.lock().pending.clone();
        if let Some(pending) = pending {
            let on_screen = find_monitor(&pending.monitor, &live).cloned();
            let shown = on_screen.as_ref().and_then(|monitor| monitor.current);
            let still_pending = shown.is_some_and(|now| {
                crate::display::modes::same_mode(now, crate::display::modes::to_raw(&pending.to))
            });
            if !still_pending {
                self.stop_countdown(&pending.change_id);
                let record = self.inner.journal.change(&pending.change_id);
                let back_before = match (&record, &on_screen) {
                    (Some(record), Some(monitor)) => monitor
                        .current
                        .is_some_and(|now| record.previous.shows(&now)),
                    _ => false,
                };
                if back_before {
                    if let Err(error) = self.inner.journal.close(&[pending.change_id.as_str()]) {
                        log::warn!("an ended pending change was not closed: {error}");
                    }
                    self.update(|state| state.pending = None);
                } else {
                    if let Err(error) = self
                        .inner
                        .journal
                        .advance(&pending.change_id, ChangeStatus::Applied)
                    {
                        log::warn!("an ended pending change kept its status: {error}");
                    }
                    let before = self
                        .inner
                        .journal
                        .baseline_of(&pending.monitor)
                        .map_or_else(
                            || pending.from.clone(),
                            |base| crate::display::modes::to_mode(base.baseline.raw_mode()),
                        );
                    let change = crate::ipc::types::ActiveChange {
                        change_id: pending.change_id.clone(),
                        monitor: pending.monitor.clone(),
                        monitor_name: pending.monitor_name.clone(),
                        before,
                        now: shown
                            .map_or_else(|| pending.to.clone(), crate::display::modes::to_mode),
                        scaling: on_screen
                            .as_ref()
                            .map_or(pending.scaling, |monitor| monitor.scaling),
                        turned_off: Vec::new(),
                        profile_id: record.and_then(|record| record.profile_id),
                        source: pending.source,
                        since_ms: self.inner.clock.epoch_ms(),
                    };
                    self.update(|state| {
                        state.pending = None;
                        keep_active(state, &pending.monitor, Some(change));
                    });
                }
                self.wake_waiters();
                self.keep_mode().close();
                log::info!(
                    "change {}: another mode replaced it during the countdown; ResSwap does \
                     not fight it",
                    pending.change_id
                );
            }
        }
        // Active changes replaced by a game or Windows: record the new mode.
        let active = self.lock().active.clone();
        for change in active {
            let Some(monitor) = find_monitor(&change.monitor, &live) else {
                continue;
            };
            if let Some(now) = monitor.current {
                let now = crate::display::modes::to_mode(now);
                if now != change.now || monitor.scaling != change.scaling {
                    let mut updated = change.clone();
                    updated.now = now;
                    updated.scaling = monitor.scaling;
                    self.update(|state| keep_active(state, &change.monitor, Some(updated)));
                }
            }
        }
        // Journaled monitors that went away, and those that came back.
        let journaled = self.inner.journal.snapshot().monitors;
        for record in &journaled {
            let present = find_monitor(&record.identity, &live).is_some();
            let was_missing = guard(&self.inner.missing)
                .iter()
                .any(|known| same_monitor(known, &record.identity));
            if !present && !was_missing {
                self.mark_missing(&record.identity);
                self.raise(
                    ErrorSource::DisplayChange,
                    &monitor_gone(
                        self.monitor_name(&record.identity, &live),
                        "a monitor with an open change is not connected; its change is kept",
                    ),
                );
            } else if present && was_missing {
                guard(&self.inner.missing).retain(|known| !same_monitor(known, &record.identity));
                if let Err(error) =
                    self.restore_baselines(Some(std::slice::from_ref(&record.identity)), false)
                {
                    self.raise(ErrorSource::DisplayChange, &error);
                } else {
                    let left = self.inner.journal.open_changes_on(&record.identity);
                    if !left.is_empty() {
                        self.raise(
                            ErrorSource::DisplayChange,
                            &self.recovery_incomplete(
                                "a monitor came back, but its baseline did not",
                            ),
                        );
                    }
                }
            }
        }
        self.close_shown_baselines(&live);
        let live = self.inner.display.enumerate().unwrap_or(live);
        let monitors: Vec<_> = live.iter().map(to_monitor).collect();
        self.update(|state| state.monitors = monitors);
    }
}
