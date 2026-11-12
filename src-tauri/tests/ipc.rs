//! The IPC contract on Tauri's mock runtime with the fake backend: every command is
//! registered, the capabilities hold exactly what each window needs, and a panic reaches
//! the window as INTERNAL. The log plugin's line layout is checked here too, because only
//! this file builds the app on the mock runtime.

mod common;

use std::collections::BTreeSet;

use common::{fake_core, invoke, mock_app, mock_app_on, mock_app_started, run_folder};
use resswap_lib::display::fake::{DisplayFault, FixtureProfile, When};
use resswap_lib::ipc::commands::{CommandFaults, CommandName};
use resswap_lib::profiles::ProfileId;
use resswap_lib::{DataFolders, Fakes, StartAction, create_app, log_app_started, log_plugin};
use serde_json::{Value, json};
use tauri::test::mock_builder;

const EVENT_PERMISSIONS: [&str; 2] = ["core:event:allow-listen", "core:event:allow-unlisten"];

const PROFILE_ID: &str = "0f8fad5b-d9cb-469f-a165-70867728950e";

#[allow(
    clippy::unwrap_used,
    reason = "test helper: a malformed capability file fails the test"
)]
fn capability(identifier: &str) -> Value {
    let file: Value = serde_json::from_str(include_str!("../capabilities/main.json")).unwrap();
    file["capabilities"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["identifier"] == identifier)
        .cloned()
        .unwrap_or_else(|| panic!("capabilities/main.json has no {identifier} capability"))
}

