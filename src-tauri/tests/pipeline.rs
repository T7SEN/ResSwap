//! The apply pipeline, the countdown, Restore native, Restore everything, start-up recovery
//! and the reaction to display changes, all on the fakes and the still fake clock, with the
//! fault points INJ-D01 to INJ-D24. No test reaches a backend that changes the PC.

#![allow(
    clippy::unwrap_used,
    reason = "test helpers: a failed step fails the test"
)]

mod common;

use std::path::{Path, PathBuf};

use common::{Recorder, fake_core, wait_for, watched_core};
use resswap_lib::apply::journal::{ChangeStatus, JOURNAL_FILE, JournalFile};
use resswap_lib::display::fake::{
    DisplayCall, DisplayFault, FixtureProfile, OutsideChange, When, known_monitors,
};
use resswap_lib::display::modes::to_mode;
use resswap_lib::display::types::{Mode, MonitorId, Scaling, ScalingChoice};
use resswap_lib::display::{GdiFlag, Rational, RawMode, RouteChoice};
use resswap_lib::error::{AppError, ErrorCode, JournalItemKind, StepKind, StepStatus};
use resswap_lib::ipc::events::CoreEvent;
use resswap_lib::ipc::types::{
    AddCustomModeRequest, ApplyOutcome, ApplyRequest, BackendKind, ChangeRef, ChangeSource,
    ConfirmOutcome, ErrorRef, ProfileRef, RecoveryStatus, RemoveCustomEntryRequest,
    RestoreNativeRequest, RevertOutcome, RevertReason, SetAutoSwitchRequest, SetAutostartRequest,
    SetHotkeyRequest, UndoAllRequest,
};
use resswap_lib::profiles::Profile;
use resswap_lib::settings::atomic::WriteStage;
use resswap_lib::settings::fake::{StoreFault, StoreFile};
use resswap_lib::settings::{HotkeyAction, SettingsInput};
use resswap_lib::shell::fake::{FolderFault, KeepModeCall, KeepModeFault};
use resswap_lib::system::notify::SystemEvent;
use resswap_lib::{Core, Fakes};
use serde_json::Value;
use tauri::async_runtime::block_on;

const ID: &str = "0f8fad5b-d9cb-469f-a165-70867728950e";
const OPEN_CHANGE: &str = include_str!("fixtures/journal-v1-open-change.json");

fn lg(profile: FixtureProfile) -> MonitorId {
    known_monitors(profile).remove(0).id
}

fn lg_id() -> MonitorId {
    lg(FixtureProfile::TestPc)
}

fn raw(width: u32, height: u32, hz: u32) -> RawMode {
    RawMode {
        width,
        height,
        refresh: Rational::hz(hz),
    }
}

fn native() -> RawMode {
    raw(1920, 1080, 240)
}

fn stretched() -> RawMode {
    raw(1280, 960, 240)
}

/// A mode request as a preset sends it: the core's own Mode, unchanged.
fn request(monitor: &MonitorId, mode: RawMode) -> ApplyRequest {
    ApplyRequest::Mode {
        monitor: monitor.clone(),
        mode: to_mode(mode),
        scaling: ScalingChoice::FullScreen,
    }
}

/// A typed size as the custom size form sends it: the label left empty (REQ-DISP-11).
fn typed(monitor: &MonitorId, mode: RawMode) -> ApplyRequest {
    let mut sent = to_mode(mode);
    sent.label = String::new();
    sent.refresh.label = String::new();
    ApplyRequest::Mode {
        monitor: monitor.clone(),
        mode: sent,
        scaling: ScalingChoice::FullScreen,
    }
}

fn apply(core: &Core, request: ApplyRequest) -> Result<ApplyOutcome, AppError> {
    block_on(core.apply(request, ChangeSource::Window))
}

fn pending_id(outcome: &ApplyOutcome) -> String {
    match outcome {
        ApplyOutcome::PendingConfirm { pending, .. } => pending.change_id.clone(),
        ApplyOutcome::Applied { .. } => panic!("expected a pending change: {outcome:?}"),
    }
}

fn change(id: &str) -> ChangeRef {
    ChangeRef {
        change_id: id.to_owned(),
    }
}

fn keep(core: &Core, id: &str) -> ConfirmOutcome {
    block_on(core.confirm(change(id), ChangeSource::Window)).unwrap()
}

/// Applies the stretched mode and keeps it, so the LG holds an active change. Once kept, the
/// mode is confirmed and applies with no countdown.
fn kept_stretched(core: &Core) -> String {
    match apply(core, request(&lg_id(), stretched())).unwrap() {
        ApplyOutcome::PendingConfirm { pending, .. } => {
            assert!(matches!(
                keep(core, &pending.change_id),
                ConfirmOutcome::Kept { .. }
            ));
            pending.change_id
        }
        ApplyOutcome::Applied { change, .. } => change.change_id,
    }
}

fn current(fakes: &Fakes, monitor: &MonitorId) -> Option<RawMode> {
    fakes.display.monitor(monitor).and_then(|live| live.current)
}

fn journal(fakes: &Fakes) -> JournalFile {
    fakes
        .store
        .file(JOURNAL_FILE)
        .map_or_else(JournalFile::empty, |bytes| {
            serde_json::from_slice(&bytes).unwrap()
        })
}

fn apply_calls(fakes: &Fakes) -> usize {
    fakes
        .display
        .calls()
        .iter()
        .filter(|call| matches!(call, DisplayCall::Apply { .. }))
        .count()
}

fn ticks(recorder: &Recorder) -> Vec<u64> {
    recorder
        .payloads(CoreEvent::CountdownTick)
        .iter()
        .map(|tick| tick["secondsLeft"].as_u64().unwrap())
        .collect()
}

fn steps(error: &AppError) -> Vec<(StepKind, StepStatus)> {
    error
        .to_ipc()
        .steps
        .iter()
        .map(|report| (report.step, report.status))
        .collect()
}

fn settings_input(core: &Core) -> SettingsInput {
    let settings = core.snapshot().settings;
    SettingsInput {
        revert_countdown_secs: settings.revert_countdown_secs,
        check_for_updates: settings.check_for_updates,
        show_hidden_monitors: settings.show_hidden_monitors,
        selected_monitor: settings.selected_monitor,
        play_stretched_profile: settings.play_stretched_profile,
    }
}

/// Every command of the contract once, with a well-formed request, straight on the core.
/// A fake that meets a changing call without an open journal entry panics, which fails the
/// test here.
fn run_every_command(core: &Core, monitor: &MonitorId) {
    let mode = to_mode(stretched());
    let profile = Profile {
        id: String::from(ID),
        name: String::from("Example"),
        exe: String::from("game.exe"),
        monitor: monitor.clone(),
        mode: mode.clone(),
        scaling: ScalingChoice::FullScreen,
        turn_off: Vec::new(),
        auto_switch: false,
        restore_delay_secs: 10,
        launch_path: None,
    };
    let source = ChangeSource::Window;
    let _ = block_on(core.list_monitors());
    let _ = block_on(core.state());
    let _ = block_on(core.apply(
        ApplyRequest::Mode {
            monitor: monitor.clone(),
            mode,
            scaling: ScalingChoice::FullScreen,
        },
        source,
    ));
    let _ = block_on(core.apply(
        ApplyRequest::Profile {
            profile_id: String::from(ID),
        },
        source,
    ));
    let _ = block_on(core.confirm(change("change-1"), source));
    let _ = block_on(core.revert(change("change-1"), RevertReason::Player));
    let _ = block_on(core.restore_native(
        RestoreNativeRequest {
            monitor: Some(monitor.clone()),
        },
        source,
    ));
    let _ = block_on(core.restore_everything(source));
    let _ = block_on(core.profiles());
    let _ = block_on(core.save_profile(profile));
    let _ = block_on(core.delete_profile(ProfileRef {
        profile_id: String::from(ID),
    }));
    let _ = block_on(core.set_auto_switch(SetAutoSwitchRequest { enabled: true }));
    let _ = block_on(core.custom_entries());
    let _ = block_on(core.add_custom_mode(
        AddCustomModeRequest {
            monitor: monitor.clone(),
            width: 1280,
            height: 960,
            approved_plan: Some(String::from("plan-1")),
        },
        source,
    ));
    let _ = block_on(core.remove_custom_entry(
        RemoveCustomEntryRequest {
            entry_id: String::from("fake-nvapi-custom-display-1"),
            approved_plan: Some(String::from("plan-1")),
        },
        source,
    ));
    let _ = block_on(core.undo_all_changes(
        UndoAllRequest {
            approved_plan: Some(String::from("plan-1")),
        },
        source,
    ));
    let _ = block_on(core.settings());
    let _ = block_on(core.save_settings(SettingsInput {
        revert_countdown_secs: 10,
        check_for_updates: false,
        show_hidden_monitors: true,
        selected_monitor: Some(monitor.clone()),
        play_stretched_profile: Some(String::from(ID)),
    }));
    let _ = block_on(core.set_hotkey(SetHotkeyRequest {
        action: HotkeyAction::PlayStretched,
        accelerator: Some(String::from("Ctrl+Alt+F9")),
    }));
    let _ = block_on(core.set_autostart(SetAutostartRequest { enabled: true }));
    let _ = block_on(core.create_play_shortcut(ProfileRef {
        profile_id: String::from(ID),
    }));
    let _ = block_on(core.check_for_update());
    let _ = block_on(core.install_update(source));
    let _ = block_on(core.open_log_folder());
    let _ = block_on(core.diagnostics());
    let _ = block_on(core.dismiss_error(ErrorRef {
        error_id: String::from("error-1"),
    }));
}

/// The integration form of pc_changes_need_ticket: every command on every fixture runs on
/// the fakes, and no changing call reaches a fake before its journal entry is open (the fake
/// panics if one does). The other half of the rule, a changing call with a closed entry,
/// needs a Ticket, which only the apply module can build; each fake's unit test covers it.
// Also REQ-SAFE-01, REQ-SAFE-17.
#[test]
fn req_disp_01_pc_changes_need_ticket() {
    for profile in [
        FixtureProfile::TestPc,
        FixtureProfile::NotNvidia,
        FixtureProfile::TwoMonitors,
    ] {
        let (core, fakes) = fake_core(profile);
        run_every_command(&core, &lg(profile));
        // Every changing call carried the ID of an entry the journal wrote first.
        for call in fakes.display.calls() {
            if let DisplayCall::Apply { change_id, .. }
            | DisplayCall::ApplyDatabaseCurrent { change_id }
            | DisplayCall::RestoreSnapshot { change_id, .. } = call
            {
                assert!(!change_id.is_empty(), "{profile:?}");
            }
        }
        assert!(fakes.registry.calls().is_empty(), "{profile:?}");
        assert!(fakes.processes.calls().is_empty(), "{profile:?}");
        assert!(!fakes.vendor.changed_anything(), "{profile:?}");
        assert!(!fakes.runner.ran_anything(), "{profile:?}");
    }
}

/// Every fake file in src, so a new fake is checked as soon as it exists.
fn fake_files() -> Vec<PathBuf> {
    fn visit(dir: &Path, found: &mut Vec<PathBuf>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                visit(&path, found);
            } else if path.file_name().is_some_and(|name| name == "fake.rs") {
                found.push(path);
            }
        }
    }
    let mut found = Vec::new();
    visit(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("src"),
        &mut found,
    );
    found.sort();
    found
}

fn has_word(text: &str, word: &str) -> bool {
    text.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .any(|token| token == word)
}

/// No fake imports the windows crate, opens a file, starts a process, opens a socket or
/// holds a block the compiler cannot check, so a fake-backend run cannot touch the PC.
#[test]
fn req_disp_01_fakes_make_no_os_call() {
    let files = fake_files();
    let names: Vec<String> = files
        .iter()
        .map(|path| {
            path.strip_prefix(Path::new(env!("CARGO_MANIFEST_DIR")).join("src"))
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/")
        })
        .collect();
    assert_eq!(
        names,
        [
            "display/fake.rs",
            "elevation/fake.rs",
            "settings/fake.rs",
            "shell/fake.rs",
            "system/fake.rs",
            "vendor/fake.rs",
            "vendor/nvidia/fake.rs",
        ]
    );
    for path in &files {
        let text = std::fs::read_to_string(path).unwrap();
        for banned in [
            "windows::",
            "std::process",
            "std::net",
            "std::fs",
            "std::env",
            "Command::new",
            "tauri::",
        ] {
            assert!(!text.contains(banned), "{}: {banned}", path.display());
        }
        assert!(!has_word(&text, "unsafe"), "{}", path.display());
        assert!(
            text.contains("#![forbid(unsafe_code)]"),
            "{}",
            path.display()
        );
    }
}

