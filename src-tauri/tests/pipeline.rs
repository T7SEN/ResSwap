//! The pipeline's guard rules on the fakes: no command changes the fake PC without an open
//! journal entry, and no fake can reach the real PC.

mod common;

use std::path::{Path, PathBuf};

use common::fake_core;
use resswap_lib::Core;
use resswap_lib::display::fake::{FixtureProfile, known_monitors};
use resswap_lib::display::modes::to_mode;
use resswap_lib::display::types::{MonitorId, ScalingChoice};
use resswap_lib::display::{Rational, RawMode};
use resswap_lib::ipc::types::{
    AddCustomModeRequest, ApplyRequest, ChangeRef, ChangeSource, ErrorRef, ProfileRef,
    RemoveCustomEntryRequest, RestoreNativeRequest, RevertReason, SetAutoSwitchRequest,
    SetAutostartRequest, SetHotkeyRequest, UndoAllRequest,
};
use resswap_lib::profiles::Profile;
use resswap_lib::settings::{HotkeyAction, SettingsInput};
use tauri::async_runtime::block_on;

const ID: &str = "0f8fad5b-d9cb-469f-a165-70867728950e";

fn lg(profile: FixtureProfile) -> MonitorId {
    known_monitors(profile).remove(0).id
}

fn stretched() -> RawMode {
    RawMode {
        width: 1280,
        height: 960,
        refresh: Rational::hz(240),
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
    let change = || ChangeRef {
        change_id: String::from("change-1"),
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
    let _ = block_on(core.confirm(change(), source));
    let _ = block_on(core.revert(change(), RevertReason::Player));
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
/// the fakes, and none reaches a changing call before its journal entry is open. The other
/// half of the rule, a changing call with a closed entry, needs a Ticket, which only the
/// apply module can build; each fake's unit test covers it.
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
        // No command in this build does its changing work, so no fake recorded a change.
        assert!(!fakes.changed_anything(), "{profile:?}");
        assert!(fakes.store.calls().is_empty(), "{profile:?}");
        assert!(fakes.registry.calls().is_empty(), "{profile:?}");
        assert!(fakes.processes.calls().is_empty(), "{profile:?}");
    }
}

/// Every fake file in src, so a new fake is checked as soon as it exists.
#[allow(
    clippy::unwrap_used,
    reason = "test helper: an unreadable source folder fails the test"
)]
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