#[allow(
    clippy::unwrap_used,
    reason = "test helper: a malformed capability file fails the test"
)]
fn permissions(capability: &Value) -> BTreeSet<String> {
    capability["permissions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|permission| permission.as_str().unwrap().to_owned())
        .collect()
}

fn code(answer: &Result<Value, Value>) -> Option<&str> {
    answer
        .as_ref()
        .err()
        .and_then(|error| error["code"].as_str())
}

fn monitor_id() -> Value {
    json!({
        "devicePath": "fake-device-path:test-pc:lg-ultragear",
        "edidManufacturerId": 7789,
        "edidProductCode": 23438,
        "connectorInstance": 0
    })
}

#[test]
fn req_sec_07_every_command_registered() {
    let test = mock_app(FixtureProfile::TestPc);
    let main = test.main_window();
    for command in CommandName::ALL {
        // An empty body: commands that take a request answer with a deserialisation error,
        // which still proves the command exists and the main window may call it.
        if let Err(error) = invoke(&main, command.name(), json!({})) {
            let text = error.to_string();
            assert!(!text.contains("not found"), "{}: {text}", command.name());
            assert!(!text.contains("not allowed"), "{}: {text}", command.name());
        }
    }
    assert!(!test.fakes.display.applied_anything());
}

#[test]
fn req_sec_07_keep_mode_capability_minimal() {
    let keep_mode = capability("keep-mode");
    let mut expected: BTreeSet<String> =
        EVENT_PERMISSIONS.iter().map(|p| (*p).to_owned()).collect();
    for command in [
        CommandName::GetState,
        CommandName::ConfirmMode,
        CommandName::RevertMode,
    ] {
        expected.insert(command.permission());
    }
    assert_eq!(permissions(&keep_mode), expected);
    assert_eq!(keep_mode["windows"], json!(["keep-mode"]));
    assert_eq!(keep_mode["local"], json!(true));
    assert!(keep_mode.get("remote").is_none());

    let main = capability("main");
    let mut expected: BTreeSet<String> =
        EVENT_PERMISSIONS.iter().map(|p| (*p).to_owned()).collect();
    expected.extend(CommandName::ALL.iter().map(|command| command.permission()));
    assert_eq!(permissions(&main), expected);
    assert_eq!(main["windows"], json!(["main"]));
    assert!(main.get("remote").is_none());

    // The same rules at run time: the keep-mode window reads the state and nothing else.
    let test = mock_app(FixtureProfile::TestPc);
    let window = test.open_window("keep-mode");
    assert!(invoke(&window, "get_state", json!({})).is_ok());
    let refused = invoke(&window, "list_monitors", json!({})).unwrap_err();
    assert!(refused.to_string().contains("not allowed"), "{refused}");
    let not_pending = invoke(
        &window,
        "revert_mode",
        json!({ "request": { "changeId": "c1" } }),
    );
    assert_eq!(not_pending.unwrap(), json!({ "kind": "notPending" }));
}

#[test]
fn req_disp_01_get_state_on_the_fake() {
    let test = mock_app(FixtureProfile::TestPc);
    let state = invoke(&test.main_window(), "get_state", json!({})).unwrap();
    assert_eq!(state["build"]["backend"], "fake");
    assert_eq!(state["build"]["debug"], json!(cfg!(debug_assertions)));
    assert_eq!(state["autoSwitch"]["enabled"], json!(false));
    assert_eq!(state["settings"]["revertCountdownSecs"], json!(10));
    assert_eq!(state["settings"]["hotkeys"], json!([]));
    assert_eq!(state["recovery"]["status"], "clean");
    assert!(state["pending"].is_null());
    let monitors = state["monitors"].as_array().unwrap();
    assert_eq!(monitors.len(), 2);
    assert_eq!(monitors[0]["name"], "LG ULTRAGEAR");
    assert!(state["revision"].as_u64().unwrap() >= 1);
}

#[test]
fn req_disp_01_list_monitors_on_the_fake() {
    let test = mock_app(FixtureProfile::TestPc);
    let monitors = invoke(&test.main_window(), "list_monitors", json!({})).unwrap();
    let lg = &monitors[0];
    assert_eq!(lg["vendor"], "nvidia");
    assert_eq!(lg["hidden"], json!(false));
    assert!(lg["hiddenReason"].is_null());
    assert_eq!(lg["native"]["label"], "1920 x 1080 at 240 Hz");
    assert_eq!(
        lg["current"]["refresh"],
        json!({ "numerator": 240, "denominator": 1, "label": "240 Hz" })
    );
    let presets: Vec<&str> = lg["presets"]
        .as_array()
        .unwrap()
        .iter()
        .map(|preset| preset["mode"]["label"].as_str().unwrap())
        .collect();
    assert_eq!(
        presets,
        vec![
            "1440 x 1080 at 240 Hz",
            "1350 x 1080 at 240 Hz",
            "1728 x 1080 at 240 Hz"
        ]
    );
    let parsec = &monitors[1];
    assert_eq!(parsec["hidden"], json!(true));
    assert_eq!(parsec["hiddenReason"], "virtualAdapter");
    assert!(parsec["name"].is_null());
    assert!(!test.fakes.display.applied_anything());
}

#[test]
fn req_ui_09_panicking_command_is_internal() {
    let test = mock_app(FixtureProfile::TestPc);
    test.fakes
        .display
        .script(DisplayFault::EnumeratePanics, When::Once);
    let answer = invoke(&test.main_window(), "list_monitors", json!({}));
    assert_eq!(code(&answer), Some("INTERNAL"), "{answer:?}");
    assert_eq!(
        answer.unwrap_err()["detail"],
        "internal error: a command stopped before it finished"
    );
    // The next call works again.
    assert!(invoke(&test.main_window(), "list_monitors", json!({})).is_ok());
}

#[test]
fn req_ui_09_failed_enumeration_is_internal() {
    let test = mock_app(FixtureProfile::TestPc);
    test.fakes
        .display
        .script(DisplayFault::EnumerateFails, When::Once);
    let answer = invoke(&test.main_window(), "list_monitors", json!({}));
    assert_eq!(code(&answer), Some("INTERNAL"), "{answer:?}");
}

/// The app.started line reaches ResSwap.log under the data folder in the log plugin's
/// default layout, `[YYYY-MM-DD][HH:MM:SS][perf][INFO] PERF app.started mono_ms=0
/// epoch_ms=<n>`, which the measurement script and the first-paint test parse. The only
/// test in this file that installs the global logger.
#[test]
fn req_perf_05_app_started_line_reaches_the_log_file() {
    let folder = run_folder("log-plugin");
    let _ = std::fs::remove_dir_all(&folder);
    let folders = DataFolders::in_folder(folder.clone());
    let (core, _fakes) = fake_core(FixtureProfile::TestPc);
    let builder = mock_builder().plugin(log_plugin(&folders));
    let _app = create_app(builder, core, StartAction::Default, folders).unwrap();
    resswap_lib::mark_process_start();
    log_app_started();

    let text = std::fs::read_to_string(folder.join("logs").join("ResSwap.log")).unwrap();
    let line = text
        .lines()
        .find(|line| line.contains("PERF app.started"))
        .unwrap_or_else(|| panic!("no app.started line in: {text}"));
    let (prefix, message) = line.split_once("[perf][INFO] ").unwrap();
    // [2026-11-12][03:04:05]: digits where the date and time have them.
    let shape: String = prefix
        .chars()
        .map(|c| if c.is_ascii_digit() { '9' } else { c })
        .collect();
    assert_eq!(shape, "[9999-99-99][99:99:99]", "{line}");
    let epoch = message
        .strip_prefix("PERF app.started mono_ms=0 epoch_ms=")
        .unwrap_or_else(|| panic!("unexpected message: {message}"));
    let epoch: u64 = epoch.parse().unwrap();
    // 1 Jan 2026 00:00:00 UTC: any PC running this test is later.
    assert!(epoch > 1_767_225_600_000, "{epoch}");
}

/// The debug variable RESSWAP_FAKE_FAIL reaches the window as the named code, from the real
/// core on the fakes.
#[test]
fn req_ui_08_scripted_command_faults_reach_the_window() {
    let faults = CommandFaults::parse("apply_mode:MODE_REJECTED,list_monitors:MONITOR_GONE");
    let test = mock_app_on(Fakes::new(FixtureProfile::TestPc).with_command_faults(faults));
    let main = test.main_window();
    let apply = invoke(
        &main,
        "apply_mode",
        json!({ "request": { "kind": "profile", "profileId": "0f8fad5b-d9cb-469f-a165-70867728950e" } }),
    );
    assert_eq!(code(&apply), Some("MODE_REJECTED"), "{apply:?}");
    assert_eq!(
        apply.unwrap_err()["detail"],
        "Windows did not accept the mode: scripted fault for apply_mode"
    );
    let monitors = invoke(&main, "list_monitors", json!({}));
    assert_eq!(code(&monitors), Some("MONITOR_GONE"), "{monitors:?}");
    // A command with no fault answers as before.
    assert!(invoke(&main, "get_settings", json!({})).is_ok());
    assert!(!test.fakes.changed_anything());
}

/// A fault entry with a typo is raised at start-up instead of passing silently.
#[test]
fn req_ui_08_refused_fault_entries_are_raised_at_start() {
    let faults = CommandFaults::parse("apply_mode:NOT_A_CODE");
    let test = mock_app_on(Fakes::new(FixtureProfile::TestPc).with_command_faults(faults));
    let state = invoke(&test.main_window(), "get_state", json!({})).unwrap();
    let errors = state["errors"].as_array().unwrap();
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert_eq!(errors[0]["source"], "startup");
    assert_eq!(errors[0]["error"]["code"], "INTERNAL");
    assert_eq!(
        errors[0]["error"]["detail"],
        "internal error: scripted fault entry refused: apply_mode:NOT_A_CODE"
    );
}

/// A RESSWAP_FAKE_PROFILE value that names no fixture is raised at start-up, and the run
/// goes on with test-pc.
#[test]
fn req_sec_05_unknown_fixture_name_is_raised_at_start() {
    let fakes = Fakes::new(FixtureProfile::TestPc).with_refused_fixture("two-monitor");
    let test = mock_app_on(fakes);
    let state = invoke(&test.main_window(), "get_state", json!({})).unwrap();
    let errors = state["errors"].as_array().unwrap();
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert_eq!(errors[0]["source"], "startup");
    assert_eq!(errors[0]["error"]["code"], "INTERNAL");
    assert_eq!(
        errors[0]["error"]["detail"],
        "internal error: fake fixture name refused: two-monitor; this run uses test-pc"
    );
    assert_eq!(state["monitors"].as_array().unwrap().len(), 2);

    // A known fixture raises nothing.
    let test = mock_app(FixtureProfile::TwoMonitors);
    let state = invoke(&test.main_window(), "get_state", json!({})).unwrap();
    assert_eq!(state["errors"], json!([]));
}

/// Start-up runs no elevated tool, launches nothing and writes no shortcut on its own, for
/// every start action. Play answers INTERNAL in this build before it reaches either.
#[test]
fn req_disp_01_runner_and_launcher_start_nothing_on_their_own() {
    let play = StartAction::Play {
        profile_id: ProfileId::parse(PROFILE_ID).unwrap(),
    };
    for action in [StartAction::Default, StartAction::Autostart, play] {
        let test = mock_app_started(Fakes::new(FixtureProfile::TestPc), action.clone());
        assert!(test.fakes.runner.calls().is_empty(), "{action:?}");
        assert!(test.fakes.launcher.calls().is_empty(), "{action:?}");
        assert!(test.fakes.shortcuts.calls().is_empty(), "{action:?}");
        assert!(!test.fakes.changed_anything(), "{action:?}");
        // The Play start reached the core, which raised its answer.
        let state = invoke(&test.main_window(), "get_state", json!({})).unwrap();
        let play_raised = state["errors"]
            .as_array()
            .unwrap()
            .iter()
            .any(|error| error["source"] == "playShortcut");
        assert_eq!(
            play_raised,
            matches!(action, StartAction::Play { .. }),
            "{action:?}"
        );
    }
}

#[test]
fn req_safe_17_commands_without_their_work_change_nothing() {
    let test = mock_app(FixtureProfile::TestPc);
    let main = test.main_window();
    let request = json!({
        "request": {
            "kind": "mode",
            "monitor": monitor_id(),
            "mode": {
                "width": 1280,
                "height": 960,
                "refresh": { "numerator": 240, "denominator": 1, "label": "240 Hz" },
                "label": "1280 x 960 at 240 Hz"
            },
            "scaling": "fullScreen"
        }
    });
    let answer = invoke(&main, "apply_mode", request);
    assert_eq!(code(&answer), Some("INTERNAL"), "{answer:?}");
    let restore = invoke(&main, "restore_everything", json!({}));
    assert_eq!(code(&restore), Some("INTERNAL"), "{restore:?}");
    let confirm = invoke(
        &main,
        "confirm_mode",
        json!({ "request": { "changeId": "c1" } }),
    );
    assert_eq!(confirm.unwrap(), json!({ "kind": "notPending" }));
    assert!(!test.fakes.changed_anything());
    let state = invoke(&main, "get_state", json!({})).unwrap();
    assert_eq!(state["active"], json!([]));
}