/// An unconfirmed mode pends with the countdown from 10, one tick per second on the core's
/// clock with AppState.pending.secondsLeft in step, then goes back with reason Timeout
/// (INJ-D14). The window opens after state-changed and mode-pending-confirm, and closes after
/// the revert.
// Also REQ-SAFE-05, REQ-UI-06, REQ-DISP-11.
#[test]
fn req_safe_03_inj_d14_countdown_runs_out() {
    let (core, fakes, recorder, window) = watched_core(FixtureProfile::TestPc);
    let outcome = apply(&core, typed(&lg_id(), stretched())).unwrap();
    let ApplyOutcome::PendingConfirm { pending, steps, .. } = &outcome else {
        panic!("{outcome:?}");
    };
    // The core's own label, never the empty one the window sent.
    assert_eq!(pending.to.label, "1280 x 960 at 240 Hz");
    assert_eq!(pending.from.label, "1920 x 1080 at 240 Hz");
    assert_eq!((pending.countdown_secs, pending.seconds_left), (10, 10));
    let kinds: Vec<StepKind> = steps.iter().map(|report| report.step).collect();
    assert_eq!(
        kinds,
        [
            StepKind::Test,
            StepKind::Journal,
            StepKind::Apply,
            StepKind::Verify,
            StepKind::Scaling
        ]
    );
    assert!(steps.iter().all(|report| report.status == StepStatus::Done));
    let names = recorder.names();
    assert_eq!(
        names[names.len() - 3..],
        ["state-changed", "mode-pending-confirm", "countdown-tick"]
    );
    assert_eq!(window.calls(), [KeepModeCall::Open]);
    assert_eq!(current(&fakes, &lg_id()), Some(stretched()));
    assert_eq!(journal(&fakes).changes[0].status, ChangeStatus::Pending);

    fakes.clock.advance(1_000);
    block_on(core.run_due_timers());
    fakes.clock.advance(2_000);
    block_on(core.run_due_timers());
    assert_eq!(ticks(&recorder), [10, 9, 8, 7]);
    assert_eq!(core.snapshot().pending.unwrap().seconds_left, 7);
    fakes.clock.advance(7_000);
    block_on(core.run_due_timers());
    assert_eq!(ticks(&recorder), [10, 9, 8, 7, 6, 5, 4, 3, 2, 1, 0]);
    let reverted = recorder.payloads(CoreEvent::ModeReverted);
    assert_eq!(reverted.len(), 1);
    assert_eq!(reverted[0]["reason"], "timeout");
    assert_eq!(reverted[0]["restored"]["label"], "1920 x 1080 at 240 Hz");
    assert_eq!(current(&fakes, &lg_id()), Some(native()));
    assert!(core.snapshot().pending.is_none());
    assert!(journal(&fakes).changes.is_empty());
    assert_eq!(window.calls(), [KeepModeCall::Open, KeepModeCall::Close]);
    // Every snapshot sent is newer than the one before.
    let revisions: Vec<u64> = recorder
        .payloads(CoreEvent::StateChanged)
        .iter()
        .map(|state| state["revision"].as_u64().unwrap())
        .collect();
    assert!(
        revisions.windows(2).all(|pair| pair[0] < pair[1]),
        "{revisions:?}"
    );
}

/// The keep-mode window does not open (INJ-D15): the countdown in Rust still reverts.
#[test]
fn req_safe_05_inj_d15_countdown_reverts_without_the_window() {
    let (core, fakes, recorder, window) = watched_core(FixtureProfile::TestPc);
    window.script(KeepModeFault::OpenFails, When::Once);
    let outcome = apply(&core, request(&lg_id(), stretched())).unwrap();
    pending_id(&outcome);
    assert!(!window.is_open());
    fakes.clock.advance(10_000);
    block_on(core.run_due_timers());
    assert_eq!(recorder.payloads(CoreEvent::ModeReverted).len(), 1);
    assert_eq!(current(&fakes, &lg_id()), Some(native()));
}

/// Keep: the active change carries the pending change's changeId, its before stays the
/// baseline, the mode is stored by identity, size and rational (REQ-DISP-19), and the same
/// mode then applies with no countdown and no window.
// Also REQ-DISP-19.
#[test]
fn req_safe_04_kept_mode_applies_without_a_countdown() {
    let (core, fakes, recorder, window) = watched_core(FixtureProfile::TestPc);
    let first = apply(&core, request(&lg_id(), stretched())).unwrap();
    let id = pending_id(&first);
    let ConfirmOutcome::Kept { change, warnings } = keep(&core, &id) else {
        panic!("not kept");
    };
    assert_eq!(change.change_id, id);
    assert_eq!(change.before.label, "1920 x 1080 at 240 Hz");
    assert_eq!(change.now.label, "1280 x 960 at 240 Hz");
    assert!(warnings.is_empty());
    let state = core.snapshot();
    assert!(state.pending.is_none());
    assert_eq!(state.active.len(), 1);
    assert_eq!(state.active[0].change_id, id);
    assert_eq!(state.settings.confirmed_modes.len(), 1);
    assert_eq!(state.settings.confirmed_modes[0].monitor, lg_id());
    assert_eq!(journal(&fakes).changes[0].status, ChangeStatus::Kept);
    let saved: Value = serde_json::from_slice(&fakes.store.file("settings.json").unwrap()).unwrap();
    assert_eq!(
        saved["settings"]["confirmedModes"][0]["mode"]["refresh"]["numerator"],
        240
    );
    assert_eq!(window.calls(), [KeepModeCall::Open, KeepModeCall::Close]);

    // Restore native, then the same mode again: applied, no countdown, no window.
    block_on(core.restore_native(
        RestoreNativeRequest {
            monitor: Some(lg_id()),
        },
        ChangeSource::Window,
    ))
    .unwrap();
    recorder.clear();
    let again = apply(&core, request(&lg_id(), stretched())).unwrap();
    let ApplyOutcome::Applied { change, .. } = again else {
        panic!("{again:?}");
    };
    assert_eq!(change.before.label, "1920 x 1080 at 240 Hz");
    assert!(recorder.payloads(CoreEvent::ModePendingConfirm).is_empty());
    assert!(ticks(&recorder).is_empty());
    assert_eq!(window.calls(), [KeepModeCall::Open, KeepModeCall::Close]);
    // A new core on the same settings.json knows the mode as confirmed.
    let reloaded = Core::new(fakes.backends());
    assert_eq!(reloaded.snapshot().settings.confirmed_modes.len(), 1);
}

/// Keep while settings.json cannot be written (INJ-D16): kept, with a SETTINGS_WRITE_FAILED
/// warning, and nothing stored, so it asks again next time.
#[test]
fn req_disp_19_inj_d16_keep_with_a_settings_write_failure() {
    let (core, fakes, _recorder, _window) = watched_core(FixtureProfile::TestPc);
    let id = pending_id(&apply(&core, request(&lg_id(), stretched())).unwrap());
    fakes.store.script(
        StoreFault::Write(StoreFile::Settings, WriteStage::TempWrite),
        When::Once,
    );
    let ConfirmOutcome::Kept { change, warnings } = keep(&core, &id) else {
        panic!("not kept");
    };
    assert_eq!(change.change_id, id);
    assert_eq!(warnings.len(), 1);
    assert_eq!(warnings[0].code, ErrorCode::SettingsWriteFailed);
    assert!(core.snapshot().settings.confirmed_modes.is_empty());
    assert_eq!(core.snapshot().active.len(), 1);
}

/// Revert, and Keep after the revert (INJ-D17): the previous state goes back, the entry
/// closes, mode-reverted carries reason Player, and a late Keep answers notPending.
#[test]
fn req_safe_03_inj_d17_revert_then_keep_is_not_pending() {
    let (core, fakes, recorder, _window) = watched_core(FixtureProfile::TestPc);
    let id = pending_id(&apply(&core, request(&lg_id(), stretched())).unwrap());
    let reverted = block_on(core.revert(change(&id), RevertReason::Player)).unwrap();
    let RevertOutcome::Reverted { reverted } = reverted else {
        panic!("not reverted");
    };
    assert_eq!(reverted.reason, RevertReason::Player);
    assert_eq!(reverted.restored.label, "1920 x 1080 at 240 Hz");
    assert_eq!(recorder.payloads(CoreEvent::ModeReverted).len(), 1);
    assert!(journal(&fakes).changes.is_empty());
    assert!(matches!(keep(&core, &id), ConfirmOutcome::NotPending));
    let again = block_on(core.revert(change(&id), RevertReason::Player)).unwrap();
    assert!(matches!(again, RevertOutcome::NotPending));
}

/// The keep-mode window closed by its title-bar button: the revert runs in Rust with reason
/// KeepWindowClosed.
#[test]
fn req_safe_03_closing_the_keep_mode_window_reverts() {
    let (core, fakes, recorder, _window) = watched_core(FixtureProfile::TestPc);
    pending_id(&apply(&core, request(&lg_id(), stretched())).unwrap());
    core.keep_window_closed();
    assert!(wait_for(|| core.snapshot().pending.is_none()));
    assert!(wait_for(|| !recorder
        .payloads(CoreEvent::ModeReverted)
        .is_empty()));
    assert_eq!(
        recorder.payloads(CoreEvent::ModeReverted)[0]["reason"],
        "keepWindowClosed"
    );
    assert_eq!(current(&fakes, &lg_id()), Some(native()));
}

/// A new apply on the same monitor supersedes the pending change, which goes back first;
/// Quit puts the pending change back too.
#[test]
fn req_safe_03_superseded_and_quit_revert_first() {
    let (core, fakes, recorder, _window) = watched_core(FixtureProfile::TestPc);
    let first = pending_id(&apply(&core, request(&lg_id(), stretched())).unwrap());
    let second = pending_id(&apply(&core, request(&lg_id(), raw(1024, 768, 240))).unwrap());
    assert_ne!(first, second);
    let reverted = recorder.payloads(CoreEvent::ModeReverted);
    assert_eq!(reverted.len(), 1);
    assert_eq!(reverted[0]["changeId"], first.as_str());
    assert_eq!(reverted[0]["reason"], "superseded");
    assert_eq!(current(&fakes, &lg_id()), Some(raw(1024, 768, 240)));
    // The first change put back the baseline, so only the second is open.
    assert_eq!(journal(&fakes).changes.len(), 1);

    block_on(core.prepare_quit());
    let reverted = recorder.payloads(CoreEvent::ModeReverted);
    assert_eq!(reverted[1]["reason"], "quit");
    assert_eq!(current(&fakes, &lg_id()), Some(native()));
    assert!(journal(&fakes).changes.is_empty());
}

/// One pending change at a time: a change for another monitor waits until the pending one
/// is kept or reverted, then runs.
#[test]
fn req_safe_03_another_monitor_waits_for_the_pending_change() {
    let (core, fakes, _recorder, _window) = watched_core(FixtureProfile::TwoMonitors);
    let lg = lg(FixtureProfile::TwoMonitors);
    let samsung = known_monitors(FixtureProfile::TwoMonitors).remove(2).id;
    let id = pending_id(&apply(&core, request(&lg, stretched())).unwrap());
    let waiting = {
        let core = core.clone();
        let samsung = samsung.clone();
        tauri::async_runtime::spawn(async move {
            core.apply(request(&samsung, raw(1280, 1024, 60)), ChangeSource::Window)
                .await
        })
    };
    std::thread::sleep(std::time::Duration::from_millis(200));
    assert_eq!(current(&fakes, &samsung), Some(raw(1920, 1080, 60)));
    keep(&core, &id);
    let outcome = block_on(waiting).unwrap().unwrap();
    pending_id(&outcome);
    assert_eq!(current(&fakes, &samsung), Some(raw(1280, 1024, 60)));
}

