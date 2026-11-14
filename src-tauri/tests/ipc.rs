//! The IPC contract on Tauri's mock runtime with the fake backend: every command is
//! registered, the capabilities hold exactly what each window needs, and a panic reaches
//! the window as INTERNAL. The log plugin's line layout is checked here too, because only
//! this file builds the app on the mock runtime.

mod common;

use std::collections::BTreeSet;

use common::{fake_core, invoke, mock_app, mock_app_on, mock_app_started, run_folder, wait_for};
use resswap_lib::display::fake::{DisplayFault, FixtureProfile, When};
use resswap_lib::ipc::commands::{CommandFaults, CommandName};
use resswap_lib::profiles::ProfileId;
use resswap_lib::{DataFolders, Fakes, StartAction, create_app, log_app_started, log_plugin};
use serde_json::{Value, json};
use tauri::Manager;
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
/// test in this file that installs the global logger, so it also checks the rotation
/// settings: a 750,000 byte file is not rotated (the plugin's default limit is 40,000
/// bytes), and of seven renamed files the five newest stay (D-027).
// Also REQ-SET-09.
#[test]
fn req_perf_05_app_started_line_reaches_the_log_file() {
    const SEED_LINE: &str = "[2026-01-01][00:00:00][seed][INFO] a line from an earlier run\n";
    let folder = run_folder("log-plugin");
    let _ = std::fs::remove_dir_all(&folder);
    let logs = folder.join("logs");
    std::fs::create_dir_all(&logs).unwrap();
    let seed = SEED_LINE.repeat(750_000_usize.div_ceil(SEED_LINE.len()));
    std::fs::write(logs.join("ResSwap.log"), &seed).unwrap();
    let renamed = |day: u32| format!("ResSwap_2026-01-0{day}_00-00-00.log");
    for day in 1..=7 {
        std::fs::write(logs.join(renamed(day)), format!("day {day}\n")).unwrap();
    }

    let folders = DataFolders::in_folder(folder.clone());
    let (core, _fakes) = fake_core(FixtureProfile::TestPc);
    let builder = mock_builder().plugin(log_plugin(&folders));
    let _app = create_app(builder, core, StartAction::Default, folders).unwrap();
    resswap_lib::mark_process_start();
    log_app_started();

    let mut kept: Vec<String> = std::fs::read_dir(&logs)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .filter(|name| name.starts_with("ResSwap_"))
        .collect();
    kept.sort();
    assert_eq!(kept, (3..=7).map(renamed).collect::<Vec<_>>());

    let text = std::fs::read_to_string(logs.join("ResSwap.log")).unwrap();
    let written = text
        .strip_prefix(seed.as_str())
        .unwrap_or_else(|| panic!("ResSwap.log was rotated below 750,000 bytes"));
    let line = written
        .lines()
        .find(|line| line.contains("PERF app.started"))
        .unwrap_or_else(|| panic!("no app.started line in: {written}"));
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
    // The scripted fault answers before any step, so no keep-mode window opens.
    std::thread::sleep(std::time::Duration::from_millis(100));
    assert!(test.app.get_webview_window("keep-mode").is_none());
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

/// The pipeline through the real commands on the mock runtime: apply_mode answers
/// pendingConfirm, the core opens the keep-mode window from its own thread, and Keep from
/// that window keeps the change with the same changeId.
// Also REQ-UI-06, REQ-SAFE-04, REQ-SEC-08.
#[test]
fn req_safe_03_keep_mode_window_on_the_mock_runtime() {
    let test = mock_app(FixtureProfile::TestPc);
    let main = test.main_window();
    let typed = json!({
        "request": {
            "kind": "mode",
            "monitor": monitor_id(),
            "mode": {
                "width": 1280,
                "height": 960,
                "refresh": { "numerator": 240, "denominator": 1, "label": "" },
                "label": ""
            },
            "scaling": "fullScreen"
        }
    });
    let answer = invoke(&main, "apply_mode", typed.clone()).unwrap();
    assert_eq!(answer["kind"], "pendingConfirm");
    assert_eq!(answer["pending"]["to"]["label"], "1280 x 960 at 240 Hz");
    let change_id = answer["pending"]["changeId"].as_str().unwrap().to_owned();
    assert!(wait_for(|| test
        .app
        .get_webview_window("keep-mode")
        .is_some()));
    let keep_mode = test.app.get_webview_window("keep-mode").unwrap();
    let state = invoke(&keep_mode, "get_state", json!({})).unwrap();
    assert_eq!(state["pending"]["changeId"], change_id.as_str());
    assert_eq!(state["pending"]["secondsLeft"], 10);
    let kept = invoke(
        &keep_mode,
        "confirm_mode",
        json!({ "request": { "changeId": change_id } }),
    )
    .unwrap();
    assert_eq!(kept["kind"], "kept");
    assert_eq!(kept["change"]["changeId"], change_id.as_str());
    // The mock runtime queues the destroy for an event loop it never runs, so the window
    // going away is checked by e2e:app on the real runtime.
    let state = invoke(&main, "get_state", json!({})).unwrap();
    assert!(state["pending"].is_null());
    assert_eq!(state["active"][0]["changeId"], change_id.as_str());
    assert_eq!(
        state["settings"]["confirmedModes"]
            .as_array()
            .unwrap()
            .len(),
        1
    );

    // Kept, so the same size applies with no keep-mode window after Restore native.
    let restored = invoke(
        &main,
        "restore_native",
        json!({ "request": { "monitor": monitor_id() } }),
    )
    .unwrap();
    assert_eq!(restored["restored"][0], monitor_id());
    let again = invoke(&main, "apply_mode", typed).unwrap();
    assert_eq!(again["kind"], "applied");

    // Restore everything puts it back and answers what it restored; then nothing is left.
    let everything = invoke(&main, "restore_everything", json!({})).unwrap();
    assert_eq!(everything["restored"][0], monitor_id());
    let nothing = invoke(&main, "restore_everything", json!({})).unwrap();
    assert_eq!(nothing, json!({ "restored": [], "steps": [] }));
    assert!(
        test.fakes
            .display
            .flag_zero_while_fullscreen_open()
            .is_empty()
    );
}

/// Revert from the keep-mode window puts the previous mode back; requests the window should never send are refused in Rust as INTERNAL (REQ-SEC-08).
#[test]
fn req_safe_03_revert_from_the_keep_mode_window() {
    let test = mock_app(FixtureProfile::TestPc);
    let main = test.main_window();
    let answer = invoke(
        &main,
        "apply_mode",
        json!({
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
        }),
    )
    .unwrap();
    let change_id = answer["pending"]["changeId"].as_str().unwrap().to_owned();
    assert!(wait_for(|| test
        .app
        .get_webview_window("keep-mode")
        .is_some()));
    let keep_mode = test.app.get_webview_window("keep-mode").unwrap();
    let reverted = invoke(
        &keep_mode,
        "revert_mode",
        json!({ "request": { "changeId": change_id } }),
    )
    .unwrap();
    assert_eq!(reverted["kind"], "reverted");
    assert_eq!(reverted["reverted"]["reason"], "player");
    assert_eq!(
        reverted["reverted"]["restored"]["label"],
        "1920 x 1080 at 240 Hz"
    );

    let zero = invoke(
        &main,
        "apply_mode",
        json!({
            "request": {
                "kind": "mode",
                "monitor": monitor_id(),
                "mode": {
                    "width": 1280,
                    "height": 960,
                    "refresh": { "numerator": 240, "denominator": 0, "label": "" },
                    "label": ""
                },
                "scaling": "keep"
            }
        }),
    );
    assert_eq!(code(&zero), Some("INTERNAL"), "{zero:?}");
    let state = invoke(&main, "get_state", json!({})).unwrap();
    assert_eq!(state["active"], json!([]));
    assert!(state["pending"].is_null());
}

/// Start-up loads the journal and recovers before the window opens (REQ-SAFE-06): the
/// change a crash left on screen goes back, and the state says recovered.
#[test]
fn req_safe_06_start_up_recovers_before_the_window() {
    use resswap_lib::display::fake::OutsideChange;
    use resswap_lib::display::types::Scaling;
    use resswap_lib::display::{Rational, RawMode};

    let fakes = Fakes::new(FixtureProfile::TestPc);
    fakes.store.put(
        "journal.json",
        include_str!("fixtures/journal-v1-open-change.json").as_bytes(),
    );
    let lg = resswap_lib::display::fake::known_monitors(FixtureProfile::TestPc)
        .remove(0)
        .id;
    let stretched = RawMode {
        width: 1280,
        height: 960,
        refresh: Rational::hz(240),
    };
    fakes
        .display
        .change_from_outside(&lg, stretched, Scaling::Stretched, OutsideChange::Temporary);
    let test = mock_app_on(fakes);
    let state = invoke(&test.main_window(), "get_state", json!({})).unwrap();
    assert_eq!(state["recovery"]["status"], "recovered");
    assert_eq!(
        state["monitors"][0]["current"]["label"],
        "1920 x 1080 at 240 Hz"
    );
    assert!(test.app.get_webview_window("keep-mode").is_none());
}

/// A damaged settings.json is raised at start as SETTINGS_CORRUPT with source startup, and
/// the defaults load (REQ-SET-05).
#[test]
fn req_set_05_damaged_settings_are_raised_at_start() {
    let fakes = Fakes::new(FixtureProfile::TestPc);
    fakes.store.put("settings.json", b"{ not json");
    let test = mock_app_on(fakes);
    let state = invoke(&test.main_window(), "get_state", json!({})).unwrap();
    let errors = state["errors"].as_array().unwrap();
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert_eq!(errors[0]["source"], "startup");
    assert_eq!(errors[0]["error"]["code"], "SETTINGS_CORRUPT");
    assert_eq!(errors[0]["error"]["context"]["settingsSource"], "defaults");
    assert_eq!(state["settings"]["revertCountdownSecs"], 10);
}

/// save_settings, open_log_folder and get_diagnostics answer through the real commands:
/// the setting is stored and sent back, the fake opener gets the run's log folder, and the
/// diagnostics carry folders with environment variables only (REQ-SET-09).
// Also REQ-DISP-08, REQ-DISP-25.
#[test]
fn req_set_09_settings_log_folder_and_diagnostics_commands() {
    let test = mock_app(FixtureProfile::TestPc);
    let main = test.main_window();
    let before = invoke(&main, "get_state", json!({})).unwrap()["revision"]
        .as_u64()
        .unwrap();
    let saved = invoke(
        &main,
        "save_settings",
        json!({
            "request": {
                "revertCountdownSecs": 10,
                "checkForUpdates": true,
                "showHiddenMonitors": true,
                "selectedMonitor": null,
                "playStretchedProfile": null
            }
        }),
    )
    .unwrap();
    assert_eq!(saved["showHiddenMonitors"], true);
    let state = invoke(&main, "get_state", json!({})).unwrap();
    assert!(state["revision"].as_u64().unwrap() > before);
    assert_eq!(state["settings"]["showHiddenMonitors"], true);

    invoke(&main, "open_log_folder", json!({})).unwrap();
    assert_eq!(
        test.fakes.folder_opener.calls(),
        [run_folder("mock-app").join("logs")]
    );

    let diagnostics = invoke(&main, "get_diagnostics", json!({})).unwrap();
    assert_eq!(diagnostics["build"]["backend"], "fake");
    assert_eq!(diagnostics["settingsFolder"], "%RESSWAP_DATA_DIR%");
    assert_eq!(diagnostics["monitors"].as_array().unwrap().len(), 2);
    assert_eq!(diagnostics["journalOpenItems"], json!([]));
    assert!(!test.fakes.changed_anything());
}

/// save_settings refuses a revert countdown of 0 or of more than 60 seconds as INTERNAL and
/// keeps the stored value; 1 and 60 are stored (REQ-SET-08, D-028).
// Also REQ-SEC-08.
#[test]
fn req_set_08_revert_countdown_between_1_and_60_seconds() {
    let test = mock_app(FixtureProfile::TestPc);
    let main = test.main_window();
    let save = |secs: u64| {
        invoke(
            &main,
            "save_settings",
            json!({
                "request": {
                    "revertCountdownSecs": secs,
                    "checkForUpdates": true,
                    "showHiddenMonitors": false,
                    "selectedMonitor": null,
                    "playStretchedProfile": null
                }
            }),
        )
    };
    for refused in [0, 61, u64::from(u32::MAX)] {
        let answer = save(refused);
        assert_eq!(code(&answer), Some("INTERNAL"), "{refused}: {answer:?}");
    }
    let state = invoke(&main, "get_state", json!({})).unwrap();
    assert_eq!(state["settings"]["revertCountdownSecs"], 10);
    for stored in [1, 60] {
        let saved = save(stored).unwrap();
        assert_eq!(saved["revertCountdownSecs"], stored);
    }
    let state = invoke(&main, "get_state", json!({})).unwrap();
    assert_eq!(state["settings"]["revertCountdownSecs"], 60);
    assert!(!test.fakes.changed_anything());
}
