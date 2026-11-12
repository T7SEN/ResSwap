//! The fake elevated-tool runner: it builds each tool's full System32 path and parameters,
//! records them, and answers with a scripted result. It starts no program and shows no
//! administrator prompt, so no test can (REQ-SAFE-10).

#![forbid(unsafe_code)]

use std::path::PathBuf;
use std::sync::Arc;

use crate::apply::{OpenEntries, Ticket};
use crate::display::fake::{CallLog, FaultScript, When};
use crate::elevation::{ElevatedCommand, ElevatedToolRunner, LaunchResult, SystemDirectory};
use crate::error::ElevatedTool;
use crate::vendor::nvidia::fake::FakeRegistry;
use crate::vendor::nvidia::nv_modes::{NvModesKind, RawKind, RawValue};

/// The System32 folder of the test PC, where nvapi64.dll was read on 11 Nov 2026.
const FAKE_SYSTEM_DIRECTORY: &str = r"C:\Windows\System32";

/// A failure a test scripts into the fake.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RunnerFault {
    /// The run reports this result and changes nothing: Declined is INJ-E08 and INJ-E18,
    /// LaunchFailed INJ-E09, NoProcess INJ-E10, TimedOut INJ-E11, a nonzero exit INJ-E12.
    Reports(LaunchResult),
    /// reg.exe exits with 0 but the registry keeps its value (INJ-E13).
    ExitsWithoutWriting,
}

/// One run the fake received, in order: what the real runner would have started.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunnerCall {
    pub change_id: String,
    pub tool: ElevatedTool,
    /// The full path of the tool, lpFile.
    pub file: PathBuf,
    /// lpParameters.
    pub parameters: String,
}

/// The elevated-tool runner for every test and every fake-backend run.
pub struct FakeRunner {
    journal: OpenEntries,
    registry: Arc<FakeRegistry>,
    system: Option<SystemDirectory>,
    log: CallLog<RunnerCall>,
    script: FaultScript<RunnerFault>,
}

impl FakeRunner {
    /// The fake, checking every Ticket against `journal` and writing NV_Modes into
    /// `registry` the way reg.exe would.
    pub fn new(journal: OpenEntries, registry: Arc<FakeRegistry>) -> Self {
        Self {
            journal,
            registry,
            system: SystemDirectory::checked(PathBuf::from(FAKE_SYSTEM_DIRECTORY)),
            log: CallLog::default(),
            script: FaultScript::default(),
        }
    }

    /// Adds a fault to the script.
    pub fn script(&self, fault: RunnerFault, when: When) {
        self.script.push(fault, when);
    }

    /// Every run so far, in order.
    pub fn calls(&self) -> Vec<RunnerCall> {
        self.log.calls()
    }

    /// Whether any run so far reached the fake PC.
    pub fn ran_anything(&self) -> bool {
        self.log.any(|_| true)
    }
}