/// Both monitors of the test PC with their devices disabled in Device Manager report no
/// device path (REQ-DISP-26): the LG is applied, kept and restored by its EDID ids and
/// connector instance, and the Samsung beside it is never touched.
#[test]
fn req_disp_26_monitors_without_a_device_path_apply_and_restore() {
    let (core, fakes) = fake_core(FixtureProfile::TwoMonitors);
    let lg = lg(FixtureProfile::TwoMonitors);
    let samsung = known_monitors(FixtureProfile::TwoMonitors).remove(2).id;
    assert!(lg.device_path.is_empty() && samsung.device_path.is_empty());
    let id = pending_id(&apply(&core, request(&lg, stretched())).unwrap());
    assert!(matches!(keep(&core, &id), ConfirmOutcome::Kept { .. }));
    assert_eq!(current(&fakes, &lg), Some(stretched()));
    assert_eq!(current(&fakes, &samsung), Some(raw(1920, 1080, 60)));
    block_on(core.restore_native(
        RestoreNativeRequest {
            monitor: Some(lg.clone()),
        },
        ChangeSource::Window,
    ))
    .unwrap();
    assert_eq!(current(&fakes, &lg), Some(native()));
    assert_eq!(current(&fakes, &samsung), Some(raw(1920, 1080, 60)));
    // The Samsung's own change leaves the LG alone.
    let id = pending_id(&apply(&core, request(&samsung, raw(1280, 1024, 60))).unwrap());
    assert!(matches!(keep(&core, &id), ConfirmOutcome::Kept { .. }));
    assert_eq!(current(&fakes, &samsung), Some(raw(1280, 1024, 60)));
    assert_eq!(current(&fakes, &lg), Some(native()));
}

/// journal.json refuses the next write once, at the rename.
fn refuse_next_journal_write(fakes: &Fakes) {
    fakes.store.script(
        StoreFault::Write(StoreFile::Journal, WriteStage::Rename),
        When::Once,
    );
}

/// The end of the countdown while journal.json refuses the reverting status: nothing is put
/// back, the countdown keeps running with the change pending at 0 seconds and the keep-mode
/// window open, a change for another monitor keeps waiting, and the error is raised once.
/// One second later the revert runs, sends mode-reverted and lets the waiting change run.
// Also REQ-SAFE-01, REQ-SAFE-03.
#[test]
fn req_safe_05_revert_that_cannot_write_tries_again() {
    let (core, fakes, recorder, window) = watched_core(FixtureProfile::TwoMonitors);
    let lg = lg(FixtureProfile::TwoMonitors);
    let samsung = known_monitors(FixtureProfile::TwoMonitors).remove(2).id;
    let id = pending_id(&apply(&core, request(&lg, stretched())).unwrap());
    let waiting = {
        let core = core.clone();
        let samsung = samsung.clone();
        tauri::async_runtime::spawn(async move {
            core.apply(request(&samsung, raw(1280, 1024, 60)), ChangeSource::Window)
                .await
        })
    };
    refuse_next_journal_write(&fakes);
    fakes.clock.advance(10_000);
    block_on(core.run_due_timers());
    assert_eq!(ticks(&recorder).last(), Some(&0));
    assert!(recorder.payloads(CoreEvent::ModeReverted).is_empty());
    assert_eq!(current(&fakes, &lg), Some(stretched()));
    assert_eq!(apply_calls(&fakes), 1);
    let state = core.snapshot();
    let pending = state.pending.unwrap();
    assert_eq!(pending.change_id, id);
    assert_eq!(pending.seconds_left, 0);
    assert!(state.busy.is_none());
    assert_eq!(state.errors.len(), 1);
    assert_eq!(state.errors[0].error.code, ErrorCode::JournalWriteFailed);
    assert!(window.is_open());
    assert_eq!(window.calls(), [KeepModeCall::Open]);
    assert_eq!(journal(&fakes).changes[0].status, ChangeStatus::Pending);
    std::thread::sleep(std::time::Duration::from_millis(200));
    assert_eq!(current(&fakes, &samsung), Some(raw(1920, 1080, 60)));

    // Half a second on, the next try is not due yet.
    fakes.clock.advance(500);
    block_on(core.run_due_timers());
    assert!(recorder.payloads(CoreEvent::ModeReverted).is_empty());
    assert_eq!(apply_calls(&fakes), 1);

    fakes.clock.advance(500);
    block_on(core.run_due_timers());
    let reverted = recorder.payloads(CoreEvent::ModeReverted);
    assert_eq!(reverted.len(), 1);
    assert_eq!(reverted[0]["changeId"], id.as_str());
    assert_eq!(reverted[0]["reason"], "timeout");
    assert_eq!(current(&fakes, &lg), Some(native()));
    assert!(
        journal(&fakes)
            .changes
            .iter()
            .all(|open| open.change_id != id)
    );
    assert_eq!(
        window.calls()[..2],
        [KeepModeCall::Open, KeepModeCall::Close]
    );
    // The waiting change runs once the revert happened.
    let outcome = block_on(waiting).unwrap().unwrap();
    pending_id(&outcome);
    assert_eq!(current(&fakes, &samsung), Some(raw(1280, 1024, 60)));
    let errors = core.snapshot().errors;
    assert_eq!(errors.len(), 1);
    assert_eq!(errors[0].count, 1);
}

/// Revert while journal.json refuses the reverting status: JOURNAL_WRITE_FAILED with the
/// journal step failed, nothing put back, and the change still pending with its countdown
/// running and the keep-mode window open; the countdown then reverts at its end.
// Also REQ-SAFE-05.
#[test]
fn req_safe_03_player_revert_that_cannot_write_stays_pending() {
    let (core, fakes, recorder, window) = watched_core(FixtureProfile::TestPc);
    let id = pending_id(&apply(&core, request(&lg_id(), stretched())).unwrap());
    refuse_next_journal_write(&fakes);
    let error = block_on(core.revert(change(&id), RevertReason::Player)).unwrap_err();
    assert_eq!(error.code(), ErrorCode::JournalWriteFailed);
    assert_eq!(steps(&error), [(StepKind::Journal, StepStatus::Failed)]);
    let state = core.snapshot();
    assert_eq!(state.pending.unwrap().change_id, id);
    assert!(state.busy.is_none());
    assert!(window.is_open());
    assert_eq!(window.calls(), [KeepModeCall::Open]);
    assert_eq!(current(&fakes, &lg_id()), Some(stretched()));
    assert_eq!(apply_calls(&fakes), 1);
    assert_eq!(journal(&fakes).changes[0].status, ChangeStatus::Pending);
    assert!(recorder.payloads(CoreEvent::ModeReverted).is_empty());

    fakes.clock.advance(1_000);
    block_on(core.run_due_timers());
    assert_eq!(ticks(&recorder), [10, 9]);
    fakes.clock.advance(9_000);
    block_on(core.run_due_timers());
    let reverted = recorder.payloads(CoreEvent::ModeReverted);
    assert_eq!(reverted.len(), 1);
    assert_eq!(reverted[0]["reason"], "timeout");
    assert_eq!(current(&fakes, &lg_id()), Some(native()));
    assert!(journal(&fakes).changes.is_empty());
    assert_eq!(window.calls(), [KeepModeCall::Open, KeepModeCall::Close]);
}

/// Quit while journal.json refuses the reverting status: no later second comes to try
/// again, so RECOVERY_INCOMPLETE is raised with what is still changed, the countdown stops,
/// and the entry stays for start-up recovery, which puts the baseline back at the next start.
// Also REQ-SAFE-06.
#[test]
fn req_safe_03_quit_that_cannot_write_leaves_the_entry_for_recovery() {
    let (core, fakes, recorder, _window) = watched_core(FixtureProfile::TestPc);
    pending_id(&apply(&core, request(&lg_id(), stretched())).unwrap());
    refuse_next_journal_write(&fakes);
    block_on(core.prepare_quit());
    let errors = core.snapshot().errors;
    assert_eq!(errors.len(), 1);
    assert_eq!(errors[0].error.code, ErrorCode::RecoveryIncomplete);
    assert_eq!(errors[0].error.context.still_changed.len(), 1);
    assert!(recorder.payloads(CoreEvent::ModeReverted).is_empty());
    assert_eq!(current(&fakes, &lg_id()), Some(stretched()));
    assert_eq!(journal(&fakes).changes[0].status, ChangeStatus::Pending);
    fakes.clock.advance(20_000);
    block_on(core.run_due_timers());
    assert_eq!(apply_calls(&fakes), 1);

    drop(core);
    let next = crashed(&fakes);
    assert_eq!(next.snapshot().recovery.status, RecoveryStatus::Recovered);
    assert_eq!(current(&fakes, &lg_id()), Some(native()));
    assert!(journal(&fakes).changes.is_empty());
}

/// Restore everything while journal.json refuses the pending change's reverting status once:
/// the restore takes the pending change over once its own status is on disk, so the
/// countdown stops and the keep-mode window closes, and the snapshot puts the baseline back.
// Also REQ-SAFE-05.
#[test]
fn req_disp_22_restore_everything_takes_over_a_pending_change() {
    let (core, fakes, recorder, window) = watched_core(FixtureProfile::TestPc);
    pending_id(&apply(&core, request(&lg_id(), stretched())).unwrap());
    refuse_next_journal_write(&fakes);
    let outcome = block_on(core.restore_everything(ChangeSource::Window)).unwrap();
    assert_eq!(outcome.restored, [lg_id()]);
    assert!(recorder.payloads(CoreEvent::ModeReverted).is_empty());
    assert_eq!(current(&fakes, &lg_id()), Some(native()));
    let state = core.snapshot();
    assert!(state.pending.is_none() && state.active.is_empty() && state.busy.is_none());
    assert_eq!(window.calls(), [KeepModeCall::Open, KeepModeCall::Close]);
    assert!(journal(&fakes).changes.is_empty());
    let applies = apply_calls(&fakes);
    fakes.clock.advance(20_000);
    block_on(core.run_due_timers());
    assert_eq!(apply_calls(&fakes), applies);
    assert!(recorder.payloads(CoreEvent::ModeReverted).is_empty());
}

/// The window follows each step as it runs, through state-changed: the busy step moves on
/// from the test to the journal write, the apply, the read-back and the scaling check, and a
/// revert shows its journal write, its put-back and its read-back.
#[test]
fn req_ui_16_busy_names_each_step_as_it_runs() {
    fn busy_steps(recorder: &Recorder) -> Vec<(String, String)> {
        let mut seen: Vec<(String, String)> = Vec::new();
        for state in recorder.payloads(CoreEvent::StateChanged) {
            let busy = &state["busy"];
            if busy.is_null() {
                continue;
            }
            let step = (
                busy["action"].as_str().unwrap().to_owned(),
                busy["step"].as_str().unwrap().to_owned(),
            );
            if seen.last() != Some(&step) {
                seen.push(step);
            }
        }
        seen
    }
    let pair = |action: &str, step: &str| (action.to_owned(), step.to_owned());
    let (core, _fakes, recorder, _window) = watched_core(FixtureProfile::TestPc);
    let id = pending_id(&apply(&core, request(&lg_id(), stretched())).unwrap());
    assert_eq!(
        busy_steps(&recorder),
        [
            pair("applying", "test"),
            pair("applying", "journal"),
            pair("applying", "apply"),
            pair("applying", "verify"),
            pair("applying", "scaling"),
        ]
    );
    assert!(core.snapshot().busy.is_none());
    recorder.clear();
    block_on(core.revert(change(&id), RevertReason::Player)).unwrap();
    assert_eq!(
        busy_steps(&recorder),
        [
            pair("reverting", "journal"),
            pair("reverting", "restore"),
            pair("reverting", "verify"),
        ]
    );
    assert!(core.snapshot().busy.is_none());
}

/// The test refuses (INJ-D06): MODE_TEST_FAILED, no journal write, no apply call.
#[test]
fn req_disp_12_inj_d06_failed_test_changes_nothing() {
    let (core, fakes) = fake_core(FixtureProfile::TestPc);
    fakes
        .display
        .script(DisplayFault::TestRefuses(-2), When::Once);
    let error = apply(&core, request(&lg_id(), stretched())).unwrap_err();
    assert_eq!(error.code(), ErrorCode::ModeTestFailed);
    assert_eq!(steps(&error), [(StepKind::Test, StepStatus::Failed)]);
    let context = error.to_ipc().context;
    assert_eq!(context.monitor.as_deref(), Some("LG ULTRAGEAR"));
    assert_eq!(context.mode.unwrap().label, "1280 x 960 at 240 Hz");
    assert!(fakes.store.file(JOURNAL_FILE).is_none());
    assert_eq!(apply_calls(&fakes), 0);
    assert!(core.snapshot().busy.is_none());
}

