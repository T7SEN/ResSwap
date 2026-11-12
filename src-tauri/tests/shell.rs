//! The exe refuses a bad argument list before the Tauri builder runs (REQ-SHELL-11).

mod common;

use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use common::run_folder;

/// A refused list exits before the builder, well under a second on the test PC, so a child
/// still running after this long has got past the parser.
const EXIT_WITHIN: Duration = Duration::from_secs(5);

#[test]
fn req_shell_11_bad_arguments_exit() {
    let exe = env!("CARGO_BIN_EXE_resswap");
    let id = "0f8fad5b-d9cb-469f-a165-70867728950e";
    let upper = id.to_uppercase();
    // Only refused lists: an accepted one would start the app. Should the parser ever accept
    // one, the fake backend and a data folder inside the target folder keep the run off the
    // real display and out of %LOCALAPPDATA%, and the time limit ends it.
    for refused in [
        vec!["--unknown"],
        vec!["--play"],
        vec!["--play", upper.as_str()],
        vec!["--play", id, "extra"],
        vec!["--autostart", "--autostart"],
    ] {
        let mut child = Command::new(exe)
            .args(&refused)
            .env("RESSWAP_BACKEND", "fake")
            .env("RESSWAP_DATA_DIR", run_folder("shell"))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("the exe starts");
        let started = Instant::now();
        let status = loop {
            if let Some(status) = child.try_wait().expect("the child can be waited on") {
                break status;
            }
            if started.elapsed() > EXIT_WITHIN {
                // Only this child: the test started it and ends it.
                let _ = child.kill();
                let _ = child.wait();
                panic!("{refused:?} did not exit within {EXIT_WITHIN:?}");
            }
            std::thread::sleep(Duration::from_millis(20));
        };
        assert_eq!(
            status.code(),
            Some(resswap_lib::BAD_ARGUMENTS),
            "{refused:?}"
        );
    }
}
