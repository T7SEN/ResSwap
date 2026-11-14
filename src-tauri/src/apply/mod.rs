//! The apply pipeline: one path for every source of a change (the window, the tray, a
//! hotkey, the watcher, a Play shortcut, recovery, the updater). Validate, take the change
//! lock, read, resolve, test, write the journal, apply as a temporary change, read back,
//! check the scaling, then either finish or start the revert countdown (REQ-SAFE-01,
//! REQ-DISP-12, REQ-DISP-13, REQ-SAFE-17). This file holds the `Ticket` every call changing
//! the PC requires; the journal, the countdown and recovery sit in the child modules.

#![forbid(unsafe_code)]

pub mod countdown;
pub mod journal;
pub mod recovery;

use std::collections::hash_map::RandomState;
use std::fmt::Write as _;
use std::hash::{BuildHasher, Hasher};
use std::sync::atomic::{AtomicU64, Ordering};

pub use journal::OpenEntries;

use crate::apply::recovery::settle_recovery;
use crate::display::modes::{
    is_confirmed, read_back_verifies, resolve_target, restore_native_target, same_mode, to_mode,
};
use crate::display::monitors::{find_monitor, has_edid_ids, same_monitor, to_monitor};
use crate::display::types::{Mode, MonitorId, Scaling, ScalingChoice};
use crate::display::{DisplayConfig, Rational, RawMode, RawMonitor, TargetConfig};
use crate::error::{
    AppError, ErrorCode, ErrorContext, Failure, JournalItemKind, ProfileProblem, StepKind,
    StepReport, StepStatus,
};
use crate::ipc::commands::CommandName;
use crate::ipc::events::{emit_mode_pending_confirm, emit_mode_reverted};
use crate::ipc::types::{
    ActiveChange, ApplyOutcome, ApplyRequest, BusyAction, BusyState, ChangeRef, ChangeSource,
    ConfirmOutcome, ErrorSource, ModeReverted, PendingChange, RecoveryStatus, RestoreNativeRequest,
    RestoreOutcome, RevertOutcome, RevertReason,
};
use crate::profiles::is_lowercase_uuid;
use crate::{Core, CoreState, guard, perf_event};

use journal::{ChangeRecord, ChangeStatus, DisplayState, MonitorRecord};

/// Proof that the journal entry for a change is on disk. Its fields are private to this
/// module and its children, so only the pipeline can build one and no command reaches a
/// changing backend call without it.
#[derive(Debug)]
pub struct Ticket {
    change_id: String,
    _private: (),
}

impl Ticket {
    /// The change this Ticket belongs to: the change ID of its journal entry.
    pub fn change_id(&self) -> &str {
        &self.change_id
    }
}

/// monitorDevicePath is a WCHAR[128] in DISPLAYCONFIG_TARGET_DEVICE_NAME, so a device path
/// Windows reports holds fewer UTF-16 units than this.
const MAX_DEVICE_PATH_UNITS: usize = 128;

/// The longest change ID a request may carry; the journal holds IDs of at most 64 ASCII
/// letters, digits and hyphens.
const MAX_CHANGE_ID_LEN: usize = 64;

/// Checks a MonitorId from the window (REQ-SEC-08): a device path no longer than Windows can
/// report and free of control characters, and an identity the matching rule can use.
/// Windows reports an empty device path for a monitor whose device is disabled in Device
/// Manager (REQ-DISP-26); that monitor is then named by its EDID ids and connector instance
/// (REQ-DISP-20), so an empty path needs both EDID ids. Anything else is a request the window
/// never sends: INTERNAL.
pub(crate) fn check_monitor_id(id: &MonitorId) -> Result<(), AppError> {
    let units = id.device_path.encode_utf16().count();
    if units >= MAX_DEVICE_PATH_UNITS || id.device_path.chars().any(char::is_control) {
        return Err(AppError::internal(
            "refused a request: the monitor's device path is too long or holds a control \
             character",
        ));
    }
    if units == 0 && !has_edid_ids(id) {
        return Err(AppError::internal(
            "refused a request: the monitor has neither a device path nor both EDID ids",
        ));
    }
    Ok(())
}

/// Checks a change ID from the window (REQ-SEC-08): 1 to 64 ASCII letters, digits or
/// hyphens, the shape the journal stores.
pub(crate) fn check_change_id(id: &str) -> Result<(), AppError> {
    let shaped = !id.is_empty()
        && id.len() <= MAX_CHANGE_ID_LEN
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-');
    if shaped {
        Ok(())
    } else {
        Err(AppError::internal(
            "refused a request: the change ID is empty, too long or has other characters",
        ))
    }
}

/// Counts the change IDs this process made, so two IDs never share their input.
static CHANGE_SERIAL: AtomicU64 = AtomicU64::new(0);

/// A new change ID: a lowercase UUID in the version 4 layout. The random bits come from the
/// standard library's per-process random hash keys, mixed with a serial number and the time,
/// so IDs are unique within a run and across runs; they guard nothing, so no stronger source
/// is needed.
pub(crate) fn new_change_id() -> String {
    let serial = CHANGE_SERIAL.fetch_add(1, Ordering::Relaxed);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_nanos());
    let mut words = [0u64; 2];
    for (index, word) in words.iter_mut().enumerate() {
        let mut hasher = RandomState::new().build_hasher();
        hasher.write_u64(serial);
        hasher.write_u128(nanos);
        hasher.write_usize(index);
        *word = hasher.finish();
    }
    let [high, low] = words;
    // Version 4 in the third group, the variant bits 10 in the fourth.
    let high = (high & 0xFFFF_FFFF_FFFF_0FFF) | 0x0000_0000_0000_4000;
    let low = (low & 0x3FFF_FFFF_FFFF_FFFF) | 0x8000_0000_0000_0000;
    format!(
        "{:08x}-{:04x}-{:04x}-{:04x}-{:012x}",
        high >> 32,
        (high >> 16) & 0xFFFF,
        high & 0xFFFF,
        low >> 48,
        low & 0xFFFF_FFFF_FFFF
    )
}

/// What one change asks for, after step 1.
#[derive(Clone, Debug)]
pub(crate) struct Target {
    pub(crate) monitor: MonitorId,
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) rate: Rational,
    pub(crate) scaling: ScalingChoice,
    pub(crate) profile_id: Option<String>,
    /// A hidden monitor may change only when the player picked it with the toggle on, or
    /// when ResSwap puts back what it changed.
    pub(crate) allow_hidden: bool,
}

/// The step reports of one operation, in order. A step shows as done only when its status is
/// Done, after the re-read that confirms it (REQ-SAFE-17).
#[derive(Clone, Debug, Default)]
pub(crate) struct Steps(Vec<StepReport>);

impl Steps {
    pub(crate) fn push(&mut self, step: StepKind, status: StepStatus) {
        self.0.push(StepReport { step, status });
    }