/// The journal write fails at each stage (INJ-D07): JOURNAL_WRITE_FAILED and no apply call.
#[test]
fn req_safe_01_inj_d07_journal_write_failure_changes_nothing() {
    for stage in [WriteStage::TempWrite, WriteStage::Sync, WriteStage::Rename] {
        let (core, fakes) = fake_core(FixtureProfile::TestPc);
        fakes
            .store
            .script(StoreFault::Write(StoreFile::Journal, stage), When::Once);
        let error = apply(&core, request(&lg_id(), stretched())).unwrap_err();
        assert_eq!(error.code(), ErrorCode::JournalWriteFailed, "{stage:?}");
        assert_eq!(
            steps(&error),
            [
                (StepKind::Test, StepStatus::Done),
                (StepKind::Journal, StepStatus::Failed)
            ]
        );
        assert_eq!(apply_calls(&fakes), 0, "{stage:?}");
        assert_eq!(current(&fakes, &lg_id()), Some(native()));
    }
}

/// The apply call fails, reads back another size, another rate, or changes nothing
/// (INJ-D08 to INJ-D11): MODE_REJECTED, the previous state put back, the entry closed.
// Also REQ-SAFE-17, REQ-DISP-17.
#[test]
fn req_disp_13_inj_d08_to_d11_rejected_modes_are_put_back() {
    for fault in [
        DisplayFault::ApplyFails(-1),
        DisplayFault::ApplySetsOtherSize {
            width: 1024,
            height: 768,
        },
        DisplayFault::ApplySetsOtherRate(Rational::hz(60)),
        DisplayFault::ApplyChangesNothing,
    ] {
        let (core, fakes, recorder, window) = watched_core(FixtureProfile::TestPc);
        fakes.display.script(fault, When::Once);
        let error = apply(&core, request(&lg_id(), stretched())).unwrap_err();
        assert_eq!(error.code(), ErrorCode::ModeRejected, "{fault:?}");
        let apply_status = if fault == DisplayFault::ApplyFails(-1) {
            StepStatus::Failed
        } else {
            StepStatus::Done
        };
        assert_eq!(
            steps(&error),
            [
                (StepKind::Test, StepStatus::Done),
                (StepKind::Journal, StepStatus::Done),
                (StepKind::Apply, apply_status),
                (StepKind::Verify, StepStatus::Failed),
                (StepKind::Restore, StepStatus::Done),
            ],
            "{fault:?}"
        );
        let context = error.to_ipc().context;
        assert_eq!(context.current.unwrap().label, "1920 x 1080 at 240 Hz");
        assert_eq!(current(&fakes, &lg_id()), Some(native()), "{fault:?}");
        assert!(journal(&fakes).changes.is_empty(), "{fault:?}");
        // No success, no countdown, no window (REQ-DISP-13).
        assert!(recorder.payloads(CoreEvent::ModePendingConfirm).is_empty());
        assert!(window.calls().is_empty());
        let state = core.snapshot();
        assert!(state.pending.is_none() && state.active.is_empty() && state.busy.is_none());
    }
}

/// The put-back fails too (INJ-D13): RECOVERY_INCOMPLETE naming what is still changed, and
/// the entry stays open.
#[test]
fn req_safe_17_inj_d13_failed_put_back_is_recovery_incomplete() {
    let (core, fakes) = fake_core(FixtureProfile::TestPc);
    fakes
        .display
        .script(DisplayFault::ApplyFails(-1), When::Always);
    let error = apply(&core, request(&lg_id(), stretched())).unwrap_err();
    assert_eq!(error.code(), ErrorCode::RecoveryIncomplete);
    let still = error.to_ipc().context.still_changed;
    assert_eq!(still.len(), 1);
    assert_eq!(still[0].kind, JournalItemKind::Mode);
    assert_eq!(still[0].monitor_name.as_deref(), Some("LG ULTRAGEAR"));
    assert_eq!(
        steps(&error).last(),
        Some(&(StepKind::Restore, StepStatus::Failed))
    );
    let open = journal(&fakes);
    assert_eq!(open.changes.len(), 1);
    assert_eq!(open.changes[0].status, ChangeStatus::Reverting);
}

/// Full-screen scaling read back as something else (INJ-D12): the mode stays, and the
/// outcome carries SCALING_NOT_APPLIED as a warning.
#[test]
fn req_nv_25_inj_d12_scaling_not_applied_is_a_warning() {
    let (core, fakes) = fake_core(FixtureProfile::TestPc);
    fakes.display.script(
        DisplayFault::ApplyIgnoresScaling(Scaling::AspectRatio),
        When::Once,
    );
    let outcome = apply(&core, request(&lg_id(), stretched())).unwrap();
    let ApplyOutcome::PendingConfirm {
        warnings, steps, ..
    } = outcome
    else {
        panic!("not pending");
    };
    assert_eq!(warnings.len(), 1);
    assert_eq!(warnings[0].code, ErrorCode::ScalingNotApplied);
    assert_eq!(
        steps.last().map(|report| (report.step, report.status)),
        Some((StepKind::Scaling, StepStatus::Failed))
    );
    assert_eq!(current(&fakes, &lg_id()), Some(stretched()));
}

/// The apply call succeeds but the read after it fails: INTERNAL naming the failed read,
/// never MODE_REJECTED with a differing mode, and the previous state goes back under the same
/// entry, which closes. When the put-back fails too: RECOVERY_INCOMPLETE whose detail still
/// names the failed read, and the entry stays open.
// Also REQ-SAFE-17, REQ-DISP-13.
#[test]
fn req_ui_09_failed_read_back_is_not_a_rejected_mode() {
    let (core, fakes, recorder, window) = watched_core(FixtureProfile::TestPc);
    // The first read is step 3; the second is the read after the apply call.
    fakes
        .display
        .script(DisplayFault::EnumerateFails, When::Nth(2));
    let error = apply(&core, request(&lg_id(), stretched())).unwrap_err();
    assert_eq!(error.code(), ErrorCode::Internal);
    let detail = error.to_ipc().detail;
    assert!(detail.contains("the read after it failed"), "{detail}");
    assert!(
        detail.contains("read back as 1920 x 1080 at 240 Hz"),
        "{detail}"
    );
    assert!(!detail.contains("differs"), "{detail}");
    assert_eq!(current(&fakes, &lg_id()), Some(native()));
    assert_eq!(apply_calls(&fakes), 2);
    assert!(journal(&fakes).changes.is_empty());
    assert!(recorder.payloads(CoreEvent::ModePendingConfirm).is_empty());
    assert!(window.calls().is_empty());
    let state = core.snapshot();
    assert!(state.pending.is_none() && state.active.is_empty() && state.busy.is_none());

    let (core, fakes) = fake_core(FixtureProfile::TestPc);
    fakes
        .display
        .script(DisplayFault::EnumerateFails, When::Nth(2));
    fakes
        .display
        .script(DisplayFault::ApplyFails(-1), When::Nth(2));
    let error = apply(&core, request(&lg_id(), stretched())).unwrap_err();
    assert_eq!(error.code(), ErrorCode::RecoveryIncomplete);
    let detail = error.to_ipc().detail;
    assert!(detail.contains("the read after it failed"), "{detail}");
    assert_eq!(
        steps(&error).last(),
        Some(&(StepKind::Restore, StepStatus::Failed))
    );
    assert_eq!(journal(&fakes).changes.len(), 1);
}

/// A revert whose put-back reads the previous mode but another scaling (the driver kept its
/// own, as in INJ-D12): the entry stays open, so the baseline is not closed with the scaling
/// changed; the countdown stops, the change shows as an active change with the scaling read
/// back, no mode-reverted is sent, and the error is RECOVERY_INCOMPLETE. Restore everything
/// then puts the baseline back from the saved snapshot.
// Also REQ-NV-25, REQ-DISP-22.
#[test]
fn req_safe_17_revert_put_back_verifies_the_scaling() {
    let (core, fakes, recorder, window) = watched_core(FixtureProfile::TestPc);
    kept_stretched(&core);
    let id = pending_id(&apply(&core, request(&lg_id(), raw(1024, 768, 240))).unwrap());
    fakes.display.script(
        DisplayFault::ApplyIgnoresScaling(Scaling::AspectRatio),
        When::Once,
    );
    let error = block_on(core.revert(change(&id), RevertReason::Player)).unwrap_err();
    assert_eq!(error.code(), ErrorCode::RecoveryIncomplete);
    assert_eq!(
        steps(&error),
        [
            (StepKind::Restore, StepStatus::Done),
            (StepKind::Verify, StepStatus::Failed)
        ]
    );
    let ipc = error.to_ipc();
    assert!(ipc.detail.contains("AspectRatio"), "{}", ipc.detail);
    assert_eq!(ipc.context.current.unwrap().label, "1280 x 960 at 240 Hz");
    assert_eq!(ipc.context.still_changed.len(), 2);
    assert_eq!(current(&fakes, &lg_id()), Some(stretched()));
    assert_eq!(
        fakes.display.monitor(&lg_id()).unwrap().scaling,
        Scaling::AspectRatio
    );
    let open = journal(&fakes);
    assert_eq!(open.changes.len(), 2);
    assert_eq!(open.changes[1].change_id, id);
    assert_eq!(open.changes[1].status, ChangeStatus::Reverting);
    assert_eq!(open.monitors.len(), 1);
    let state = core.snapshot();
    assert!(state.pending.is_none() && state.busy.is_none());
    assert_eq!(state.active.len(), 1);
    assert_eq!(state.active[0].change_id, id);
    assert_eq!(state.active[0].now.label, "1280 x 960 at 240 Hz");
    assert_eq!(state.active[0].before.label, "1920 x 1080 at 240 Hz");
    assert_eq!(state.active[0].scaling, Scaling::AspectRatio);
    assert!(recorder.payloads(CoreEvent::ModeReverted).is_empty());
    assert_eq!(
        window.calls(),
        [
            KeepModeCall::Open,
            KeepModeCall::Close,
            KeepModeCall::Open,
            KeepModeCall::Close
        ]
    );
    // The countdown stopped with the put-back.
    let applies = apply_calls(&fakes);
    fakes.clock.advance(20_000);
    block_on(core.run_due_timers());
    assert_eq!(apply_calls(&fakes), applies);

    let outcome = block_on(core.restore_everything(ChangeSource::Window)).unwrap();
    assert_eq!(outcome.restored, [lg_id()]);
    assert_eq!(current(&fakes, &lg_id()), Some(native()));
    assert_eq!(
        fakes.display.monitor(&lg_id()).unwrap().scaling,
        Scaling::Identity
    );
    let open = journal(&fakes);
    assert!(open.changes.is_empty() && open.monitors.is_empty());
    assert!(core.snapshot().active.is_empty());
}

/// A rejected change whose put-back reads the previous mode but another scaling: the entry
/// stays open and the error is RECOVERY_INCOMPLETE naming both, never MODE_REJECTED with the
/// change reported as put back.
// Also REQ-NV-25, REQ-DISP-13.
#[test]
fn req_safe_17_rejected_change_put_back_verifies_the_scaling() {
    let (core, fakes) = fake_core(FixtureProfile::TestPc);
    kept_stretched(&core);
    fakes.display.script(
        DisplayFault::ApplySetsOtherRate(Rational::hz(60)),
        When::Once,
    );
    fakes.display.script(
        DisplayFault::ApplyIgnoresScaling(Scaling::AspectRatio),
        When::Nth(2),
    );
    let error = apply(&core, request(&lg_id(), raw(1024, 768, 240))).unwrap_err();
    assert_eq!(error.code(), ErrorCode::RecoveryIncomplete);
    let detail = error.to_ipc().detail;
    assert!(detail.contains("differs"), "{detail}");
    assert!(detail.contains("AspectRatio"), "{detail}");
    assert_eq!(
        steps(&error).last(),
        Some(&(StepKind::Restore, StepStatus::Failed))
    );
    assert_eq!(current(&fakes, &lg_id()), Some(stretched()));
    let open = journal(&fakes);
    assert_eq!(open.changes.len(), 2);
    assert_eq!(open.changes[1].status, ChangeStatus::Reverting);
    let state = core.snapshot();
    assert!(state.pending.is_none() && state.busy.is_none());
    assert_eq!(state.active.len(), 1);
    assert_eq!(state.active[0].now.label, "1280 x 960 at 240 Hz");
    assert_eq!(state.active[0].scaling, Scaling::AspectRatio);
}

