//! The only code that starts a program with the runas verb: reg.exe for the NV_Modes write
//! and pnputil.exe /restart-device for the driver restart, both from System32, through
//! ShellExecuteExW, after the in-app explanation (REQ-SAFE-10, REQ-SAFE-11, D-001).
//! ResSwap's own code never runs elevated. This build holds the typed commands, their
//! argument builders, the result mapping and the runner trait; it starts nothing.

pub mod fake;
pub mod pnputil;
pub mod reg_tool;

use std::path::{Component, Path, PathBuf, Prefix};

use crate::apply::Ticket;
use crate::error::{AppError, ElevatedTool, ErrorContext, Failure};

pub use pnputil::RestartDeviceArgs;
pub use reg_tool::RegAddArgs;

/// The only things ResSwap ever starts with the runas verb. Each variant is built only by
/// its builder from validated values, so no raw path or argument text can reach a launch.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ElevatedCommand {
    /// reg.exe add for NV_Modes (reg_tool.rs).
    RegAdd(RegAddArgs),
    /// pnputil.exe /restart-device (pnputil.rs).
    RestartDevice(RestartDeviceArgs),
}

impl ElevatedCommand {
    /// The Windows tool this command runs.
    pub const fn tool(&self) -> ElevatedTool {
        match self {
            Self::RegAdd(_) => ElevatedTool::RegExe,
            Self::RestartDevice(_) => ElevatedTool::Pnputil,
        }
    }

    /// The tool's full path inside `system` and its parameter string, ready for
    /// SHELLEXECUTEINFOW: lpFile and lpParameters.
    pub fn file_and_parameters(&self, system: &SystemDirectory) -> (PathBuf, String) {
        let tokens = match self {
            Self::RegAdd(args) => args.tokens(),
            Self::RestartDevice(args) => args.tokens(),
        };
        (system.tool_path(self.tool()), join_parameters(&tokens))
    }
}

/// The file name of each tool inside System32.
pub const fn file_name(tool: ElevatedTool) -> &'static str {
    match tool {
        ElevatedTool::RegExe => "reg.exe",
        ElevatedTool::Pnputil => "pnputil.exe",
    }
}

/// One token of a tool's parameters.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Token {
    /// Text the builder writes itself: a verb or a switch.
    Fixed(&'static str),
    /// A value from a validated newtype.
    Value(String),
}

/// The one function that assembles lpParameters. Tokens are separated by one space; a token
/// is wrapped in double quotes only when it holds a space. No token holds a double quote,
/// because every value passed an allow-list without one, so nothing is ever escaped.
pub fn join_parameters(tokens: &[Token]) -> String {
    let mut parameters = String::new();
    for token in tokens {
        let text = match token {
            Token::Fixed(text) => *text,
            Token::Value(text) => text.as_str(),
        };
        if !parameters.is_empty() {
            parameters.push(' ');
        }
        if text.contains(' ') {
            parameters.push('"');
            parameters.push_str(text);
            parameters.push('"');
        } else {
            parameters.push_str(text);
        }
    }
    parameters
}

/// The System32 folder as GetSystemDirectoryW reports it: an absolute path on a drive
/// letter whose last part is System32. Never built from an environment variable, settings,
/// IPC or the journal, so a tool always starts from the Windows folder and never from a
/// folder the player can write.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SystemDirectory(PathBuf);

impl SystemDirectory {
    /// `None` unless `path` is absolute, starts with a drive letter, holds no `.` or `..`
    /// part and ends in System32.
    pub(in crate::elevation) fn checked(path: PathBuf) -> Option<Self> {
        let mut components = path.components();
        let on_drive = matches!(
            components.next(),
            Some(Component::Prefix(prefix)) if matches!(prefix.kind(), Prefix::Disk(_))
        );
        let plain = path
            .components()
            .all(|part| !matches!(part, Component::CurDir | Component::ParentDir));
        let last_is_system32 = path
            .file_name()
            .is_some_and(|name| name.eq_ignore_ascii_case("system32"));
        (path.is_absolute() && on_drive && plain && last_is_system32).then_some(Self(path))
    }

    /// The folder itself, also the tool's working folder (lpDirectory).
    pub fn as_path(&self) -> &Path {
        &self.0
    }

    /// The full path of `tool` in this folder.
    pub fn tool_path(&self, tool: ElevatedTool) -> PathBuf {
        self.0.join(file_name(tool))
    }
}

/// What one launch did, as plain data, so the mapping to an error is a pure function.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LaunchResult {
    /// ShellExecuteExW returned FALSE and GetLastError gave 1223 (ERROR_CANCELLED).
    Declined,
    /// ShellExecuteExW returned FALSE with any other error, or the launch could not be
    /// prepared.
    LaunchFailed { win32: u32 },
    /// ShellExecuteExW returned TRUE with no process handle.
    NoProcess,
    /// The wait limit passed or the wait failed; the tool is never terminated.
    TimedOut,
    /// The tool exited with this code.
    Exited { code: u32 },
}

/// Runs one elevated command after its journal status is on disk: one prompt, one
/// operation. The Ticket keeps it behind the journal write (REQ-SAFE-01).
pub trait ElevatedToolRunner: Send + Sync {
    /// Starts the tool, waits for it and reports what happened. Changes the PC.
    fn run(&self, ticket: &Ticket, command: &ElevatedCommand) -> LaunchResult;
}

/// pnputil's exit code for "a Windows restart is needed". None is documented, so no exit
/// code maps to DRIVER_RESTART_NEEDS_REBOOT until the NVIDIA work records one.
pub const PNPUTIL_REBOOT_NEEDED: Option<u32> = None;