    pub(crate) fn done(&mut self, step: StepKind) {
        self.push(step, StepStatus::Done);
    }

    pub(crate) fn failed(&mut self, step: StepKind) {
        self.push(step, StepStatus::Failed);
    }

    pub(crate) fn list(&self) -> Vec<StepReport> {
        self.0.clone()
    }

    pub(crate) fn extend(&mut self, more: &Self) {
        self.0.extend(more.0.iter().cloned());
    }

    /// The steps as one log fragment: "test done, journal done, apply failed".
    pub(crate) fn summary(&self) -> String {
        let mut text = String::new();
        for (index, report) in self.0.iter().enumerate() {
            if index > 0 {
                text.push_str(", ");
            }
            let _ = write!(
                text,
                "{} {}",
                format!("{:?}", report.step).to_lowercase(),
                status_word(report.status)
            );
        }
        if text.is_empty() {
            text.push_str("no step");
        }
        text
    }
}

fn status_word(status: StepStatus) -> &'static str {
    match status {
        StepStatus::Done => "done",
        StepStatus::Failed => "failed",
        StepStatus::NotRun => "not run",
        StepStatus::Undone => "undone",
    }
}

/// The source name the timing lines carry, as the window spells it.
fn source_name(source: ChangeSource) -> &'static str {
    match source {
        ChangeSource::Window => "window",
        ChangeSource::Tray => "tray",
        ChangeSource::Hotkey => "hotkey",
        ChangeSource::Watcher => "watcher",
        ChangeSource::PlayShortcut => "playShortcut",
        ChangeSource::Recovery => "recovery",
        ChangeSource::Update => "update",
    }
}

/// Where an error from the background work of a change is raised: the change's own source
/// where ErrorSource has one, start-up for recovery, and the display check for the window and
/// the updater, whose countdown and keep-mode window have no source of their own.
pub(crate) fn error_source_for(source: ChangeSource) -> ErrorSource {
    match source {
        ChangeSource::Tray => ErrorSource::Tray,
        ChangeSource::Hotkey => ErrorSource::Hotkey,
        ChangeSource::Watcher => ErrorSource::Watcher,
        ChangeSource::PlayShortcut => ErrorSource::PlayShortcut,
        ChangeSource::Recovery => ErrorSource::Startup,
        ChangeSource::Window | ChangeSource::Update => ErrorSource::DisplayChange,
    }
}

/// Milliseconds since main() started, for the timing lines (REQ-PERF-03); "none" in a
/// process that never marked its start, such as a test.
fn mono_text() -> String {
    crate::mono_ms().map_or_else(|| String::from("none"), |ms| ms.to_string())
}

/// One PERF line of the mode-switch measurement (REQ-PERF-03).
fn perf_step(event: &str, change_id: &str) {
    perf_event(event, &[("change", &change_id), ("mono_ms", &mono_text())]);
}

/// The ScalingChoice that puts back a recorded scaling: stretched needs the full-screen
/// request; anything else keeps what the path holds, and the native size reads identity.
fn scaling_choice_for(scaling: Scaling) -> ScalingChoice {
    if scaling == Scaling::Stretched {
        ScalingChoice::FullScreen
    } else {
        ScalingChoice::Keep
    }
}

/// Whether a monitor shows a recorded state: the same size, an equal rational and the same
/// scaling.
pub(crate) fn shows_state(monitor: &RawMonitor, state: &DisplayState) -> bool {
    monitor.current.is_some_and(|current| state.shows(&current)) && monitor.scaling == state.scaling
}

/// MONITOR_GONE for a monitor named `name`.
pub(crate) fn monitor_gone(name: Option<String>, detail: &str) -> AppError {
    AppError::MonitorGone(Box::new(Failure::new(detail).with_context(ErrorContext {
        monitor: name,
        ..ErrorContext::default()
    })))
}

/// What putting a change back gave.
pub(crate) enum PutBack {
    /// The previous state verified on screen, scaling included: the mode read back and the
    /// fresh read.
    Done(RawMode, Vec<RawMonitor>),
    /// The previous mode verified on screen, but the scaling read back differs from the one
    /// recorded before the change. The entry stays open as still changed, so its baseline is
    /// never closed with the scaling changed; Restore everything puts the scaling back from
    /// the saved snapshot (REQ-SAFE-17, REQ-DISP-22).
    ScalingDiffers {
        restored: RawMode,
        scaling: Scaling,
        after: Vec<RawMonitor>,
        detail: String,
    },
    /// The reverting status could not be written, so nothing was tried.
    NotStarted(AppError),
    /// It was tried and did not verify, or the call failed.
    Failed(AppError),
}

/// Why a change did not verify after its apply call.
enum Rejection {
    /// The apply call failed.
    Apply(AppError),
    /// The apply call succeeded, but the read after it failed, so nothing is known about
    /// what the screen shows.
    ReadBack(AppError),
    /// The read after the apply call shows another mode, or no mode on that monitor.
    Differs,
    /// The read after the apply call worked but no longer lists the monitor.
    Gone,
}

impl Core {
    /// apply_mode, and the way every other source changes a mode: all of them go through this
    /// one pipeline (REQ-SAFE-01).
    pub async fn apply(
        &self,
        request: ApplyRequest,
        source: ChangeSource,
    ) -> Result<ApplyOutcome, AppError> {
        self.scripted(CommandName::ApplyMode)?;
        let target = self.target_of(request)?;
        let _lock = self.lock_changes_for(Some(&target.monitor)).await;
        self.apply_locked(&target, source)
    }

    /// Step 1: every field checked in Rust, never trusting the window (REQ-SEC-08). A typed
    /// size arrives with an empty label; labels are never read, the core builds its own.
    fn target_of(&self, request: ApplyRequest) -> Result<Target, AppError> {
        match request {
            ApplyRequest::Mode {
                monitor,
                mode,
                scaling,
            } => {
                check_monitor_id(&monitor)?;
                check_mode(&mode)?;
                Ok(Target {
                    monitor,
                    width: mode.width,
                    height: mode.height,
                    rate: Rational {
                        numerator: mode.refresh.numerator,
                        denominator: mode.refresh.denominator,
                    },
                    scaling,
                    profile_id: None,
                    allow_hidden: false,
                })
            }
            ApplyRequest::Profile { profile_id } => {
                let not_found = || {
                    AppError::ProfileInvalid(Box::new(
                        Failure::new("no saved profile has this ID").with_context(ErrorContext {
                            profile_problem: Some(ProfileProblem::NotFound),
                            ..ErrorContext::default()
                        }),
                    ))
                };
                if !is_lowercase_uuid(&profile_id) {
                    return Err(not_found());
                }
                let profile = self
                    .inner
                    .settings
                    .profiles()
                    .into_iter()
                    .find(|known| known.id == profile_id)
                    .ok_or_else(not_found)?;
                if !profile.turn_off.is_empty() {
                    return Err(AppError::internal(
                        "turning monitors off is not available in this build; nothing was \
                         changed",
                    ));
                }
                check_mode(&profile.mode)?;
                Ok(Target {
                    monitor: profile.monitor,
                    width: profile.mode.width,
                    height: profile.mode.height,
                    rate: Rational {
                        numerator: profile.mode.refresh.numerator,
                        denominator: profile.mode.refresh.denominator,
                    },
                    scaling: profile.scaling,
                    profile_id: Some(profile.id),
                    allow_hidden: false,
                })
            }
        }
    }