/// A size the monitor does not list (INJ-D04) and a listed size at a rate it does not offer
/// (INJ-D05): nothing changes, with the core's labels and the offered rates.
// Also REQ-DISP-18.
#[test]
fn req_disp_11_inj_d04_d05_unlisted_sizes_and_rates_change_nothing() {
    let (core, fakes) = fake_core(FixtureProfile::TestPc);
    let error = apply(&core, typed(&lg_id(), raw(1440, 1080, 240))).unwrap_err();
    assert_eq!(error.code(), ErrorCode::ModeNotOffered);
    let context = error.to_ipc().context;
    assert_eq!(context.monitor.as_deref(), Some("LG ULTRAGEAR"));
    assert_eq!(context.mode.unwrap().label, "1440 x 1080 at 240 Hz");

    let error = apply(&core, request(&lg_id(), raw(1920, 1080, 165))).unwrap_err();
    assert_eq!(error.code(), ErrorCode::RefreshNotAvailable);
    let offered: Vec<String> = error
        .to_ipc()
        .context
        .offered_refresh
        .into_iter()
        .map(|rate| rate.label)
        .collect();
    assert_eq!(
        offered,
        [
            "240 Hz", "144 Hz", "120 Hz", "100 Hz", "75 Hz", "60 Hz", "48 Hz"
        ]
    );
    assert!(fakes.store.file(JOURNAL_FILE).is_none());
    assert_eq!(apply_calls(&fakes), 0);
}

/// A fractional rate is kept exactly through the request, the apply, the read-back and the
/// journal (REQ-DISP-17).
#[test]
fn req_disp_17_fractional_rates_stay_exact() {
    let (core, fakes) = fake_core(FixtureProfile::TestPc);
    let ntsc = Rational {
        numerator: 60_000,
        denominator: 1_001,
    };
    let at = |width, height| RawMode {
        width,
        height,
        refresh: ntsc,
    };
    fakes.display.edit_monitor(&lg_id(), |monitor| {
        monitor.modes.push(at(1920, 1080));
        monitor.modes.push(at(1280, 960));
    });
    fakes.display.change_from_outside(
        &lg_id(),
        at(1920, 1080),
        Scaling::Identity,
        OutsideChange::Temporary,
    );
    let outcome = apply(&core, typed(&lg_id(), at(1280, 960))).unwrap();
    let ApplyOutcome::PendingConfirm { pending, .. } = outcome else {
        panic!("not pending");
    };
    assert_eq!(pending.to.label, "1280 x 960 at 59.94 Hz");
    assert_eq!(
        (pending.to.refresh.numerator, pending.to.refresh.denominator),
        (60_000, 1_001)
    );
    assert_eq!(current(&fakes, &lg_id()), Some(at(1280, 960)));
    let open = journal(&fakes);
    assert_eq!(open.changes[0].previous.refresh, ntsc);
    assert_eq!(open.monitors[0].baseline.refresh, ntsc);
}

/// A monitor on an adapter that is not NVIDIA gets the whole display core.
#[test]
fn req_disp_02_other_adapters_get_the_display_core() {
    let (core, fakes) = fake_core(FixtureProfile::NotNvidia);
    let monitor = lg(FixtureProfile::NotNvidia);
    let id = pending_id(&apply(&core, request(&monitor, raw(1440, 1080, 60))).unwrap());
    assert_eq!(current(&fakes, &monitor), Some(raw(1440, 1080, 60)));
    block_on(core.revert(change(&id), RevertReason::Player)).unwrap();
    assert_eq!(current(&fakes, &monitor), Some(raw(1920, 1080, 60)));
}

/// The show-hidden toggle: list_monitors marks virtual displays, save_settings stores the
/// toggle, and a hidden monitor changes only while it is on (REQ-DISP-08, REQ-SET-01).
// Also REQ-SET-01, REQ-DISP-07.
#[test]
fn req_disp_08_hidden_monitors_change_only_with_the_toggle_on() {
    let (core, fakes, recorder, _window) = watched_core(FixtureProfile::TestPc);
    let monitors = block_on(core.list_monitors()).unwrap();
    let parsec = monitors[1].id.clone();
    assert!(monitors[1].hidden);
    let error = apply(&core, request(&parsec, raw(1920, 1080, 60))).unwrap_err();
    assert_eq!(error.code(), ErrorCode::Internal);

    let mut input = settings_input(&core);
    input.show_hidden_monitors = true;
    let revision = core.snapshot().revision;
    let saved = block_on(core.save_settings(input)).unwrap();
    assert!(saved.show_hidden_monitors);
    assert!(core.snapshot().revision > revision);
    assert!(!recorder.payloads(CoreEvent::StateChanged).is_empty());
    let file: Value = serde_json::from_slice(&fakes.store.file("settings.json").unwrap()).unwrap();
    assert_eq!(file["settings"]["showHiddenMonitors"], true);
    // The mode it already shows: applied, back at its baseline, nothing left open.
    let outcome = apply(&core, request(&parsec, raw(1920, 1080, 60))).unwrap();
    assert!(matches!(outcome, ApplyOutcome::Applied { .. }));
    assert!(journal(&fakes).changes.is_empty());
}

/// save_settings checks every field and leaves memory and the file as they were when the
/// write fails (REQ-SET-06).
#[test]
fn req_set_01_save_settings_checks_and_keeps_memory_on_failure() {
    let (core, fakes) = fake_core(FixtureProfile::TestPc);
    let mut zero = settings_input(&core);
    zero.revert_countdown_secs = 0;
    let error = block_on(core.save_settings(zero)).unwrap_err();
    assert_eq!(error.code(), ErrorCode::Internal);
    let mut unknown = settings_input(&core);
    unknown.play_stretched_profile = Some(String::from(ID));
    assert_eq!(
        block_on(core.save_settings(unknown)).unwrap_err().code(),
        ErrorCode::Internal
    );
    assert!(fakes.store.file("settings.json").is_none());

    fakes.store.script(
        StoreFault::Write(StoreFile::Settings, WriteStage::Rename),
        When::Once,
    );
    let mut input = settings_input(&core);
    input.check_for_updates = false;
    let error = block_on(core.save_settings(input)).unwrap_err();
    assert_eq!(error.code(), ErrorCode::SettingsWriteFailed);
    assert!(core.snapshot().settings.check_for_updates);
}

/// Requests are checked in Rust before anything is read (REQ-SEC-08): a zero size, a zero
/// denominator, a malformed monitor or change ID is INTERNAL, an unknown profile is
/// PROFILE_INVALID, and no display call is made.
#[test]
fn req_sec_08_requests_are_checked_before_anything_runs() {
    let (core, fakes) = fake_core(FixtureProfile::TestPc);
    let mut zero_width = to_mode(stretched());
    zero_width.width = 0;
    let mut zero_rate = to_mode(stretched());
    zero_rate.refresh.denominator = 0;
    for mode in [zero_width, zero_rate] {
        let error = apply(
            &core,
            ApplyRequest::Mode {
                monitor: lg_id(),
                mode,
                scaling: ScalingChoice::Keep,
            },
        )
        .unwrap_err();
        assert_eq!(error.code(), ErrorCode::Internal);
    }
    // No device path and no EDID ids: nothing names a monitor (REQ-DISP-26).
    let mut nameless = lg_id();
    nameless.device_path = String::new();
    nameless.edid_manufacturer_id = None;
    nameless.edid_product_code = None;
    assert_eq!(
        apply(&core, request(&nameless, stretched()))
            .unwrap_err()
            .code(),
        ErrorCode::Internal
    );
    let profile = apply(
        &core,
        ApplyRequest::Profile {
            profile_id: String::from(ID),
        },
    )
    .unwrap_err();
    assert_eq!(profile.code(), ErrorCode::ProfileInvalid);
    let malformed = block_on(core.confirm(change("not a change id"), ChangeSource::Window));
    assert_eq!(malformed.unwrap_err().code(), ErrorCode::Internal);
    assert!(fakes.display.calls().is_empty());
}

/// INTERNAL only where no other code fits (INJ-D01): a failed enumeration writes nothing.
#[test]
fn req_ui_09_inj_d01_failed_read_is_internal_and_writes_nothing() {
    let (core, fakes) = fake_core(FixtureProfile::TestPc);
    fakes
        .display
        .script(DisplayFault::EnumerateFails, When::Once);
    let error = apply(&core, request(&lg_id(), stretched())).unwrap_err();
    assert_eq!(error.code(), ErrorCode::Internal);
    assert!(fakes.store.file(JOURNAL_FILE).is_none());
    assert_eq!(apply_calls(&fakes), 0);
}

/// The monitor is missing at step 3 or unavailable at the test (INJ-D02, INJ-D03):
/// MONITOR_GONE with its name, and nothing written.
// Also REQ-SAFE-09.
#[test]
fn req_disp_02_inj_d02_d03_missing_monitor_is_monitor_gone() {
    let (core, fakes) = fake_core(FixtureProfile::TestPc);
    core.refresh_monitors().unwrap();
    fakes
        .display
        .script(DisplayFault::UnplugBeforeTest, When::Once);
    let error = apply(&core, request(&lg_id(), stretched())).unwrap_err();
    assert_eq!(error.code(), ErrorCode::MonitorGone);
    assert_eq!(
        error.to_ipc().context.monitor.as_deref(),
        Some("LG ULTRAGEAR")
    );
    let error = apply(&core, request(&lg_id(), stretched())).unwrap_err();
    assert_eq!(error.code(), ErrorCode::MonitorGone);
    assert_eq!(
        error.to_ipc().context.monitor.as_deref(),
        Some("LG ULTRAGEAR")
    );
    assert!(fakes.store.file(JOURNAL_FILE).is_none());
}

/// The journal holds the previous state before the change: the baseline with its rational
/// and scaling, the adapter device path, and the CCD snapshot of every monitor (REQ-SAFE-02).
#[test]
fn req_safe_02_journal_holds_the_previous_state_and_the_snapshot() {
    let (core, fakes) = fake_core(FixtureProfile::TestPc);
    pending_id(&apply(&core, request(&lg_id(), stretched())).unwrap());
    let open = journal(&fakes);
    assert_eq!(open.monitors.len(), 1);
    assert_eq!(open.monitors[0].identity, lg_id());
    assert_eq!(open.monitors[0].baseline.raw_mode(), native());
    assert_eq!(open.monitors[0].baseline.scaling, Scaling::Identity);
    assert_eq!(
        open.monitors[0].adapter_device_path,
        "fake-adapter-path:test-pc:rtx-4060-ti"
    );
    assert_eq!(open.changes[0].previous.raw_mode(), native());
    assert_eq!(open.changes[0].target.raw_mode(), stretched());
    assert_eq!(
        open.changes[0].route_choice(),
        RouteChoice::gdi(GdiFlag::Fullscreen)
    );
    let layout = open.layout.unwrap();
    assert_eq!(layout.targets.len(), 2);
    assert_eq!(layout.targets[0], lg_id());
    assert_eq!(layout.paths.len(), 2);
    assert_eq!(layout.modes.len(), 4);
    assert!(
        layout
            .adapters
            .iter()
            .all(|adapter| !adapter.adapter_device_path.is_empty())
    );
    assert!(!layout.virtual_mode_aware);
}

/// Restore native returns a changed monitor to its native size at the rate it had before
/// ResSwap's first change, with no countdown, and closes its entries (REQ-DISP-21).
#[test]
fn req_disp_21_restore_native_keeps_the_baseline_rate() {
    let (core, fakes, recorder, window) = watched_core(FixtureProfile::TestPc);
    fakes.display.edit_monitor(&lg_id(), |monitor| {
        monitor.modes.push(raw(1280, 960, 144));
    });
    fakes.display.change_from_outside(
        &lg_id(),
        raw(1920, 1080, 144),
        Scaling::Identity,
        OutsideChange::Temporary,
    );
    let id = pending_id(&apply(&core, request(&lg_id(), raw(1280, 960, 144))).unwrap());
    keep(&core, &id);
    recorder.clear();
    let outcome = block_on(core.restore_native(
        RestoreNativeRequest {
            monitor: Some(lg_id()),
        },
        ChangeSource::Window,
    ))
    .unwrap();
    assert_eq!(outcome.restored, [lg_id()]);
    assert!(!outcome.steps.is_empty());
    assert_eq!(current(&fakes, &lg_id()), Some(raw(1920, 1080, 144)));
    assert!(recorder.payloads(CoreEvent::ModePendingConfirm).is_empty());
    assert_eq!(window.calls(), [KeepModeCall::Open, KeepModeCall::Close]);
    assert!(journal(&fakes).changes.is_empty());
    assert!(core.snapshot().active.is_empty());
    // Nothing changed now: nothing to restore.
    let nothing =
        block_on(core.restore_native(RestoreNativeRequest { monitor: None }, ChangeSource::Window))
            .unwrap();
    assert!(nothing.restored.is_empty() && nothing.steps.is_empty());
}