/// From a launch result to the error the player sees (REQ-SAFE-13, REQ-SAFE-14). A declined
/// prompt is UAC_DECLINED and never ELEVATED_TOOL_FAILED. `Ok(())` is not success yet: the
/// caller re-reads the system before any step shows as done (REQ-SAFE-17).
pub fn map_result(tool: ElevatedTool, result: LaunchResult) -> Result<(), AppError> {
    let name = file_name(tool);
    let context = |exit_code: Option<u32>| ErrorContext {
        tool: Some(tool),
        exit_code,
        ..ErrorContext::default()
    };
    match result {
        LaunchResult::Exited { code: 0 } => Ok(()),
        LaunchResult::Declined => Err(AppError::UacDeclined(Box::new(
            Failure::new(format!("the administrator prompt for {name} was declined"))
                .with_context(context(None)),
        ))),
        LaunchResult::Exited { code }
            if tool == ElevatedTool::Pnputil && Some(code) == PNPUTIL_REBOOT_NEEDED =>
        {
            Err(AppError::DriverRestartNeedsReboot(Box::new(
                Failure::new(format!("{name} exited with {code}"))
                    .with_context(context(Some(code))),
            )))
        }
        LaunchResult::Exited { code } => Err(AppError::ElevatedToolFailed(Box::new(
            Failure::new(format!("{name} exited with {code}")).with_context(context(Some(code))),
        ))),
        LaunchResult::LaunchFailed { win32 } => Err(AppError::ElevatedToolFailed(Box::new(
            Failure::new(format!("{name} did not start: Win32 error {win32}"))
                .with_context(context(None)),
        ))),
        LaunchResult::NoProcess => Err(AppError::ElevatedToolFailed(Box::new(
            Failure::new(format!("{name} started with no process handle"))
                .with_context(context(None)),
        ))),
        LaunchResult::TimedOut => Err(AppError::ElevatedToolFailed(Box::new(
            Failure::new(format!("{name} did not finish within the wait limit"))
                .with_context(context(None)),
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::ErrorCode;

    fn mapped(tool: ElevatedTool, result: LaunchResult) -> Option<(ErrorCode, Option<u32>)> {
        map_result(tool, result).err().map(|error| {
            let ipc = error.to_ipc();
            assert_eq!(ipc.context.tool, Some(tool));
            (ipc.code, ipc.context.exit_code)
        })
    }

    #[test]
    fn req_safe_14_exit_zero_needs_the_reread() {
        assert_eq!(
            mapped(ElevatedTool::RegExe, LaunchResult::Exited { code: 0 }),
            None
        );
    }

    #[test]
    fn req_safe_13_inj_e08_declined_prompt_is_its_own_code() {
        assert_eq!(
            mapped(ElevatedTool::RegExe, LaunchResult::Declined),
            Some((ErrorCode::UacDeclined, None))
        );
        assert_eq!(
            mapped(ElevatedTool::Pnputil, LaunchResult::Declined),
            Some((ErrorCode::UacDeclined, None))
        );
    }

    #[test]
    fn req_safe_14_inj_e09_failures_are_elevated_tool_failed() {
        let failed = Some((ErrorCode::ElevatedToolFailed, None));
        // Also INJ-E10, INJ-E11.
        for result in [
            LaunchResult::LaunchFailed { win32: 2 },
            LaunchResult::NoProcess,
            LaunchResult::TimedOut,
        ] {
            assert_eq!(mapped(ElevatedTool::RegExe, result), failed, "{result:?}");
        }
        // Also INJ-E12. 259 (STILL_ACTIVE) after a signalled wait is never a result.
        for code in [1, 5, 259] {
            assert_eq!(
                mapped(ElevatedTool::RegExe, LaunchResult::Exited { code }),
                Some((ErrorCode::ElevatedToolFailed, Some(code)))
            );
        }
    }

    #[test]
    fn req_nv_21_no_exit_code_maps_to_reboot_needed_yet() {
        assert_eq!(PNPUTIL_REBOOT_NEEDED, None);
        assert_eq!(
            mapped(ElevatedTool::Pnputil, LaunchResult::Exited { code: 1 }),
            Some((ErrorCode::ElevatedToolFailed, Some(1)))
        );
    }

    #[test]
    fn req_safe_11_system_directory_is_an_absolute_system32() {
        let system = SystemDirectory::checked(PathBuf::from(r"C:\Windows\System32")).unwrap();
        assert_eq!(
            system.tool_path(ElevatedTool::RegExe),
            PathBuf::from(r"C:\Windows\System32\reg.exe")
        );
        assert_eq!(
            system.tool_path(ElevatedTool::Pnputil),
            PathBuf::from(r"C:\Windows\System32\pnputil.exe")
        );
        assert!(SystemDirectory::checked(PathBuf::from(r"C:\WINDOWS\system32")).is_some());
        for refused in [
            r"System32",
            r"Windows\System32",
            r"\Windows\System32",
            r"C:Windows\System32",
            r"\\server\share\System32",
            r"\\?\C:\Windows\System32",
            r"C:\Windows\System32\..\System32",
            r"C:\Windows\SysWOW64",
            "",
        ] {
            assert!(
                SystemDirectory::checked(PathBuf::from(refused)).is_none(),
                "{refused}"
            );
        }
    }

    #[test]
    fn req_sec_01_parameters_quote_only_tokens_with_a_space() {
        let tokens = [
            Token::Fixed("add"),
            Token::Value(String::from(r"HKLM\SYSTEM\Key")),
            Token::Fixed("/d"),
            Token::Value(String::from("{*}S 640x480x32=1;")),
        ];
        assert_eq!(
            join_parameters(&tokens),
            r#"add HKLM\SYSTEM\Key /d "{*}S 640x480x32=1;""#
        );
    }
}