impl ElevatedToolRunner for FakeRunner {
    fn run(&self, ticket: &Ticket, command: &ElevatedCommand) -> LaunchResult {
        // A change with no open journal entry is a pipeline bug (pc_changes_need_ticket).
        self.journal.require_open(ticket);
        let Some(system) = &self.system else {
            // The fixture path failed its own check, so the launch could not be prepared.
            // No Win32 call ran, so there is no error code. A unit test keeps this branch
            // unused.
            return LaunchResult::LaunchFailed { win32: 0 };
        };
        let (file, parameters) = command.file_and_parameters(system);
        self.log.record(RunnerCall {
            change_id: ticket.change_id().to_owned(),
            tool: command.tool(),
            file,
            parameters,
        });
        match self.script.due(|_| true) {
            Some(RunnerFault::Reports(result)) => return result,
            Some(RunnerFault::ExitsWithoutWriting) => return LaunchResult::Exited { code: 0 },
            None => {}
        }
        if let ElevatedCommand::RegAdd(args) = command {
            let kind = match args.kind() {
                NvModesKind::MultiSz => RawKind::MultiSz,
                NvModesKind::Sz => RawKind::Sz,
            };
            self.registry.set_nv_modes(
                args.key().as_str(),
                Some(RawValue {
                    kind,
                    strings: vec![args.data().as_str().to_owned()],
                }),
            );
        }
        LaunchResult::Exited { code: 0 }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::display::fake::FixtureProfile;
    use crate::elevation::{RegAddArgs, RestartDeviceArgs};
    use crate::vendor::VendorExtras;
    use crate::vendor::fake::FakeVendor;
    use crate::vendor::nvidia::nv_modes::RegistryReader;

    struct Setup {
        journal: OpenEntries,
        registry: Arc<FakeRegistry>,
        runner: FakeRunner,
        reg_add: ElevatedCommand,
        restart: ElevatedCommand,
    }

    /// The commands built the way the NVIDIA work will: values from the fake enumeration
    /// and the fake registry read.
    fn setup() -> Setup {
        let journal = OpenEntries::default();
        let registry = Arc::new(FakeRegistry::new(FixtureProfile::TestPc));
        let vendor = FakeVendor::new(FixtureProfile::TestPc, journal.clone());
        let lg = crate::display::fake::known_monitors(FixtureProfile::TestPc)
            .remove(0)
            .id;
        let adapter = vendor.nvidia_adapter(&lg).unwrap();
        let value = registry.nv_modes(&adapter.class_key).unwrap().unwrap();
        Setup {
            runner: FakeRunner::new(journal.clone(), registry.clone()),
            reg_add: ElevatedCommand::RegAdd(RegAddArgs::new(&adapter.class_key, &value)),
            restart: ElevatedCommand::RestartDevice(RestartDeviceArgs::new(&adapter.instance_id)),
            journal,
            registry,
        }
    }

    #[test]
    fn req_safe_10_runner_fake_builds_paths_and_starts_nothing() {
        let setup = setup();
        assert!(setup.runner.system.is_some());
        let ticket = setup.journal.open_for_tests("op-1");
        assert_eq!(
            setup.runner.run(&ticket, &setup.reg_add),
            LaunchResult::Exited { code: 0 }
        );
        assert_eq!(
            setup.runner.run(&ticket, &setup.restart),
            LaunchResult::Exited { code: 0 }
        );
        let calls = setup.runner.calls();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].tool, ElevatedTool::RegExe);
        assert_eq!(calls[0].file, PathBuf::from(r"C:\Windows\System32\reg.exe"));
        assert!(calls[0].parameters.starts_with(
            r"add HKLM\SYSTEM\CurrentControlSet\Control\Class\{4d36e968-e325-11ce-bfc1-08002be10318}\0001 /v NV_Modes /t REG_MULTI_SZ /d "
        ));
        assert!(calls[0].parameters.ends_with(r#"=1F;" /f"#));
        assert_eq!(calls[1].tool, ElevatedTool::Pnputil);
        assert_eq!(
            calls[1].file,
            PathBuf::from(r"C:\Windows\System32\pnputil.exe")
        );
        assert_eq!(
            calls[1].parameters,
            r"/restart-device PCI\VEN_10DE&DEV_2803&SYNTHETIC_SUFFIX"
        );
        assert!(setup.runner.ran_anything());
    }

    // Also INJ-E08.
    #[test]
    fn req_safe_14_inj_e13_runner_fake_results_follow_the_script() {
        let setup = setup();
        let ticket = setup.journal.open_for_tests("op-1");
        setup
            .runner
            .script(RunnerFault::Reports(LaunchResult::Declined), When::Once);
        assert_eq!(
            setup.runner.run(&ticket, &setup.reg_add),
            LaunchResult::Declined
        );
        setup
            .runner
            .script(RunnerFault::ExitsWithoutWriting, When::Once);
        setup.registry.set_nv_modes(
            "0001",
            Some(RawValue {
                kind: RawKind::MultiSz,
                strings: vec![String::from("{*}S 640x480x32,64=1;")],
            }),
        );
        assert_eq!(
            setup.runner.run(&ticket, &setup.reg_add),
            LaunchResult::Exited { code: 0 }
        );
        // The registry kept its value, so the re-read shows the mismatch (INJ-E13).
        let key = match &setup.reg_add {
            ElevatedCommand::RegAdd(args) => args.key().clone(),
            ElevatedCommand::RestartDevice(_) => unreachable!("set up as RegAdd above"),
        };
        let now = setup.registry.nv_modes(&key).unwrap().unwrap();
        assert_eq!(now.data.as_str(), "{*}S 640x480x32,64=1;");
        // Without a fault the write lands.
        setup.runner.run(&ticket, &setup.reg_add);
        let now = setup.registry.nv_modes(&key).unwrap().unwrap();
        assert!(now.data.as_str().ends_with("1280x882x8,16,32,64=1F;"));
    }

    #[test]
    #[should_panic(expected = "has no open journal entry")]
    fn req_disp_01_runner_change_needs_ticket() {
        let setup = setup();
        let ticket = setup.journal.open_for_tests("op-1");
        setup.journal.close_for_tests(&ticket);
        let _ = setup.runner.run(&ticket, &setup.restart);
    }
}