/// Restore native with no monitor named restores every monitor ResSwap changed, and puts a
/// pending change back first.
#[test]
fn req_disp_21_restore_native_for_every_changed_monitor() {
    let (core, fakes, recorder, _window) = watched_core(FixtureProfile::TwoMonitors);
    let lg = lg(FixtureProfile::TwoMonitors);
    let samsung = known_monitors(FixtureProfile::TwoMonitors).remove(2).id;
    let first = pending_id(&apply(&core, request(&lg, stretched())).unwrap());
    keep(&core, &first);
    pending_id(&apply(&core, request(&samsung, raw(1280, 1024, 60))).unwrap());
    let outcome =
        block_on(core.restore_native(RestoreNativeRequest { monitor: None }, ChangeSource::Window))
            .unwrap();
    assert_eq!(
        recorder.payloads(CoreEvent::ModeReverted)[0]["reason"],
        "superseded"
    );
    assert!(outcome.restored.contains(&lg));
    assert_eq!(current(&fakes, &lg), Some(native()));
    assert_eq!(current(&fakes, &samsung), Some(raw(1920, 1080, 60)));
    assert!(journal(&fakes).changes.is_empty());
}

/// Restore everything puts the baseline back from the saved snapshot (REQ-DISP-22).
#[test]
fn req_disp_22_restore_everything_uses_the_snapshot() {
    let (core, fakes) = fake_core(FixtureProfile::TestPc);
    kept_stretched(&core);
    let outcome = block_on(core.restore_everything(ChangeSource::Window)).unwrap();
    assert_eq!(outcome.restored, [lg_id()]);
    assert!(
        fakes
            .display
            .calls()
            .iter()
            .any(|call| matches!(call, DisplayCall::RestoreSnapshot { .. }))
    );
    assert_eq!(current(&fakes, &lg_id()), Some(native()));
    assert!(journal(&fakes).changes.is_empty() && journal(&fakes).layout.is_none());
    assert!(core.snapshot().active.is_empty());
    // Nothing open now.
    let nothing = block_on(core.restore_everything(ChangeSource::Window)).unwrap();
    assert!(nothing.restored.is_empty() && nothing.steps.is_empty());
}

/// Restore everything puts a pending change back first, with reason Superseded.
#[test]
fn req_disp_22_restore_everything_puts_back_a_pending_change_first() {
    let (core, fakes, recorder, _window) = watched_core(FixtureProfile::TestPc);
    pending_id(&apply(&core, request(&lg_id(), stretched())).unwrap());
    let outcome = block_on(core.restore_everything(ChangeSource::Window)).unwrap();
    assert_eq!(
        recorder.payloads(CoreEvent::ModeReverted)[0]["reason"],
        "superseded"
    );
    assert!(outcome.restored.is_empty() && outcome.steps.is_empty());
    assert_eq!(current(&fakes, &lg_id()), Some(native()));
}

/// The snapshot restore fails and the database fallback works; both fail (INJ-D20).
#[test]
fn req_disp_22_inj_d20_database_fallback() {
    let (core, fakes) = fake_core(FixtureProfile::TestPc);
    kept_stretched(&core);
    fakes
        .display
        .script(DisplayFault::RestoreSnapshotFails(31), When::Once);
    let outcome = block_on(core.restore_everything(ChangeSource::Window)).unwrap();
    assert_eq!(outcome.restored, [lg_id()]);
    assert!(
        fakes
            .display
            .calls()
            .iter()
            .any(|call| matches!(call, DisplayCall::ApplyDatabaseCurrent { .. }))
    );
    assert_eq!(current(&fakes, &lg_id()), Some(native()));

    kept_stretched(&core);
    fakes
        .display
        .script(DisplayFault::RestoreSnapshotFails(31), When::Once);
    fakes
        .display
        .script(DisplayFault::DatabaseCurrentFails(31), When::Once);
    let error = block_on(core.restore_everything(ChangeSource::Window)).unwrap_err();
    assert_eq!(error.code(), ErrorCode::RecoveryIncomplete);
    assert_eq!(error.to_ipc().context.still_changed.len(), 1);
    let open = journal(&fakes);
    assert_eq!(open.changes.len(), 1);
    assert_eq!(open.changes[0].status, ChangeStatus::Restoring);
}

/// The adapter LUIDs changed between the snapshot and the restore (INJ-D21): the restore
/// maps them by device path and succeeds.
#[test]
fn req_disp_22_inj_d21_luids_are_mapped_again() {
    let (core, fakes) = fake_core(FixtureProfile::TestPc);
    kept_stretched(&core);
    fakes.display.change_adapter_luids();
    block_on(core.restore_everything(ChangeSource::Window)).unwrap();
    let remapped = fakes
        .display
        .calls()
        .into_iter()
        .find_map(|call| match call {
            DisplayCall::RestoreSnapshot { remapped, .. } => Some(remapped),
            _ => None,
        });
    assert_eq!(remapped, Some(2));
    assert_eq!(current(&fakes, &lg_id()), Some(native()));
}

/// Every changing call is temporary (INJ-D24): the logged routes are only the temporary
/// forms, and with CDS_FULLSCREEN no flag-0 call comes while a change is open (REQ-DISP-15).
// Also REQ-DISP-15.
#[test]
fn req_disp_14_inj_d24_flag_log_shows_only_temporary_changes() {
    for route in [
        RouteChoice::gdi(GdiFlag::Fullscreen),
        RouteChoice::gdi(GdiFlag::Zero),
        RouteChoice::ccd(),
    ] {
        let (core, fakes) = fake_core(FixtureProfile::TestPc);
        fakes.display.set_route(route);
        let id = pending_id(&apply(&core, request(&lg_id(), stretched())).unwrap());
        block_on(core.revert(change(&id), RevertReason::Player)).unwrap();
        let id = pending_id(&apply(&core, request(&lg_id(), stretched())).unwrap());
        keep(&core, &id);
        pending_id(&apply(&core, request(&lg_id(), raw(1024, 768, 240))).unwrap());
        pending_id(&apply(&core, request(&lg_id(), raw(800, 600, 240))).unwrap());
        block_on(core.restore_native(RestoreNativeRequest { monitor: None }, ChangeSource::Window))
            .unwrap();
        kept_stretched(&core);
        block_on(core.restore_everything(ChangeSource::Window)).unwrap();
        for call in fakes.display.calls() {
            if let DisplayCall::Apply { route: used, .. } = call {
                assert_eq!(used, route);
                assert!(used.is_consistent());
            }
        }
        assert!(
            fakes.display.flag_zero_while_fullscreen_open().is_empty(),
            "{route:?}"
        );
        assert_eq!(current(&fakes, &lg_id()), Some(native()));
    }
}

/// WM_DISPLAYCHANGE with another mode while pending (INJ-D22): the countdown stops, nothing
/// is re-applied, and the change stays open as an active change, with status applied; when
/// the screen shows the state before it, the entry closes. No mode-reverted, because ResSwap
/// put nothing back.
#[test]
fn req_disp_23_inj_d22_outside_change_while_pending() {
    let (core, fakes, recorder, window) = watched_core(FixtureProfile::TestPc);
    let id = pending_id(&apply(&core, request(&lg_id(), stretched())).unwrap());
    fakes.display.change_from_outside(
        &lg_id(),
        raw(1024, 768, 60),
        Scaling::Stretched,
        OutsideChange::Temporary,
    );
    block_on(core.on_system_event(SystemEvent::DisplayChanged));
    let state = core.snapshot();
    assert!(state.pending.is_none());
    assert_eq!(state.active.len(), 1);
    assert_eq!(state.active[0].change_id, id);
    assert_eq!(state.active[0].now.label, "1024 x 768 at 60 Hz");
    assert_eq!(state.active[0].before.label, "1920 x 1080 at 240 Hz");
    assert_eq!(journal(&fakes).changes[0].status, ChangeStatus::Applied);
    assert!(recorder.payloads(CoreEvent::ModeReverted).is_empty());
    assert_eq!(window.calls(), [KeepModeCall::Open, KeepModeCall::Close]);
    assert_eq!(apply_calls(&fakes), 1);
    // The countdown no longer runs.
    fakes.clock.advance(20_000);
    block_on(core.run_due_timers());
    assert_eq!(apply_calls(&fakes), 1);

    // A pending change replaced by the state before it closes.
    let (core, fakes, recorder, _window) = watched_core(FixtureProfile::TestPc);
    pending_id(&apply(&core, request(&lg_id(), stretched())).unwrap());
    fakes.display.change_from_outside(
        &lg_id(),
        native(),
        Scaling::Identity,
        OutsideChange::Temporary,
    );
    block_on(core.on_system_event(SystemEvent::DisplayChanged));
    assert!(core.snapshot().pending.is_none() && core.snapshot().active.is_empty());
    assert!(journal(&fakes).changes.is_empty());
    assert!(recorder.payloads(CoreEvent::ModeReverted).is_empty());
}

/// WM_DISPLAYCHANGE with another mode while active: the new mode is recorded, never fought,
/// and state-changed is sent (REQ-DISP-23).
#[test]
fn req_disp_23_outside_change_while_active_is_recorded() {
    let (core, fakes, recorder, _window) = watched_core(FixtureProfile::TestPc);
    kept_stretched(&core);
    let applies = apply_calls(&fakes);
    recorder.clear();
    fakes.display.change_from_outside(
        &lg_id(),
        raw(1024, 768, 60),
        Scaling::Stretched,
        OutsideChange::Temporary,
    );
    block_on(core.on_system_event(SystemEvent::DisplayChanged));
    let state = core.snapshot();
    assert_eq!(state.active[0].now.label, "1024 x 768 at 60 Hz");
    assert_eq!(
        state.monitors[0]
            .current
            .as_ref()
            .map(|mode| mode.label.as_str()),
        Some("1024 x 768 at 60 Hz")
    );
    assert!(!recorder.payloads(CoreEvent::StateChanged).is_empty());
    assert_eq!(apply_calls(&fakes), applies);
}

/// A journaled monitor goes away and comes back (INJ-D22, REQ-SAFE-09): MONITOR_GONE once,
/// its entry kept, then its baseline restored when it returns.
#[test]
fn req_safe_09_inj_d22_monitor_gone_and_back() {
    let (core, fakes, _recorder, _window) = watched_core(FixtureProfile::TestPc);
    kept_stretched(&core);
    assert!(fakes.display.unplug(&lg_id()));
    block_on(core.on_system_event(SystemEvent::DisplayChanged));
    block_on(core.on_system_event(SystemEvent::DisplayChanged));
    let errors = core.snapshot().errors;
    assert_eq!(errors.len(), 1);
    assert_eq!(errors[0].error.code, ErrorCode::MonitorGone);
    assert_eq!(errors[0].count, 1);
    assert_eq!(journal(&fakes).changes.len(), 1);

    assert!(fakes.display.plug(&lg_id()));
    assert_eq!(current(&fakes, &lg_id()), Some(stretched()));
    block_on(core.on_system_event(SystemEvent::DisplayChanged));
    assert_eq!(current(&fakes, &lg_id()), Some(native()));
    assert!(journal(&fakes).changes.is_empty());
    assert!(core.snapshot().active.is_empty());
}

