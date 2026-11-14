//! Helpers shared by the integration tests. Every test builds its `Core` here, on the
//! fakes, so no test reaches a backend that changes the PC.

#![allow(clippy::unwrap_used, clippy::expect_used, reason = "test helpers")]
#![allow(dead_code, reason = "each test file uses its own part of the helpers")]

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use resswap_lib::display::fake::FixtureProfile;
use resswap_lib::ipc::events::{CoreEvent, EventSink};
use resswap_lib::shell::fake::FakeKeepModeWindow;
use resswap_lib::{Core, DataFolders, Fakes, StartAction, create_app, start_up};
use serde_json::Value;
use tauri::ipc::{CallbackFn, InvokeBody};
use tauri::test::{INVOKE_KEY, MockRuntime, get_ipc_response, mock_builder};
use tauri::webview::InvokeRequest;
use tauri::{App, Manager, WebviewUrl, WebviewWindow, WebviewWindowBuilder};

/// A `Core` on the fakes of `profile`, with the fakes kept for scripting and call logs.
pub fn fake_core(profile: FixtureProfile) -> (Core, Fakes) {
    let fakes = Fakes::new(profile);
    (Core::new(fakes.backends()), fakes)
}

/// A `Core` on the fakes of `profile` that records every event it sends and opens the fake
/// keep-mode window, with the fakes, the recorder and the window kept for the test.
pub fn watched_core(
    profile: FixtureProfile,
) -> (Core, Fakes, Arc<Recorder>, Arc<FakeKeepModeWindow>) {
    watched_core_on(Fakes::new(profile))
}

/// `watched_core` over fakes a test prepared first.
pub fn watched_core_on(fakes: Fakes) -> (Core, Fakes, Arc<Recorder>, Arc<FakeKeepModeWindow>) {
    let core = Core::new(fakes.backends());
    let recorder = Arc::new(Recorder::default());
    let window = Arc::new(FakeKeepModeWindow::new());
    core.set_event_sink(recorder.clone());
    core.set_keep_mode_window(window.clone());
    (core, fakes, recorder, window)
}

/// Records every event the core sends, as a window that listens would receive them.
#[derive(Default)]
pub struct Recorder {
    events: Mutex<Vec<(CoreEvent, Value)>>,
}

impl Recorder {
    /// Every event name so far, in order.
    pub fn names(&self) -> Vec<&'static str> {
        self.events
            .lock()
            .unwrap()
            .iter()
            .map(|(event, _)| event.name())
            .collect()
    }

    /// The payloads of every `event` so far, in order.
    pub fn payloads(&self, event: CoreEvent) -> Vec<Value> {
        self.events
            .lock()
            .unwrap()
            .iter()
            .filter(|(sent, _)| *sent == event)
            .map(|(_, payload)| payload.clone())
            .collect()
    }

    /// Forgets the events so far.
    pub fn clear(&self) {
        self.events.lock().unwrap().clear();
    }
}

impl EventSink for Recorder {
    fn has_listeners(&self) -> bool {
        true
    }

    fn send(&self, event: CoreEvent, payload: Value) {
        self.events.lock().unwrap().push((event, payload));
    }
}

/// Waits up to five seconds for `ready`, for work the core queues on a task or a thread.
pub fn wait_for(ready: impl Fn() -> bool) -> bool {
    let started = Instant::now();
    while started.elapsed() < Duration::from_secs(5) {
        if ready() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    ready()
}

/// A folder for one test inside Cargo's target folder, so no test writes outside the
/// repository.
pub fn run_folder(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name)
}

/// The app on Tauri's mock runtime, built by the same `create_app` the real run uses.
pub struct TestApp {
    pub app: App<MockRuntime>,
    pub fakes: Fakes,
}

pub fn mock_app(profile: FixtureProfile) -> TestApp {
    mock_app_on(Fakes::new(profile))
}

/// The app on Tauri's mock runtime over `fakes`, for a test that scripts faults first.
pub fn mock_app_on(fakes: Fakes) -> TestApp {
    mock_app_started(fakes, StartAction::Default)
}

/// The app on Tauri's mock runtime over `fakes`, started as `action` asks.
pub fn mock_app_started(fakes: Fakes, action: StartAction) -> TestApp {
    let core = Core::new(fakes.backends());
    let folders = DataFolders::in_folder(run_folder("mock-app"));
    let app = create_app(mock_builder(), core, action.clone(), folders)
        .expect("the app builds on the mock runtime");
    // Setup runs when the event loop starts; the mock runtime has none, so start here.
    start_up(app.handle(), action);
    TestApp { app, fakes }
}

impl TestApp {
    /// The main window that start-up opened.
    pub fn main_window(&self) -> WebviewWindow<MockRuntime> {
        self.app.get_webview_window("main").unwrap_or_else(|| {
            let errors = self.app.state::<Core>().snapshot().errors;
            panic!("start-up did not open the main window: {errors:?}")
        })
    }

    /// A new window with `label`, as the core opens the keep-mode prompt.
    pub fn open_window(&self, label: &str) -> WebviewWindow<MockRuntime> {
        WebviewWindowBuilder::new(&self.app, label, WebviewUrl::default())
            .build()
            .expect("the mock runtime builds the window")
    }
}

/// Invokes `command` from `window` as the bundled page would, and returns the answer as
/// JSON: `Ok` with the value, or `Err` with the rejection.
pub fn invoke(
    window: &WebviewWindow<MockRuntime>,
    command: &str,
    body: Value,
) -> Result<Value, Value> {
    let request = InvokeRequest {
        cmd: command.into(),
        callback: CallbackFn(0),
        error: CallbackFn(1),
        url: "http://tauri.localhost".parse().unwrap(),
        body: InvokeBody::Json(body),
        headers: Default::default(),
        invoke_key: INVOKE_KEY.to_string(),
    };
    get_ipc_response(window, request).map(|body| body.deserialize::<Value>().unwrap())
}
