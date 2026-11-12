//! Helpers shared by the integration tests. Every test builds its `Core` here, on the
//! fakes, so no test reaches a backend that changes the PC.

#![allow(clippy::unwrap_used, clippy::expect_used, reason = "test helpers")]
#![allow(dead_code, reason = "each test file uses its own part of the helpers")]

use std::path::PathBuf;

use resswap_lib::display::fake::FixtureProfile;
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