/// Suspend only flushes and holds the countdown's revert; the resume reverts a pending
/// change at once, reason Timeout (INJ-D22, REQ-SAFE-08).
#[test]
fn req_safe_08_inj_d22_resume_reverts_a_pending_change() {
    let (core, fakes, recorder, _window) = watched_core(FixtureProfile::TestPc);
    pending_id(&apply(&core, request(&lg_id(), stretched())).unwrap());
    let writes = fakes.store.calls().len();
    block_on(core.on_system_event(SystemEvent::Suspending));
    assert_eq!(fakes.store.calls().len(), writes);
    fakes.clock.advance(10_000);
    block_on(core.run_due_timers());
    assert!(core.snapshot().pending.is_some());
    block_on(core.on_system_event(SystemEvent::Resumed));
    let reverted = recorder.payloads(CoreEvent::ModeReverted);
    assert_eq!(reverted.len(), 1);
    assert_eq!(reverted[0]["reason"], "timeout");
    assert_eq!(current(&fakes, &lg_id()), Some(native()));
}

/// A new core on the store an earlier core left: the crash cases of INJ-D18.
fn crashed(fakes: &Fakes) -> Core {
    let core = Core::new(fakes.backends());
    core.set_keep_mode_window(std::sync::Arc::new(
        resswap_lib::shell::fake::FakeKeepModeWindow::new(),
    ));
    block_on(core.recover_at_start());
    core
}

/// Start-up recovery at each status a crash can leave (INJ-D18): an intent whose apply never
/// ran closes; an applied, pending or kept change is restored with no countdown; a change
/// made outside ResSwap closes; a monitor that is not connected keeps its entry and leaves
/// recovery incomplete (REQ-SAFE-06).
#[test]
fn req_safe_06_inj_d18_recovery_after_a_crash() {
    // Intent written, the apply never ran: Windows shows the baseline.
    let fakes = Fakes::new(FixtureProfile::TestPc);
    fakes.store.put(JOURNAL_FILE, OPEN_CHANGE.as_bytes());
    let core = crashed(&fakes);
    assert_eq!(core.snapshot().recovery.status, RecoveryStatus::Clean);
    assert!(journal(&fakes).changes.is_empty());
    assert_eq!(apply_calls(&fakes), 0);

    // Intent written and the apply ran: the target is on screen, so the baseline goes back.
    let fakes = Fakes::new(FixtureProfile::TestPc);
    fakes.store.put(JOURNAL_FILE, OPEN_CHANGE.as_bytes());
    fakes.display.change_from_outside(
        &lg_id(),
        stretched(),
        Scaling::Stretched,
        OutsideChange::Temporary,
    );
    let core = crashed(&fakes);
    assert_eq!(core.snapshot().recovery.status, RecoveryStatus::Recovered);
    assert_eq!(current(&fakes, &lg_id()), Some(native()));
    assert!(journal(&fakes).changes.is_empty());

    // Pending and kept: restored from the snapshot, never through a countdown.
    for keep_it in [false, true] {
        let fakes = Fakes::new(FixtureProfile::TestPc);
        let earlier = Core::new(fakes.backends());
        let id = pending_id(&apply(&earlier, request(&lg_id(), stretched())).unwrap());
        if keep_it {
            keep(&earlier, &id);
        }
        drop(earlier);
        let recorder = std::sync::Arc::new(Recorder::default());
        let core = Core::new(fakes.backends());
        core.set_event_sink(recorder.clone());
        block_on(core.recover_at_start());
        assert_eq!(core.snapshot().recovery.status, RecoveryStatus::Recovered);
        assert_eq!(current(&fakes, &lg_id()), Some(native()));
        assert!(
            fakes
                .display
                .calls()
                .iter()
                .any(|call| matches!(call, DisplayCall::RestoreSnapshot { .. }))
        );
        assert!(recorder.payloads(CoreEvent::ModePendingConfirm).is_empty());
        assert!(core.snapshot().pending.is_none());
        assert!(journal(&fakes).changes.is_empty());
    }

    // Reverting, with the monitor already back on its baseline: closed.
    let fakes = Fakes::new(FixtureProfile::TestPc);
    let earlier = Core::new(fakes.backends());
    fakes
        .display
        .script(DisplayFault::ApplyFails(-1), When::Always);
    apply(&earlier, request(&lg_id(), stretched())).unwrap_err();
    assert_eq!(journal(&fakes).changes[0].status, ChangeStatus::Reverting);
    drop(earlier);
    let core = crashed(&fakes);
    assert_eq!(core.snapshot().recovery.status, RecoveryStatus::Clean);
    assert!(journal(&fakes).changes.is_empty());

    // Changed outside ResSwap: never fought, the entry closes.
    let fakes = Fakes::new(FixtureProfile::TestPc);
    fakes.store.put(JOURNAL_FILE, OPEN_CHANGE.as_bytes());
    fakes.display.change_from_outside(
        &lg_id(),
        raw(1024, 768, 60),
        Scaling::Stretched,
        OutsideChange::Temporary,
    );
    let core = crashed(&fakes);
    assert_eq!(current(&fakes, &lg_id()), Some(raw(1024, 768, 60)));
    assert!(journal(&fakes).changes.is_empty());
    assert_eq!(core.snapshot().recovery.status, RecoveryStatus::Clean);

    // Not connected: kept, incomplete, raised.
    let fakes = Fakes::new(FixtureProfile::TestPc);
    fakes.store.put(JOURNAL_FILE, OPEN_CHANGE.as_bytes());
    fakes.display.unplug(&lg_id());
    let core = crashed(&fakes);
    let state = core.snapshot();
    assert_eq!(state.recovery.status, RecoveryStatus::Incomplete);
    assert_eq!(state.recovery.still_changed.len(), 1);
    assert_eq!(state.errors[0].error.code, ErrorCode::RecoveryIncomplete);
    assert_eq!(journal(&fakes).changes.len(), 1);
}

/// Recovery that could not finish leaves Restore everything to do it; a verified Restore
/// everything sets the recovery state back to clean.
#[test]
fn req_safe_06_restore_everything_clears_an_incomplete_recovery() {
    let fakes = Fakes::new(FixtureProfile::TestPc);
    let earlier = Core::new(fakes.backends());
    kept_stretched(&earlier);
    drop(earlier);
    fakes
        .display
        .script(DisplayFault::RestoreSnapshotFails(31), When::Always);
    fakes
        .display
        .script(DisplayFault::DatabaseCurrentFails(31), When::Once);
    let core = crashed(&fakes);
    assert_eq!(core.snapshot().recovery.status, RecoveryStatus::Incomplete);
    assert_eq!(current(&fakes, &lg_id()), Some(stretched()));
    let outcome = block_on(core.restore_everything(ChangeSource::Window)).unwrap();
    assert_eq!(outcome.restored, [lg_id()]);
    let state = core.snapshot();
    assert_eq!(state.recovery.status, RecoveryStatus::Clean);
    assert!(state.recovery.still_changed.is_empty());
    assert_eq!(current(&fakes, &lg_id()), Some(native()));
}

/// A damaged journal (INJ-D19): kept aside, RECOVERY_INCOMPLETE raised at start, and Restore
/// everything puts back the configuration Windows keeps, after which recovery is clean.
#[test]
fn req_safe_06_inj_d19_damaged_journal_leaves_restore_everything() {
    let fakes = Fakes::new(FixtureProfile::TestPc);
    fakes.store.put(JOURNAL_FILE, b"{");
    let core = Core::new(fakes.backends());
    core.raise_load_errors();
    block_on(core.recover_at_start());
    let state = core.snapshot();
    assert_eq!(state.recovery.status, RecoveryStatus::Incomplete);
    assert_eq!(state.errors[0].error.code, ErrorCode::RecoveryIncomplete);
    assert!(
        fakes
            .store
            .names()
            .iter()
            .any(|name| name.starts_with("journal.damaged-"))
    );
    let outcome = block_on(core.restore_everything(ChangeSource::Window)).unwrap();
    assert_eq!(outcome.restored.len(), 2);
    assert!(
        fakes
            .display
            .calls()
            .iter()
            .any(|call| matches!(call, DisplayCall::ApplyDatabaseCurrent { .. }))
    );
    assert_eq!(core.snapshot().recovery.status, RecoveryStatus::Clean);
    assert!(journal(&fakes).changes.is_empty());
}

/// A debug build on the Win32 backend kind with an unfinished journal only reports it
/// (INJ-D23): no changing call, recovery incomplete, Restore everything offered.
#[test]
fn req_safe_07_inj_d23_debug_win32_build_reports_only() {
    if !cfg!(debug_assertions) {
        return;
    }
    let fakes = Fakes::new(FixtureProfile::TestPc);
    fakes.store.put(JOURNAL_FILE, OPEN_CHANGE.as_bytes());
    fakes.display.change_from_outside(
        &lg_id(),
        stretched(),
        Scaling::Stretched,
        OutsideChange::Temporary,
    );
    let mut backends = fakes.backends();
    backends.kind = BackendKind::Real;
    let core = Core::new(backends);
    block_on(core.recover_at_start());
    assert!(!fakes.display.applied_anything());
    let state = core.snapshot();
    assert_eq!(state.recovery.status, RecoveryStatus::Incomplete);
    assert_eq!(state.recovery.still_changed.len(), 1);
    assert_eq!(state.errors[0].error.code, ErrorCode::RecoveryIncomplete);
    assert_eq!(current(&fakes, &lg_id()), Some(stretched()));
}

/// open_log_folder opens the folder the core picked through the opener, from Rust; the
/// window never sends a path (REQ-SET-09).
#[test]
fn req_set_09_open_log_folder_uses_the_opener() {
    let (core, fakes) = fake_core(FixtureProfile::TestPc);
    assert_eq!(
        block_on(core.open_log_folder()).unwrap_err().code(),
        ErrorCode::Internal
    );
    let folder = PathBuf::from(r"C:\ResSwap-test-data\logs");
    core.set_log_folder(Some(folder.clone()), true);
    block_on(core.open_log_folder()).unwrap();
    assert_eq!(fakes.folder_opener.calls(), [folder]);
    fakes
        .folder_opener
        .script(FolderFault::OpenFails, When::Once);
    assert_eq!(
        block_on(core.open_log_folder()).unwrap_err().code(),
        ErrorCode::Internal
    );
    assert!(!fakes.changed_anything());
}

/// get_diagnostics reads and changes nothing, lists what the journal holds, and shows the
/// folders with environment variables only (REQ-DISP-25).
#[test]
fn req_disp_25_diagnostics_are_read_only() {
    let (core, fakes) = fake_core(FixtureProfile::TestPc);
    let empty = block_on(core.diagnostics()).unwrap();
    assert_eq!(empty.monitors.len(), 2);
    assert!(empty.journal_open_items.is_empty());
    assert_eq!(empty.build.backend, BackendKind::Fake);
    assert_eq!(
        empty.settings_folder,
        r"%LOCALAPPDATA%\io.github.T7SEN.ResSwap"
    );
    assert_eq!(
        empty.log_folder,
        r"%LOCALAPPDATA%\io.github.T7SEN.ResSwap\logs"
    );
    assert!(!fakes.display.applied_anything());
    pending_id(&apply(&core, request(&lg_id(), stretched())).unwrap());
    let applies = apply_calls(&fakes);
    let open = block_on(core.diagnostics()).unwrap();
    assert_eq!(open.journal_open_items.len(), 1);
    assert_eq!(
        open.journal_open_items[0]
            .mode
            .as_ref()
            .map(|mode: &Mode| mode.label.as_str()),
        Some("1280 x 960 at 240 Hz")
    );
    assert_eq!(apply_calls(&fakes), applies);
    core.set_log_folder(None, true);
    let overridden = block_on(core.diagnostics()).unwrap();
    assert_eq!(overridden.settings_folder, "%RESSWAP_DATA_DIR%");
}

/// A debug build on the Win32 backend kind closes an unfinished entry whose monitor already
/// shows its baseline, as when Windows put a CDS_FULLSCREEN mode back on exit, and reports
/// nothing: no changing call, recovery clean, no raised error (D-033, REQ-SAFE-07).
#[test]
fn req_safe_07_debug_build_closes_entries_windows_already_put_back() {
    if !cfg!(debug_assertions) {
        return;
    }
    let fakes = Fakes::new(FixtureProfile::TestPc);
    fakes.store.put(JOURNAL_FILE, OPEN_CHANGE.as_bytes());
    let mut backends = fakes.backends();
    backends.kind = BackendKind::Real;
    let core = Core::new(backends);
    block_on(core.recover_at_start());
    assert!(!fakes.display.applied_anything());
    let state = core.snapshot();
    assert_eq!(state.recovery.status, RecoveryStatus::Clean);
    assert!(state.errors.is_empty());
    assert!(journal(&fakes).changes.is_empty());
    assert_eq!(current(&fakes, &lg_id()), Some(native()));
}