    /// Takes the change lock, in arrival order. A change for another monitor than the
    /// pending one waits until the pending change is kept or reverted, because only one
    /// change pends at a time (REQ-SAFE-03); `None` never waits. Read commands never take
    /// this lock.
    pub(crate) async fn lock_changes_for(&self, monitor: Option<&MonitorId>) -> impl Sized + '_ {
        loop {
            let lock = self.inner.change_lock.lock().await;
            let blocked = monitor.is_some_and(|wanted| {
                self.lock()
                    .pending
                    .as_ref()
                    .is_some_and(|pending| !same_monitor(&pending.monitor, wanted))
            });
            if !blocked {
                return lock;
            }
            let (wake, mut woken) = tauri::async_runtime::channel::<()>(1);
            guard(&self.inner.pending_waiters).push(wake);
            drop(lock);
            let _ = woken.recv().await;
        }
    }

    /// Wakes the changes that wait for the pending change to end.
    pub(crate) fn wake_waiters(&self) {
        for waiter in std::mem::take(&mut *guard(&self.inner.pending_waiters)) {
            let _ = waiter.try_send(());
        }
    }

    /// The pending change on `monitor`, if any.
    fn pending_on(&self, monitor: &MonitorId) -> Option<PendingChange> {
        self.lock()
            .pending
            .clone()
            .filter(|pending| same_monitor(&pending.monitor, monitor))
    }

    /// Step 2 and on, under the change lock: a pending change on the same monitor goes back
    /// first (reason Superseded), then the change runs.
    pub(crate) fn apply_locked(
        &self,
        target: &Target,
        source: ChangeSource,
    ) -> Result<ApplyOutcome, AppError> {
        if let Some(pending) = self.pending_on(&target.monitor) {
            self.revert_locked(&pending.change_id, RevertReason::Superseded)?;
        }
        self.change_mode(target, source, BusyAction::Applying)
    }

    /// Sets AppState.busy and sends state-changed.
    pub(crate) fn set_busy(&self, action: BusyAction, step: StepKind) {
        self.update(|state| {
            state.busy = Some(BusyState {
                action,
                step,
                waiting_for_uac: false,
            });
        });
    }

    /// Clears AppState.busy and sends state-changed.
    pub(crate) fn clear_busy(&self) {
        self.update(|state| state.busy = None);
    }

    /// Steps 3 to 12 for one monitor. Every failure leaves AppState.busy clear.
    pub(crate) fn change_mode(
        &self,
        target: &Target,
        source: ChangeSource,
        busy: BusyAction,
    ) -> Result<ApplyOutcome, AppError> {
        let change_id = new_change_id();
        let profile = target
            .profile_id
            .clone()
            .unwrap_or_else(|| String::from("none"));
        perf_event(
            "apply.received",
            &[
                ("change", &change_id),
                ("source", &source_name(source)),
                ("profile", &profile),
                ("mono_ms", &mono_text()),
            ],
        );
        self.set_busy(busy, StepKind::Test);
        let mut steps = Steps::default();
        let result = self.change_mode_steps(&change_id, target, source, busy, &mut steps);
        match &result {
            Ok(outcome) => {
                let kind = match outcome {
                    ApplyOutcome::Applied { .. } => "applied",
                    ApplyOutcome::PendingConfirm { .. } => "pending",
                };
                log::info!(
                    "change {change_id} from {}: {}; {kind}",
                    source_name(source),
                    steps.summary()
                );
            }
            Err(error) => {
                log::warn!(
                    "change {change_id} from {}: {}; {:?}: {error}",
                    source_name(source),
                    steps.summary(),
                    error.code()
                );
                self.clear_busy();
            }
        }
        result
    }

    #[allow(
        clippy::too_many_lines,
        reason = "the steps of one change read best in their documented order"
    )]
    fn change_mode_steps(
        &self,
        change_id: &str,
        target: &Target,
        source: ChangeSource,
        busy: BusyAction,
        steps: &mut Steps,
    ) -> Result<ApplyOutcome, AppError> {
        // Step 3: a fresh read, which is also the previous state.
        let before = self.inner.display.enumerate()?;
        let Some(monitor) = find_monitor(&target.monitor, &before).cloned() else {
            return Err(monitor_gone(
                self.monitor_name(&target.monitor, &before),
                "no connected monitor matches the request",
            ));
        };
        let name = self.monitor_name(&monitor.id, &before);
        if !target.allow_hidden
            && to_monitor(&monitor).hidden
            && !self.lock().settings.show_hidden_monitors
        {
            return Err(AppError::internal(
                "refused a request for a hidden monitor while Show hidden monitors is off",
            ));
        }
        // Step 4: the listed mode, at the exact rational.
        let listed = resolve_target(&monitor, target.width, target.height, target.rate)?;
        let config = DisplayConfig {
            targets: vec![TargetConfig {
                monitor: monitor.id.clone(),
                mode: listed,
                scaling: target.scaling,
                exact_scaling: None,
            }],
        };
        let requested = to_mode(listed);
        let with_mode = |error: AppError| {
            error.with_context(|context| {
                context.monitor.clone_from(&name);
                context.mode = Some(requested.clone());
            })
        };
        // Step 5: the test changes nothing (REQ-DISP-12).
        if let Err(error) = self.inner.display.test_config(&config) {
            steps.failed(StepKind::Test);
            return Err(with_mode(error).with_steps(steps.list()));
        }
        steps.done(StepKind::Test);
        perf_step("apply.tested", change_id);
        // Step 6: the journal entry with the full previous state, before any change. The
        // window shows each step as it starts.
        self.set_busy(busy, StepKind::Journal);
        let Some(previous) = DisplayState::of_monitor(&monitor) else {
            return Err(monitor_gone(name, "the monitor shows no mode"));
        };
        let now_ms = self.inner.clock.epoch_ms();
        let baseline = match self.inner.journal.baseline_of(&monitor.id) {
            Some(_) => None,
            None => MonitorRecord::baseline_of(&monitor, now_ms),
        };
        let layout = if self.inner.journal.snapshot().layout.is_none() {
            match self.inner.display.snapshot() {
                Ok(mut layout) => {
                    layout.taken_ms = now_ms;
                    Some(layout)
                }
                Err(error) => {
                    steps.failed(StepKind::Journal);
                    return Err(error.with_steps(steps.list()));
                }
            }
        } else {
            None
        };
        let route = self.inner.display.route();
        let expected_scaling = match target.scaling {
            ScalingChoice::FullScreen => Scaling::Stretched,
            ScalingChoice::Keep => previous.scaling,
        };
        let record = ChangeRecord {
            change_id: change_id.to_owned(),
            kind: JournalItemKind::Mode,
            monitor: Some(monitor.id.clone()),
            previous: previous.clone(),
            target: DisplayState {
                width: listed.width,
                height: listed.height,
                refresh: listed.refresh,
                scaling: expected_scaling,
                position_x: previous.position_x,
                position_y: previous.position_y,
                gdi: None,
            },
            scaling_requested: target.scaling,
            route: route.route,
            gdi_flag: route.gdi_flag,
            status: ChangeStatus::Intent,
            source,
            profile_id: target.profile_id.clone(),
            started_ms: now_ms,
            updated_ms: now_ms,
        };
        let ticket = match self
            .inner
            .journal
            .open_change_with(record.clone(), baseline, layout)
        {
            Ok(ticket) => ticket,
            Err(error) => {
                steps.failed(StepKind::Journal);
                return Err(error.with_steps(steps.list()));
            }
        };
        steps.done(StepKind::Journal);
        perf_step("apply.journaled", change_id);
        // Step 7: a temporary change, never the Windows default (REQ-DISP-14).
        self.set_busy(busy, StepKind::Apply);
        let applied = self.inner.display.apply_config(&ticket, &config);
        perf_step("apply.applied", change_id);
        // Step 8: a step is done only after this re-read (REQ-SAFE-17).
        let ccd_rates: Vec<Rational> = [Some(previous.refresh), monitor.native.map(|m| m.refresh)]
            .into_iter()
            .flatten()
            .collect();
        match &applied {
            Ok(()) => steps.done(StepKind::Apply),
            Err(_) => steps.failed(StepKind::Apply),
        }
        self.set_busy(busy, StepKind::Verify);
        let read = self.inner.display.enumerate();
        let checked = match (applied, read) {
            (Ok(()), Ok(after)) => match find_monitor(&monitor.id, &after).cloned() {
                // Unplugged or switched off between the apply call and the read: the monitor
                // is gone, not the mode rejected (REQ-SAFE-09).
                None => Err(Rejection::Gone),
                Some(live)
                    if live
                        .current
                        .is_some_and(|now| read_back_verifies(listed, now, &ccd_rates)) =>
                {
                    Ok((live, after))
                }
                Some(_) => Err(Rejection::Differs),
            },
            (Ok(()), Err(error)) => {
                // A failed read says nothing about the mode, so it is never reported as a
                // rejected mode (REQ-UI-09).
                log::warn!("change {change_id}: the read after the apply call failed: {error}");
                Err(Rejection::ReadBack(error))
            }
            (Err(error), _) => Err(Rejection::Apply(error)),
        };
        let (live, after) = match checked {
            Ok(verified) => verified,
            Err(cause) => {
                steps.failed(StepKind::Verify);
                return Err(self.reject(&record, &monitor, requested, name, cause, busy, steps));
            }
        };
        steps.done(StepKind::Verify);
        perf_event(
            "apply.verified",
            &[
                ("change", &change_id),
                ("mono_ms", &mono_text()),
                ("epoch_ms", &crate::system::clock::epoch_ms_now()),
            ],
        );
        let Some(now_mode) = live.current else {
            return Err(AppError::internal("the verified monitor shows no mode"));
        };
        // Step 9: scaling is an intent the driver may ignore (REQ-NV-25).
        let mut warnings = Vec::new();
        let sizes_differ = monitor
            .native
            .is_some_and(|native| native.width != listed.width || native.height != listed.height);
        if target.scaling == ScalingChoice::FullScreen && sizes_differ {
            self.set_busy(busy, StepKind::Scaling);
            if live.scaling == Scaling::Stretched {
                steps.done(StepKind::Scaling);
            } else {
                steps.failed(StepKind::Scaling);
                warnings.push(
                    AppError::ScalingNotApplied(Box::new(
                        Failure::new(format!(
                            "full-screen scaling was asked for; the path reads {:?}",
                            live.scaling
                        ))
                        .with_context(ErrorContext {
                            monitor: name.clone(),
                            mode: Some(to_mode(now_mode)),
                            ..ErrorContext::default()
                        }),
                    ))
                    .to_ipc(),
                );
            }
        }
        // Step 10: a countdown only for a mode not yet confirmed on that monitor.
        let baseline_state = self
            .inner
            .journal
            .baseline_of(&monitor.id)
            .map(|record| record.baseline);
        let baseline_mode = baseline_state.as_ref().map(DisplayState::raw_mode);
        let confirmed = {
            let state = self.lock();
            is_confirmed(
                &state.settings.confirmed_modes,
                &after,
                &live.id,
                baseline_mode,
                listed,
            )
        };
        let monitors: Vec<_> = after.iter().map(to_monitor).collect();
        if confirmed {
            if let Err(error) = self.inner.journal.advance(change_id, ChangeStatus::Applied) {
                log::warn!("change {change_id}: the applied status was not written: {error}");
            }
            let change = ActiveChange {
                change_id: change_id.to_owned(),
                monitor: live.id.clone(),
                monitor_name: name,
                before: to_mode(baseline_mode.unwrap_or(previous.raw_mode())),
                now: to_mode(now_mode),
                scaling: live.scaling,
                turned_off: Vec::new(),
                profile_id: target.profile_id.clone(),
                source,
                since_ms: self.inner.clock.epoch_ms(),
            };
            // Back at the baseline: nothing of ResSwap's is left on this monitor.
            let at_baseline = baseline_state
                .as_ref()
                .is_some_and(|state| shows_state(&live, state));
            if at_baseline {
                self.close_monitor_entries(&live.id);
            }
            self.update(|state| {
                state.monitors = monitors;
                state.busy = None;
                keep_active(state, &live.id, (!at_baseline).then(|| change.clone()));
            });
            return Ok(ApplyOutcome::Applied {
                change,
                steps: steps.list(),
                warnings,
            });
        }
        if let Err(error) = self.inner.journal.advance(change_id, ChangeStatus::Pending) {
            log::warn!("change {change_id}: the pending status was not written: {error}");
        }
        let secs = self.lock().settings.revert_countdown_secs;
        let pending = PendingChange {
            change_id: change_id.to_owned(),
            monitor: live.id.clone(),
            monitor_name: name,
            from: to_mode(previous.raw_mode()),
            to: to_mode(now_mode),
            scaling: live.scaling,
            countdown_secs: secs,
            seconds_left: secs,
            source,
        };
        self.update(|state| {
            state.monitors = monitors;
            state.busy = None;
            state.pending = Some(pending.clone());
        });
        let sink = self.sink();
        if sink.has_listeners() {
            emit_mode_pending_confirm(sink.as_ref(), &pending);
        }
        self.start_countdown(change_id, secs);
        // After state-changed and mode-pending-confirm, so the window's first read sees it.
        if let Err(error) = self.keep_mode().open() {
            log::warn!("change {change_id}: {error}; the countdown still reverts it");
        }
        Ok(ApplyOutcome::PendingConfirm {
            pending,
            steps: steps.list(),
            warnings,
        })
    }

    /// The apply call failed, the read after it failed, or the read-back differs: the
    /// previous state goes back, the entry closes, and the error is MODE_REJECTED
    /// (REQ-DISP-13), or INTERNAL with the read error when the read failed, because no other
    /// code describes a failed read (REQ-UI-09). When putting it back fails too, or leaves the
    /// scaling changed, the entry stays open and the error is RECOVERY_INCOMPLETE. When the
    /// apply call found the monitor gone, or the read after it no longer lists the monitor,
    /// the error is MONITOR_GONE and its entry stays for when it returns (REQ-SAFE-09).
    #[allow(
        clippy::too_many_arguments,
        reason = "the parts of one failed change that its error names"
    )]
    fn reject(
        &self,
        record: &ChangeRecord,
        monitor: &RawMonitor,
        requested: Mode,
        name: Option<String>,
        cause: Rejection,
        busy: BusyAction,
        steps: &mut Steps,
    ) -> AppError {
        let rejected = match cause {
            Rejection::Apply(error) if error.code() == ErrorCode::MonitorGone => {
                self.mark_missing(&monitor.id);
                let detail = error.to_string();
                return monitor_gone(name, &detail).with_steps(steps.list());
            }
            // Its entry stays, as above, so the change goes back once the monitor returns.
            Rejection::Gone => {
                self.mark_missing(&monitor.id);
                if let Err(error) = self.refresh_monitors() {
                    log::warn!("after a monitor went away, the monitors were not read: {error}");
                }
                return monitor_gone(
                    name,
                    "the read after the apply call no longer lists it; the change stays recorded                      and goes back when the monitor returns",
                )
                .with_steps(steps.list());
            }
            Rejection::Apply(error) => error,
            Rejection::ReadBack(error) => AppError::internal(format!(
                "the apply call succeeded, but the read after it failed, so the mode was not \
                 verified: {error}"
            )),
            Rejection::Differs => AppError::ModeRejected(Box::new(Failure::new(
                "the mode read back differs from the one asked for",
            ))),
        };
        match self.put_back(record, busy) {
            PutBack::Done(restored, after) => {
                steps.done(StepKind::Restore);
                if let Err(error) = self.inner.journal.close(&[record.change_id.as_str()]) {
                    log::warn!("change {}: not closed: {error}", record.change_id);
                }
                let monitors = after.iter().map(to_monitor).collect();
                self.update(|state| state.monitors = monitors);
                let restored = to_mode(restored);
                // INTERNAL carries no context or steps, so its detail says how it ended.
                let rejected = match rejected {
                    AppError::Internal { detail } => AppError::internal(format!(
                        "{detail}; the previous mode went back and read back as {}",
                        restored.label
                    )),
                    coded => coded,
                };
                rejected
                    .with_context(|context| {
                        context.monitor = name;
                        context.mode = Some(requested);
                        context.current = Some(restored);
                    })
                    .with_steps(steps.list())
            }
            PutBack::ScalingDiffers {
                restored,
                scaling,
                after,
                detail,
            } => {
                steps.failed(StepKind::Restore);
                let restored = to_mode(restored);
                // The entry stays open with the previous mode on screen, so it shows as an
                // active change that Restore can still put back.
                let before = self.inner.journal.baseline_of(&monitor.id).map_or_else(
                    || to_mode(record.previous.raw_mode()),
                    |base| to_mode(base.baseline.raw_mode()),
                );
                let change = ActiveChange {
                    change_id: record.change_id.clone(),
                    monitor: monitor.id.clone(),
                    monitor_name: name.clone(),
                    before,
                    now: restored.clone(),
                    scaling,
                    turned_off: Vec::new(),
                    profile_id: record.profile_id.clone(),
                    source: record.source,
                    since_ms: self.inner.clock.epoch_ms(),
                };
                let monitors = after.iter().map(to_monitor).collect();
                self.update(|state| {
                    state.monitors = monitors;
                    keep_active(state, &monitor.id, Some(change));
                });
                self.recovery_incomplete(&format!(
                    "{rejected}; the previous mode went back, but {detail}"
                ))
                .with_context(|context| {
                    context.monitor = name;
                    context.mode = Some(requested);
                    context.current = Some(restored);
                })
                .with_steps(steps.list())
            }
            PutBack::NotStarted(error) | PutBack::Failed(error) => {
                steps.failed(StepKind::Restore);
                self.recovery_incomplete(&format!("{rejected}; putting it back failed: {error}"))
                    .with_steps(steps.list())
            }
        }
    }

    /// RECOVERY_INCOMPLETE with everything the journal still lists, so the window can show
    /// what is still changed and offer Restore everything (REQ-SAFE-17).
    pub(crate) fn recovery_incomplete(&self, detail: &str) -> AppError {
        let live = self.inner.display.enumerate().unwrap_or_default();
        let still_changed = self.inner.journal.items(|id| self.monitor_name(id, &live));
        AppError::RecoveryIncomplete(Box::new(Failure::new(detail).with_context(ErrorContext {
            still_changed,
            ..ErrorContext::default()
        })))
    }

    /// Puts the state before `record` back on its monitor: status reverting first, which
    /// gives the Ticket, then the previous mode on the route the change used, on that monitor
    /// only (D-024), then a fresh read that must show the previous mode and scaling
    /// (REQ-SAFE-17). `busy` is the operation the window shows while each step runs.
    pub(crate) fn put_back(&self, record: &ChangeRecord, busy: BusyAction) -> PutBack {
        let Some(monitor) = record.monitor.clone() else {
            return PutBack::Failed(AppError::internal("a change with no monitor"));
        };
        self.set_busy(busy, StepKind::Journal);
        let ticket = match self.inner.journal.begin_revert(&record.change_id) {
            Ok(ticket) => ticket,
            Err(error) => return PutBack::NotStarted(error),
        };
        let previous = record.previous.raw_mode();
        let config = DisplayConfig {
            targets: vec![TargetConfig {
                monitor: monitor.clone(),
                mode: previous,
                scaling: scaling_choice_for(record.previous.scaling),
                // The recorded scaling itself, as far as the route can write it (D-035).
                exact_scaling: Some(record.previous.scaling),
            }],
        };
        self.set_busy(busy, StepKind::Restore);
        if let Err(error) = self.inner.display.apply_config(&ticket, &config) {
            return PutBack::Failed(error);
        }
        self.set_busy(busy, StepKind::Verify);
        let after = match self.inner.display.enumerate() {
            Ok(after) => after,
            Err(error) => return PutBack::Failed(error),
        };
        let Some((current, scaling)) = find_monitor(&monitor, &after)
            .and_then(|live| live.current.map(|current| (current, live.scaling)))
        else {
            return PutBack::Failed(monitor_gone(None, "the monitor is gone after the put-back"));
        };
        if !read_back_verifies(previous, current, &[previous.refresh]) {
            return PutBack::Failed(AppError::ModeRejected(Box::new(Failure::new(format!(
                "the put-back read back {} x {} at {}/{}",
                current.width,
                current.height,
                current.refresh.numerator,
                current.refresh.denominator
            )))));
        }
        if scaling != record.previous.scaling {
            // The previous scaling can only be asked for as full screen or as the path's own
            // value, and the driver may keep another one (REQ-NV-25), so the read decides.
            let detail = format!(
                "the scaling reads {scaling:?} where the state before the change had {:?}",
                record.previous.scaling
            );
            log::warn!(
                "change {}: the put-back verified the mode, but {detail}; the entry stays open",
                record.change_id
            );
            return PutBack::ScalingDiffers {
                restored: current,
                scaling,
                after,
                detail,
            };
        }
        PutBack::Done(current, after)
    }

    /// The active change a pending change becomes when it ends without a verified revert:
    /// its entry stays open, so Restore can still put it back (D-025).
    pub(crate) fn pending_as_active(
        &self,
        pending: &PendingChange,
        now: Mode,
        scaling: Scaling,
    ) -> ActiveChange {
        let before = self
            .inner
            .journal
            .baseline_of(&pending.monitor)
            .map_or_else(
                || pending.from.clone(),
                |base| to_mode(base.baseline.raw_mode()),
            );
        ActiveChange {
            change_id: pending.change_id.clone(),
            monitor: pending.monitor.clone(),
            monitor_name: pending.monitor_name.clone(),
            before,
            now,
            scaling,
            turned_off: Vec::new(),
            profile_id: self
                .inner
                .journal
                .change(&pending.change_id)
                .and_then(|record| record.profile_id),
            source: pending.source,
            since_ms: self.inner.clock.epoch_ms(),
        }
    }

    /// Closes every open change on `monitor` once it shows its baseline again.
    pub(crate) fn close_monitor_entries(&self, monitor: &MonitorId) {
        let ids: Vec<String> = self
            .inner
            .journal
            .open_changes_on(monitor)
            .into_iter()
            .map(|change| change.change_id)
            .collect();
        let refs: Vec<&str> = ids.iter().map(String::as_str).collect();
        if let Err(error) = self.inner.journal.close(&refs) {
            log::warn!("the entries of a restored monitor were not closed: {error}");
        }
    }

    /// Records a journaled monitor as gone, so its baseline goes back when it returns
    /// (REQ-SAFE-09).
    pub(crate) fn mark_missing(&self, monitor: &MonitorId) {
        let mut missing = guard(&self.inner.missing);
        if !missing.iter().any(|known| same_monitor(known, monitor)) {
            missing.push(monitor.clone());
        }
    }

    /// Keep (confirm_mode): the pending change becomes an active change with the same
    /// changeId, its mode is stored as confirmed on that monitor (REQ-DISP-19), and the
    /// keep-mode window closes. A change that is no longer pending answers notPending.
    pub async fn confirm(
        &self,
        request: ChangeRef,
        _source: ChangeSource,
    ) -> Result<ConfirmOutcome, AppError> {
        self.scripted(CommandName::ConfirmMode)?;
        check_change_id(&request.change_id)?;
        let _lock = self.lock_changes_for(None).await;
        Ok(self.confirm_locked(&request.change_id))
    }

    fn confirm_locked(&self, change_id: &str) -> ConfirmOutcome {
        let Some(pending) = self
            .lock()
            .pending
            .clone()
            .filter(|pending| pending.change_id == change_id)
        else {
            return ConfirmOutcome::NotPending;
        };
        self.stop_countdown(change_id);
        if let Err(error) = self.inner.journal.advance(change_id, ChangeStatus::Kept) {
            log::warn!("change {change_id}: the kept status was not written: {error}");
        }
        let mut warnings = Vec::new();
        // Kept now; if settings.json cannot be written, it asks again next time.
        if let Err(error) = self
            .inner
            .settings
            .add_confirmed_mode(&pending.monitor, &pending.to)
        {
            log::warn!("change {change_id}: the confirmed mode was not saved: {error}");
            warnings.push(error.to_ipc());
        }
        let profile_id = self
            .inner
            .journal
            .change(change_id)
            .and_then(|record| record.profile_id);
        let before = self
            .inner
            .journal
            .baseline_of(&pending.monitor)
            .map_or_else(
                || pending.from.clone(),
                |record| to_mode(record.baseline.raw_mode()),
            );
        let change = ActiveChange {
            change_id: change_id.to_owned(),
            monitor: pending.monitor.clone(),
            monitor_name: pending.monitor_name.clone(),
            before,
            now: pending.to.clone(),
            scaling: pending.scaling,
            turned_off: Vec::new(),
            profile_id,
            source: pending.source,
            since_ms: self.inner.clock.epoch_ms(),
        };
        let settings = self.inner.settings.settings();
        self.update(|state| {
            state.pending = None;
            state.settings = settings;
            keep_active(state, &pending.monitor, Some(change.clone()));
        });
        self.wake_waiters();
        self.keep_mode().close();
        log::info!("change {change_id}: kept");
        ConfirmOutcome::Kept {
            change: Box::new(change),
            warnings,
        }
    }

    /// revert_mode, and every other way a pending change goes back.
    pub async fn revert(
        &self,
        request: ChangeRef,
        reason: RevertReason,
    ) -> Result<RevertOutcome, AppError> {
        self.scripted(CommandName::RevertMode)?;
        check_change_id(&request.change_id)?;
        let _lock = self.lock_changes_for(None).await;
        self.revert_locked(&request.change_id, reason)
    }

    /// Puts the pending change `change_id` back, under the change lock (REQ-SAFE-03): the
    /// countdown stops only once the reverting status is on disk, which gives the Ticket
    /// (REQ-SAFE-05). On success, mode-reverted. When that status cannot be written, nothing
    /// was tried: the change stays pending with its countdown running, which reverts at its
    /// end and after its end tries again each second, and the error is JOURNAL_WRITE_FAILED.
    /// When the put-back fails, or leaves the scaling changed, the entry stays open as an
    /// active change and the error is RECOVERY_INCOMPLETE. A change that is no longer pending
    /// answers notPending.
    pub(crate) fn revert_locked(
        &self,
        change_id: &str,
        reason: RevertReason,
    ) -> Result<RevertOutcome, AppError> {
        let Some(pending) = self
            .lock()
            .pending
            .clone()
            .filter(|pending| pending.change_id == change_id)
        else {
            return Ok(RevertOutcome::NotPending);
        };
        let Some(record) = self.inner.journal.change(change_id) else {
            // Nothing on disk can put it back, so a later second could not either: the
            // pending change ends here.
            self.end_pending(change_id);
            return Err(AppError::internal(
                "the pending change has no journal entry",
            ));
        };
        let mut steps = Steps::default();
        let error = match self.put_back(&record, BusyAction::Reverting) {
            PutBack::NotStarted(error) if error.code() == ErrorCode::JournalWriteFailed => {
                // The countdown keeps running and AppState.pending stays, so the keep-mode
                // window stays and the countdown's own revert still comes; Revert and
                // Restore everything can try too. The changes waiting behind it keep waiting.
                steps.failed(StepKind::Journal);
                log::warn!("change {change_id}: the revert ({reason:?}) did not start: {error}");
                self.clear_busy();
                return Err(error.with_steps(steps.list()));
            }
            PutBack::NotStarted(error) => {
                // The entry is in a status a revert cannot follow, so another try would fail
                // the same way: the change ends as below.
                steps.failed(StepKind::Journal);
                error
            }
            PutBack::Failed(error) => {
                steps.failed(StepKind::Restore);
                error
            }
            PutBack::ScalingDiffers {
                restored,
                scaling,
                after,
                detail,
            } => {
                self.stop_countdown(change_id);
                steps.done(StepKind::Restore);
                steps.failed(StepKind::Verify);
                let restored = to_mode(restored);
                let change = self.pending_as_active(&pending, restored.clone(), scaling);
                let monitors = after.iter().map(to_monitor).collect();
                self.update(|state| {
                    state.monitors = monitors;
                    state.busy = None;
                    state.pending = None;
                    keep_active(state, &pending.monitor, Some(change));
                });
                self.wake_waiters();
                self.keep_mode().close();
                log::warn!(
                    "change {change_id}: the revert ({reason:?}) put the mode back, but {detail}"
                );
                return Err(self
                    .recovery_incomplete(&format!(
                        "putting back the pending change left the scaling changed: {detail}"
                    ))
                    .with_context(|context| {
                        context.monitor.clone_from(&pending.monitor_name);
                        context.mode = Some(pending.to.clone());
                        context.current = Some(restored);
                    })
                    .with_steps(steps.list()));
            }
            PutBack::Done(restored, after) => {
                // The reverting status is on disk, so the countdown has done its work.
                self.stop_countdown(change_id);
                steps.done(StepKind::Restore);
                steps.done(StepKind::Verify);
                if let Err(error) = self.inner.journal.close(&[change_id]) {
                    log::warn!("change {change_id}: not closed after its revert: {error}");
                }
                let live = find_monitor(&pending.monitor, &after).cloned();
                let at_baseline = live.as_ref().is_some_and(|live| {
                    self.inner
                        .journal
                        .baseline_of(&live.id)
                        .is_none_or(|record| shows_state(live, &record.baseline))
                });
                if at_baseline {
                    self.close_monitor_entries(&pending.monitor);
                }
                let reverted = ModeReverted {
                    change_id: change_id.to_owned(),
                    monitor: pending.monitor.clone(),
                    monitor_name: pending.monitor_name.clone(),
                    reason,
                    restored: to_mode(restored),
                    steps: steps.list(),
                };
                let monitors = after.iter().map(to_monitor).collect();
                self.update(|state| {
                    state.monitors = monitors;
                    state.busy = None;
                    state.pending = None;
                    if at_baseline {
                        keep_active(state, &pending.monitor, None);
                    }
                });
                self.wake_waiters();
                let sink = self.sink();
                if sink.has_listeners() {
                    emit_mode_reverted(sink.as_ref(), &reverted);
                }
                self.keep_mode().close();
                log::info!(
                    "change {change_id}: reverted ({reason:?}), {}",
                    steps.summary()
                );
                return Ok(RevertOutcome::Reverted { reverted });
            }
        };
        // The put-back was tried and failed, or cannot be tried: another second would not do
        // better, so the countdown stops and the change stays open as an active change that
        // Restore everything can still put back.
        self.stop_countdown(change_id);
        let change = self.pending_as_active(&pending, pending.to.clone(), pending.scaling);
        self.update(|state| {
            state.busy = None;
            state.pending = None;
            keep_active(state, &pending.monitor, Some(change));
        });
        self.wake_waiters();
        self.keep_mode().close();
        log::warn!("change {change_id}: the revert ({reason:?}) failed: {error}");
        Err(self
            .recovery_incomplete(&format!("putting back the pending change failed: {error}"))
            .with_context(|context| {
                context.monitor.clone_from(&pending.monitor_name);
                context.mode = Some(pending.to.clone());
            })
            .with_steps(steps.list()))
    }

    /// Ends a pending change that nothing can put back any more: its countdown stops, the
    /// keep-mode window closes and the changes waiting behind it run.
    fn end_pending(&self, change_id: &str) {
        self.stop_countdown(change_id);
        self.update(|state| {
            state.busy = None;
            if state
                .pending
                .as_ref()
                .is_some_and(|pending| pending.change_id == change_id)
            {
                state.pending = None;
            }
        });
        self.wake_waiters();
        self.keep_mode().close();
    }

    /// restore_native: each monitor ResSwap changed, or the one named, back to its native
    /// size at the baseline rational when offered there (REQ-DISP-21), through the same steps
    /// as any apply. A pending change on one of those monitors goes back first (reason
    /// Superseded).
    pub async fn restore_native(
        &self,
        request: RestoreNativeRequest,
        source: ChangeSource,
    ) -> Result<RestoreOutcome, AppError> {
        self.scripted(CommandName::RestoreNative)?;
        if let Some(monitor) = &request.monitor {
            check_monitor_id(monitor)?;
        }
        let _lock = self.lock_changes_for(request.monitor.as_ref()).await;
        let targets: Vec<MonitorId> = match &request.monitor {
            Some(monitor) => vec![monitor.clone()],
            None => self
                .inner
                .journal
                .snapshot()
                .monitors
                .into_iter()
                .map(|record| record.identity)
                .collect(),
        };
        let pending = self.lock().pending.clone();
        if let Some(pending) = pending
            && targets
                .iter()
                .any(|monitor| same_monitor(monitor, &pending.monitor))
        {
            self.revert_locked(&pending.change_id, RevertReason::Superseded)?;
        }
        let mut restored = Vec::new();
        let mut steps = Steps::default();
        for wanted in targets {
            let live = self.inner.display.enumerate()?;
            let Some(monitor) = find_monitor(&wanted, &live).cloned() else {
                if request.monitor.is_some() {
                    return Err(monitor_gone(
                        self.monitor_name(&wanted, &live),
                        "no connected monitor matches the request",
                    ));
                }
                log::warn!("Restore native left out a monitor that is not connected");
                continue;
            };
            let baseline = self
                .inner
                .journal
                .baseline_of(&monitor.id)
                .map(|record| record.baseline);
            let native =
                restore_native_target(&monitor, baseline.as_ref().map(|state| state.refresh))?;
            let on_native = monitor
                .current
                .is_some_and(|current| same_mode(current, native));
            if on_native {
                // Already there: nothing to apply. Entries close only when the baseline
                // shows too.
                if baseline
                    .as_ref()
                    .is_some_and(|state| shows_state(&monitor, state))
                {
                    self.close_monitor_entries(&monitor.id);
                    self.update(|state| keep_active(state, &monitor.id, None));
                    restored.push(monitor.id.clone());
                }
                continue;
            }
            let target = Target {
                monitor: monitor.id.clone(),
                width: native.width,
                height: native.height,
                rate: native.refresh,
                scaling: ScalingChoice::Keep,
                profile_id: None,
                allow_hidden: true,
            };
            let outcome = self.change_mode(&target, source, BusyAction::Restoring)?;
            let (ApplyOutcome::Applied { steps: done, .. }
            | ApplyOutcome::PendingConfirm { steps: done, .. }) = outcome;
            steps.extend(&Steps(done));
            restored.push(monitor.id.clone());
        }
        Ok(RestoreOutcome {
            restored,
            steps: steps.list(),
        })
    }

    /// restore_everything: a pending change goes back first (reason Superseded), then every
    /// open entry returns to its baseline from the saved snapshot, with
    /// SDC_USE_DATABASE_CURRENT as the fallback (REQ-DISP-22, D-024). A monitor that is not
    /// connected keeps its entries and is listed as still changed (D-025).
    pub async fn restore_everything(
        &self,
        source: ChangeSource,
    ) -> Result<RestoreOutcome, AppError> {
        self.scripted(CommandName::RestoreEverything)?;
        let _lock = self.lock_changes_for(None).await;
        let pending = self.lock().pending.clone();
        if let Some(pending) = pending
            && let Err(error) = self.revert_locked(&pending.change_id, RevertReason::Superseded)
        {
            // The entry stays open, so the restore below takes it.
            log::warn!("Restore everything: the pending change did not go back: {error}");
        }
        if !self.inner.journal.needs_recovery() {
            if self.lock().recovery.status == RecoveryStatus::Incomplete {
                return self.restore_without_journal(source);
            }
            return Ok(RestoreOutcome {
                restored: Vec::new(),
                steps: Vec::new(),
            });
        }
        let putback = self.restore_baselines(None, true)?;
        if putback.left.is_empty() {
            // A moved-aside journal may still have a change on screen: recovery stays
            // incomplete, so the next Restore everything puts the saved configuration back
            // (D-023, D-036).
            let journal_lost = self.inner.journal_lost.load(Ordering::Relaxed);
            if !self.inner.journal.needs_recovery() && !journal_lost {
                self.update(settle_recovery);
            }
            return Ok(RestoreOutcome {
                restored: putback.restored,
                steps: putback.steps.list(),
            });
        }
        let error = self
            .recovery_incomplete("Restore everything left something changed")
            .with_steps(putback.steps.list());
        let still = error.to_ipc().context.still_changed;
        self.update(|state| {
            if state.recovery.status == RecoveryStatus::Incomplete {
                state.recovery.still_changed = still;
            }
        });
        Err(error)
    }
}