/// Once Restore everything leaves nothing changed, the RECOVERY_INCOMPLETE raised at start
/// leaves the error list with the recovery state (D-034).
#[test]
fn req_disp_22_clean_restore_clears_the_raised_recovery_error() {
    if !cfg!(debug_assertions) {
        return;
    }
    let fakes = Fakes::new(FixtureProfile::TestPc);
    fakes.store.put(JOURNAL_FILE, OPEN_CHANGE.as_bytes());
    fakes.display.change_from_outside(
        &lg_id(),
        stretched(),
        Scaling::Stretched,
        OutsideChange::Temporary,
    );
    let mut backends = fakes.backends();
    backends.kind = BackendKind::Real;
    let core = Core::new(backends);
    block_on(core.recover_at_start());
    assert_eq!(
        core.snapshot().errors[0].error.code,
        ErrorCode::RecoveryIncomplete
    );
    block_on(core.restore_everything(ChangeSource::Window)).unwrap();
    let state = core.snapshot();
    assert_eq!(state.recovery.status, RecoveryStatus::Clean);
    assert!(
        state
            .errors
            .iter()
            .all(|raised| raised.error.code != ErrorCode::RecoveryIncomplete)
    );
    assert_eq!(current(&fakes, &lg_id()), Some(native()));
}

/// The apply call works, then the read after it no longer lists the monitor: MONITOR_GONE,
/// never MODE_REJECTED, and the entry stays for when the monitor returns (REQ-SAFE-09).
#[test]
fn req_safe_09_monitor_gone_after_the_apply_call_keeps_the_entry() {
    let (core, fakes) = fake_core(FixtureProfile::TestPc);
    fakes
        .display
        .script(DisplayFault::UnplugAfterApply, When::Once);
    let error = apply(&core, request(&lg_id(), stretched())).unwrap_err();
    assert_eq!(error.code(), ErrorCode::MonitorGone);
    assert_eq!(journal(&fakes).changes.len(), 1);
    assert!(core.snapshot().pending.is_none());
    // The window no longer lists it, and the change goes back when the monitor returns.
    assert!(
        core.snapshot()
            .monitors
            .iter()
            .all(|monitor| monitor.id != lg_id())
    );
    assert!(fakes.display.plug(&lg_id()));
    block_on(core.on_system_event(SystemEvent::DisplayChanged));
    assert_eq!(current(&fakes, &lg_id()), Some(native()));
    assert!(journal(&fakes).changes.is_empty());
}

/// Closing the keep-mode window while journal.json refuses every write raises the error once:
/// the countdown's own failed try at its end does not raise it again (D-029).
#[test]
fn req_safe_05_keep_window_closed_failure_is_raised_once() {
    let (core, fakes, _recorder, _window) = watched_core(FixtureProfile::TestPc);
    pending_id(&apply(&core, request(&lg_id(), stretched())).unwrap());
    fakes.store.script(
        StoreFault::Write(StoreFile::Journal, WriteStage::Rename),
        When::Always,
    );
    core.keep_window_closed();
    assert!(wait_for(|| !core.snapshot().errors.is_empty()));
    fakes.clock.advance(10_000);
    block_on(core.run_due_timers());
    let state = core.snapshot();
    let raised: Vec<_> = state
        .errors
        .iter()
        .filter(|raised| raised.error.code == ErrorCode::JournalWriteFailed)
        .collect();
    assert_eq!(raised.len(), 1);
    assert_eq!(raised[0].count, 1);
    assert!(state.pending.is_some());
}

/// On the GDI route a revert to a centred state sends DMDFO_CENTER, so the put-back verifies
/// instead of leaving the stretch of the pending change (D-035).
#[test]
fn req_safe_17_gdi_put_back_writes_a_centred_scaling() {
    let (core, fakes) = fake_core(FixtureProfile::TestPc);
    let kept = raw(1024, 768, 240);
    fakes
        .display
        .change_from_outside(&lg_id(), kept, Scaling::Centered, OutsideChange::Temporary);
    let id = pending_id(&apply(&core, request(&lg_id(), stretched())).unwrap());
    let reverted = block_on(core.revert(change(&id), RevertReason::Player)).unwrap();
    assert!(matches!(reverted, RevertOutcome::Reverted { .. }));
    let live = fakes.display.monitor(&lg_id()).unwrap();
    assert_eq!(live.current, Some(kept));
    assert_eq!(live.scaling, Scaling::Centered);
}

/// The GDI route has no value for an aspect-ratio state, so Windows picks and the put-back
/// reads back the stretch: RECOVERY_INCOMPLETE, never a revert reported as done; the CCD route
/// writes the recorded value and verifies (D-030, D-035).
#[test]
fn req_safe_17_put_back_scaling_follows_the_route() {
    for (route, verifies) in [
        (RouteChoice::gdi(GdiFlag::Fullscreen), false),
        (RouteChoice::ccd(), true),
    ] {
        let (core, fakes) = fake_core(FixtureProfile::TestPc);
        fakes.display.set_route(route);
        let kept = raw(1024, 768, 240);
        fakes.display.change_from_outside(
            &lg_id(),
            kept,
            Scaling::AspectRatio,
            OutsideChange::Temporary,
        );
        let id = pending_id(&apply(&core, request(&lg_id(), stretched())).unwrap());
        let answer = block_on(core.revert(change(&id), RevertReason::Player));
        let live = fakes.display.monitor(&lg_id()).unwrap();
        assert_eq!(live.current, Some(kept), "{route:?}");
        if verifies {
            assert!(matches!(answer.unwrap(), RevertOutcome::Reverted { .. }));
            assert_eq!(live.scaling, Scaling::AspectRatio);
        } else {
            assert_eq!(answer.unwrap_err().code(), ErrorCode::RecoveryIncomplete);
            assert_eq!(live.scaling, Scaling::Stretched);
        }
    }
}

/// A debug build on the Win32 backend kind leaves a journaled monitor that is not connected
/// at start for Restore everything: it is not marked missing, so its return puts nothing back
/// by itself (D-033).
#[test]
fn req_safe_07_debug_build_does_not_restore_a_monitor_that_returns() {
    if !cfg!(debug_assertions) {
        return;
    }
    let fakes = Fakes::new(FixtureProfile::TestPc);
    fakes.store.put(JOURNAL_FILE, OPEN_CHANGE.as_bytes());
    fakes.display.change_from_outside(
        &lg_id(),
        stretched(),
        Scaling::Stretched,
        OutsideChange::Temporary,
    );
    assert!(fakes.display.unplug(&lg_id()));
    let mut backends = fakes.backends();
    backends.kind = BackendKind::Real;
    let core = Core::new(backends);
    block_on(core.recover_at_start());
    assert_eq!(core.snapshot().recovery.status, RecoveryStatus::Incomplete);
    assert!(fakes.display.plug(&lg_id()));
    block_on(core.on_system_event(SystemEvent::DisplayChanged));
    assert!(!fakes.display.applied_anything());
    assert_eq!(current(&fakes, &lg_id()), Some(stretched()));
    assert_eq!(journal(&fakes).changes.len(), 1);
}

/// A debug build closes an entry whose monitor was changed outside ResSwap and reports
/// nothing for it, as a release build does (D-033).
#[test]
fn req_safe_07_debug_build_closes_a_monitor_changed_outside() {
    if !cfg!(debug_assertions) {
        return;
    }
    let fakes = Fakes::new(FixtureProfile::TestPc);
    fakes.store.put(JOURNAL_FILE, OPEN_CHANGE.as_bytes());
    let outside = raw(800, 600, 240);
    fakes.display.change_from_outside(
        &lg_id(),
        outside,
        Scaling::Stretched,
        OutsideChange::Temporary,
    );
    let mut backends = fakes.backends();
    backends.kind = BackendKind::Real;
    let core = Core::new(backends);
    block_on(core.recover_at_start());
    let state = core.snapshot();
    assert_eq!(state.recovery.status, RecoveryStatus::Clean);
    assert!(state.errors.is_empty());
    assert!(journal(&fakes).changes.is_empty());
    assert!(!fakes.display.applied_anything());
    assert_eq!(current(&fakes, &lg_id()), Some(outside));
}

/// A RECOVERY_INCOMPLETE raised by a revert that could not put the mode back leaves the list
/// once a Restore everything leaves nothing changed, whatever raised it (D-034).
#[test]
fn req_disp_22_clean_restore_clears_a_recovery_error_raised_later() {
    let (core, fakes, _recorder, _window) = watched_core(FixtureProfile::TestPc);
    pending_id(&apply(&core, request(&lg_id(), stretched())).unwrap());
    fakes
        .display
        .script(DisplayFault::ApplyFails(-1), When::Once);
    fakes.clock.advance(10_000);
    block_on(core.run_due_timers());
    assert!(
        core.snapshot()
            .errors
            .iter()
            .any(|raised| raised.error.code == ErrorCode::RecoveryIncomplete)
    );
    block_on(core.restore_everything(ChangeSource::Window)).unwrap();
    assert!(
        core.snapshot()
            .errors
            .iter()
            .all(|raised| raised.error.code != ErrorCode::RecoveryIncomplete)
    );
    assert_eq!(current(&fakes, &lg_id()), Some(native()));
}

/// While a moved-aside journal waits for its fallback, a Restore everything that puts every
/// listed change back leaves recovery incomplete; the next one runs the fallback and ends it
/// (D-023, D-036).
#[test]
fn req_safe_06_lost_journal_keeps_recovery_incomplete_until_its_fallback() {
    let fakes = Fakes::new(FixtureProfile::TestPc);
    fakes.store.put(JOURNAL_FILE, b"not a journal");
    let core = Core::new(fakes.backends());
    block_on(core.recover_at_start());
    assert_eq!(core.snapshot().recovery.status, RecoveryStatus::Incomplete);
    keep(
        &core,
        &pending_id(&apply(&core, request(&lg_id(), stretched())).unwrap()),
    );
    block_on(core.restore_everything(ChangeSource::Window)).unwrap();
    assert_eq!(core.snapshot().recovery.status, RecoveryStatus::Incomplete);
    assert!(
        !fakes
            .display
            .calls()
            .iter()
            .any(|call| matches!(call, DisplayCall::ApplyDatabaseCurrent { .. }))
    );
    block_on(core.restore_everything(ChangeSource::Window)).unwrap();
    assert!(
        fakes
            .display
            .calls()
            .iter()
            .any(|call| matches!(call, DisplayCall::ApplyDatabaseCurrent { .. }))
    );
    assert_eq!(core.snapshot().recovery.status, RecoveryStatus::Clean);
}

/// When recovery closes one monitor's entry because it was changed outside ResSwap while
/// another monitor still has an open change, the saved snapshot is dropped, so a later
/// Restore everything puts back only the listed monitor and never undoes the outside change
/// (D-036, REQ-WATCH-07).
#[test]
fn req_disp_22_restore_everything_leaves_a_monitor_changed_outside() {
    if !cfg!(debug_assertions) {
        return;
    }
    let fakes = Fakes::new(FixtureProfile::TwoMonitors);
    let lg = lg(FixtureProfile::TwoMonitors);
    let samsung = known_monitors(FixtureProfile::TwoMonitors).remove(2).id;
    {
        let first = Core::new(fakes.backends());
        keep(
            &first,
            &pending_id(&apply(&first, request(&lg, stretched())).unwrap()),
        );
        keep(
            &first,
            &pending_id(&apply(&first, request(&samsung, raw(1280, 1024, 60))).unwrap()),
        );
    }
    assert!(journal(&fakes).layout.is_some());
    let outside = raw(800, 600, 240);
    fakes
        .display
        .change_from_outside(&lg, outside, Scaling::Stretched, OutsideChange::Temporary);
    let mut backends = fakes.backends();
    backends.kind = BackendKind::Real;
    let core = Core::new(backends);
    block_on(core.recover_at_start());
    assert!(journal(&fakes).layout.is_none());
    block_on(core.restore_everything(ChangeSource::Window)).unwrap();
    assert_eq!(current(&fakes, &lg), Some(outside));
    assert_eq!(current(&fakes, &samsung), Some(raw(1920, 1080, 60)));
}