/// Checks a mode from a request: a size above 0 and a rate with a denominator above 0
/// (REQ-SEC-08). Anything else is INTERNAL.
fn check_mode(mode: &Mode) -> Result<(), AppError> {
    if mode.width == 0 || mode.height == 0 || mode.refresh.denominator == 0 {
        return Err(AppError::internal(format!(
            "refused a mode request of {} x {} at {}/{}",
            mode.width, mode.height, mode.refresh.numerator, mode.refresh.denominator
        )));
    }
    Ok(())
}

/// Sets the one active change of `monitor`: `Some` replaces it, `None` removes it.
pub(crate) fn keep_active(
    state: &mut CoreState,
    monitor: &MonitorId,
    change: Option<ActiveChange>,
) {
    state
        .active
        .retain(|active| !same_monitor(&active.monitor, monitor));
    if let Some(change) = change {
        state.active.push(change);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn req_sec_08_change_ids_are_lowercase_uuids() {
        let first = new_change_id();
        let second = new_change_id();
        assert!(is_lowercase_uuid(&first), "{first}");
        assert!(is_lowercase_uuid(&second), "{second}");
        assert_ne!(first, second);
        assert_eq!(first.as_bytes()[14], b'4');
        assert!(check_change_id(&first).is_ok());
    }

    #[test]
    fn req_sec_08_requests_are_checked_in_rust() {
        for refused in ["", "change id", "change\n1", &"a".repeat(65)] {
            assert!(check_change_id(refused).is_err(), "{refused:?}");
        }
        let id = |path: &str| MonitorId {
            device_path: path.to_owned(),
            edid_manufacturer_id: None,
            edid_product_code: None,
            connector_instance: 0,
        };
        assert!(check_monitor_id(&id("fake-device-path:test-pc:lg-ultragear")).is_ok());
        for refused in [
            String::new(),
            "a".repeat(128),
            String::from("line\u{7}bell"),
        ] {
            assert!(check_monitor_id(&id(&refused)).is_err(), "{refused:?}");
        }
    }

    #[test]
    fn req_disp_26_empty_device_path_needs_both_edid_ids() {
        // The LG on the test PC with its monitor device disabled in Device Manager, as
        // resswap-diag read it on 14 Nov 2026: no device path, valid EDID ids.
        let id = |manufacturer: Option<u16>, product: Option<u16>| MonitorId {
            device_path: String::new(),
            edid_manufacturer_id: manufacturer,
            edid_product_code: product,
            connector_instance: 1,
        };
        assert!(check_monitor_id(&id(Some(0x6D1E), Some(0x5B8E))).is_ok());
        for (manufacturer, product) in [(None, None), (Some(0x6D1E), None), (None, Some(0x5B8E))] {
            let refused = check_monitor_id(&id(manufacturer, product)).unwrap_err();
            assert_eq!(refused.code(), ErrorCode::Internal);
        }
        // A path with a control character stays refused even with EDID ids.
        let bell = MonitorId {
            device_path: String::from("line\u{7}bell"),
            ..id(Some(0x6D1E), Some(0x5B8E))
        };
        assert!(check_monitor_id(&bell).is_err());
    }

    #[test]
    fn req_safe_17_step_summary_names_every_step() {
        let mut steps = Steps::default();
        assert_eq!(steps.summary(), "no step");
        steps.done(StepKind::Test);
        steps.failed(StepKind::Apply);
        steps.push(StepKind::Restore, StepStatus::NotRun);
        assert_eq!(steps.summary(), "test done, apply failed, restore not run");
    }
}
