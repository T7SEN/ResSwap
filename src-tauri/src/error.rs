//! AppError and the IpcError shape the window receives, with stable codes (REQ-UI-08).

#![forbid(unsafe_code)]

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::display::types::{Mode, RefreshRate};

/// Stable strings sent over the IPC. Never renamed; a unit test checks every one.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ErrorCode {
    ModeRejected,
    ModeTestFailed,
    ModeNotOffered,
    RefreshNotAvailable,
    MonitorGone,
    LayoutRestoreFailed,
    NotNvidia,
    NvapiMissing,
    NvapiCallFailed,
    CustomTimingRefused,
    NvModesParseFailed,
    RegistryReadFailed,
    RegistryVerifyMismatch,
    UacDeclined,
    ElevatedToolFailed,
    DriverRestartNeedsReboot,
    DriverRestartRefusedGameRunning,
    ScalingNotApplied,
    JournalWriteFailed,
    RecoveryIncomplete,
    SettingsCorrupt,
    SettingsWriteFailed,
    ProfileInvalid,
    ShortcutFailed,
    HotkeyTaken,
    AutostartFailed,
    UpdateCheckFailed,
    UpdateSignatureInvalid,
    Internal,
}

impl ErrorCode {
    /// Every code, in the order of the contract's table.
    pub const ALL: [Self; 29] = [
        Self::ModeRejected,
        Self::ModeTestFailed,
        Self::ModeNotOffered,
        Self::RefreshNotAvailable,
        Self::MonitorGone,
        Self::LayoutRestoreFailed,
        Self::NotNvidia,
        Self::NvapiMissing,
        Self::NvapiCallFailed,
        Self::CustomTimingRefused,
        Self::NvModesParseFailed,
        Self::RegistryReadFailed,
        Self::RegistryVerifyMismatch,
        Self::UacDeclined,
        Self::ElevatedToolFailed,
        Self::DriverRestartNeedsReboot,
        Self::DriverRestartRefusedGameRunning,
        Self::ScalingNotApplied,
        Self::JournalWriteFailed,
        Self::RecoveryIncomplete,
        Self::SettingsCorrupt,
        Self::SettingsWriteFailed,
        Self::ProfileInvalid,
        Self::ShortcutFailed,
        Self::HotkeyTaken,
        Self::AutostartFailed,
        Self::UpdateCheckFailed,
        Self::UpdateSignatureInvalid,
        Self::Internal,
    ];
}

/// One step of a multi-step operation. The window shows a step as done only when its
/// status is done (REQ-SAFE-17).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum StepKind {
    Test,
    Journal,
    Backup,
    Apply,
    Verify,
    Scaling,
    Layout,
    NvapiTry,
    NvapiSave,
    NvapiDelete,
    RegistryWrite,
    DriverRestart,
    Restore,
    SettingsWrite,
    Download,
    SignatureCheck,
    Install,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum StepStatus {
    Done,
    Failed,
    NotRun,
    Undone,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct StepReport {
    pub step: StepKind,
    pub status: StepStatus,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum JournalItemKind {
    Mode,
    Scaling,
    Layout,
    NvModes,
    NvapiCustomDisplay,
}

/// One thing still changed, as listed in the journal.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct JournalItem {
    pub kind: JournalItemKind,
    pub monitor_name: Option<String>,
    pub mode: Option<Mode>,
}

/// The two Windows tools that may run elevated (D-001).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum ElevatedTool {
    RegExe,
    Pnputil,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum ProfileProblem {
    NotFound,
    InvalidFields,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum ProfileField {
    Name,
    Exe,
    Monitor,
    Mode,
    RestoreDelay,
    TurnOff,
    LaunchPath,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum SettingsSource {
    LastGoodCopy,
    Defaults,
}

/// The values a message needs. A constructor fills only the fields its message uses.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct ErrorContext {
    pub monitor: Option<String>,
    pub mode: Option<Mode>,
    pub current: Option<Mode>,
    pub offered_refresh: Vec<RefreshRate>,
    pub adapter: Option<String>,
    pub nvapi_function: Option<String>,
    pub nvapi_status: Option<i32>,
    pub tool: Option<ElevatedTool>,
    pub exit_code: Option<u32>,
    pub exe: Option<String>,
    pub profile: Option<String>,
    pub profile_problem: Option<ProfileProblem>,
    pub fields: Vec<ProfileField>,
    pub hotkey: Option<String>,
    pub backup_ms: Option<u64>,
    pub settings_source: Option<SettingsSource>,
    pub autostart_enabled: Option<bool>,
    pub still_changed: Vec<JournalItem>,
}

/// What the window receives for every failed command and every raised error.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct IpcError {
    pub code: ErrorCode,
    /// Values the message needs.
    pub context: ErrorContext,
    /// Every step of the operation with its status; empty for single-step commands.
    pub steps: Vec<StepReport>,
    /// Technical detail for the log and the Details fold: the Win32, NVAPI or exit code and
    /// the call that failed. Never shown as the main message.
    pub detail: String,
}

/// The payload every coded variant carries. Boxed inside `AppError`, so a `Result` that
/// carries an `AppError` stays two words wide.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Failure {
    pub context: ErrorContext,
    pub steps: Vec<StepReport>,
    pub detail: String,
}

impl Failure {
    pub fn new(detail: impl Into<String>) -> Self {
        Self {
            context: ErrorContext::default(),
            steps: Vec::new(),
            detail: detail.into(),
        }
    }

    #[must_use]
    pub fn with_context(mut self, context: ErrorContext) -> Self {
        self.context = context;
        self
    }

    #[must_use]
    pub fn with_steps(mut self, steps: Vec<StepReport>) -> Self {
        self.steps = steps;
        self
    }
}

/// The internal error. One variant per `ErrorCode`; over the IPC it is always the
/// `IpcError` shape. There is no blanket `From` for Win32, I/O or JSON errors, so each
/// failure keeps its own code (REQ-UI-09).
#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("Windows did not accept the mode: {}", .0.detail)]
    ModeRejected(Box<Failure>),
    #[error("the mode failed its test: {}", .0.detail)]
    ModeTestFailed(Box<Failure>),
    #[error("Windows does not offer the mode: {}", .0.detail)]
    ModeNotOffered(Box<Failure>),
    #[error("the size is offered, but not at that refresh rate: {}", .0.detail)]
    RefreshNotAvailable(Box<Failure>),
    #[error("the monitor is not connected: {}", .0.detail)]
    MonitorGone(Box<Failure>),
    #[error("the monitor layout was not restored: {}", .0.detail)]
    LayoutRestoreFailed(Box<Failure>),
    #[error("no NVIDIA adapter drives the monitor: {}", .0.detail)]
    NotNvidia(Box<Failure>),
    #[error("the NVIDIA driver interface did not load: {}", .0.detail)]
    NvapiMissing(Box<Failure>),
    #[error("an NVIDIA call failed: {}", .0.detail)]
    NvapiCallFailed(Box<Failure>),
    #[error("the NVIDIA driver refused the custom timing: {}", .0.detail)]
    CustomTimingRefused(Box<Failure>),
    #[error("NV_Modes did not parse: {}", .0.detail)]
    NvModesParseFailed(Box<Failure>),
    #[error("a registry read failed: {}", .0.detail)]
    RegistryReadFailed(Box<Failure>),
    #[error("the registry value read back differs: {}", .0.detail)]
    RegistryVerifyMismatch(Box<Failure>),
    #[error("the administrator prompt was declined: {}", .0.detail)]
    UacDeclined(Box<Failure>),
    #[error("an elevated Windows tool reported an error: {}", .0.detail)]
    ElevatedToolFailed(Box<Failure>),
    #[error("the graphics driver needs a Windows restart: {}", .0.detail)]
    DriverRestartNeedsReboot(Box<Failure>),
    #[error("a driver restart was refused while a watched game runs: {}", .0.detail)]
    DriverRestartRefusedGameRunning(Box<Failure>),
    #[error("full-screen scaling was not applied: {}", .0.detail)]
    ScalingNotApplied(Box<Failure>),
    #[error("the undo record could not be written: {}", .0.detail)]
    JournalWriteFailed(Box<Failure>),
    #[error("putting things back left something changed: {}", .0.detail)]
    RecoveryIncomplete(Box<Failure>),
    #[error("the settings file was damaged: {}", .0.detail)]
    SettingsCorrupt(Box<Failure>),
    #[error("the settings file could not be written: {}", .0.detail)]
    SettingsWriteFailed(Box<Failure>),
    #[error("the profile cannot be used: {}", .0.detail)]
    ProfileInvalid(Box<Failure>),
    #[error("the Play shortcut was not created: {}", .0.detail)]
    ShortcutFailed(Box<Failure>),
    #[error("the hotkey is held by another app: {}", .0.detail)]
    HotkeyTaken(Box<Failure>),
    #[error("Start with Windows was not changed: {}", .0.detail)]
    AutostartFailed(Box<Failure>),
    #[error("the update check failed: {}", .0.detail)]
    UpdateCheckFailed(Box<Failure>),
    #[error("the update failed its signature check: {}", .0.detail)]
    UpdateSignatureInvalid(Box<Failure>),
    #[error("internal error: {detail}")]
    Internal { detail: String },
}

impl AppError {
    /// The one place a variant becomes a wire code. No `_` arm: a new variant does not
    /// compile until it has its code.
    pub fn code(&self) -> ErrorCode {
        match self {
            Self::ModeRejected(_) => ErrorCode::ModeRejected,
            Self::ModeTestFailed(_) => ErrorCode::ModeTestFailed,
            Self::ModeNotOffered(_) => ErrorCode::ModeNotOffered,
            Self::RefreshNotAvailable(_) => ErrorCode::RefreshNotAvailable,
            Self::MonitorGone(_) => ErrorCode::MonitorGone,
            Self::LayoutRestoreFailed(_) => ErrorCode::LayoutRestoreFailed,
            Self::NotNvidia(_) => ErrorCode::NotNvidia,
            Self::NvapiMissing(_) => ErrorCode::NvapiMissing,
            Self::NvapiCallFailed(_) => ErrorCode::NvapiCallFailed,
            Self::CustomTimingRefused(_) => ErrorCode::CustomTimingRefused,
            Self::NvModesParseFailed(_) => ErrorCode::NvModesParseFailed,
            Self::RegistryReadFailed(_) => ErrorCode::RegistryReadFailed,
            Self::RegistryVerifyMismatch(_) => ErrorCode::RegistryVerifyMismatch,
            Self::UacDeclined(_) => ErrorCode::UacDeclined,
            Self::ElevatedToolFailed(_) => ErrorCode::ElevatedToolFailed,
            Self::DriverRestartNeedsReboot(_) => ErrorCode::DriverRestartNeedsReboot,
            Self::DriverRestartRefusedGameRunning(_) => ErrorCode::DriverRestartRefusedGameRunning,
            Self::ScalingNotApplied(_) => ErrorCode::ScalingNotApplied,
            Self::JournalWriteFailed(_) => ErrorCode::JournalWriteFailed,
            Self::RecoveryIncomplete(_) => ErrorCode::RecoveryIncomplete,
            Self::SettingsCorrupt(_) => ErrorCode::SettingsCorrupt,
            Self::SettingsWriteFailed(_) => ErrorCode::SettingsWriteFailed,
            Self::ProfileInvalid(_) => ErrorCode::ProfileInvalid,
            Self::ShortcutFailed(_) => ErrorCode::ShortcutFailed,
            Self::HotkeyTaken(_) => ErrorCode::HotkeyTaken,
            Self::AutostartFailed(_) => ErrorCode::AutostartFailed,
            Self::UpdateCheckFailed(_) => ErrorCode::UpdateCheckFailed,
            Self::UpdateSignatureInvalid(_) => ErrorCode::UpdateSignatureInvalid,
            Self::Internal { .. } => ErrorCode::Internal,
        }
    }

    /// The variant for `code`, carrying `failure`. Used where a code arrives as data, for
    /// example a scripted fault in the fakes. INTERNAL keeps only the detail.
    pub fn with_code(code: ErrorCode, failure: Failure) -> Self {
        let failure = Box::new(failure);
        match code {
            ErrorCode::ModeRejected => Self::ModeRejected(failure),
            ErrorCode::ModeTestFailed => Self::ModeTestFailed(failure),
            ErrorCode::ModeNotOffered => Self::ModeNotOffered(failure),
            ErrorCode::RefreshNotAvailable => Self::RefreshNotAvailable(failure),
            ErrorCode::MonitorGone => Self::MonitorGone(failure),
            ErrorCode::LayoutRestoreFailed => Self::LayoutRestoreFailed(failure),
            ErrorCode::NotNvidia => Self::NotNvidia(failure),
            ErrorCode::NvapiMissing => Self::NvapiMissing(failure),
            ErrorCode::NvapiCallFailed => Self::NvapiCallFailed(failure),
            ErrorCode::CustomTimingRefused => Self::CustomTimingRefused(failure),
            ErrorCode::NvModesParseFailed => Self::NvModesParseFailed(failure),
            ErrorCode::RegistryReadFailed => Self::RegistryReadFailed(failure),
            ErrorCode::RegistryVerifyMismatch => Self::RegistryVerifyMismatch(failure),
            ErrorCode::UacDeclined => Self::UacDeclined(failure),
            ErrorCode::ElevatedToolFailed => Self::ElevatedToolFailed(failure),
            ErrorCode::DriverRestartNeedsReboot => Self::DriverRestartNeedsReboot(failure),
            ErrorCode::DriverRestartRefusedGameRunning => {
                Self::DriverRestartRefusedGameRunning(failure)
            }
            ErrorCode::ScalingNotApplied => Self::ScalingNotApplied(failure),
            ErrorCode::JournalWriteFailed => Self::JournalWriteFailed(failure),
            ErrorCode::RecoveryIncomplete => Self::RecoveryIncomplete(failure),
            ErrorCode::SettingsCorrupt => Self::SettingsCorrupt(failure),
            ErrorCode::SettingsWriteFailed => Self::SettingsWriteFailed(failure),
            ErrorCode::ProfileInvalid => Self::ProfileInvalid(failure),
            ErrorCode::ShortcutFailed => Self::ShortcutFailed(failure),
            ErrorCode::HotkeyTaken => Self::HotkeyTaken(failure),
            ErrorCode::AutostartFailed => Self::AutostartFailed(failure),
            ErrorCode::UpdateCheckFailed => Self::UpdateCheckFailed(failure),
            ErrorCode::UpdateSignatureInvalid => Self::UpdateSignatureInvalid(failure),
            ErrorCode::Internal => Self::Internal {
                detail: failure.detail,
            },
        }
    }

    pub fn internal(detail: impl Into<String>) -> Self {
        Self::Internal {
            detail: detail.into(),
        }
    }

    /// The payload of a coded variant; `None` for INTERNAL.
    fn failure(&self) -> Option<&Failure> {
        match self {
            Self::ModeRejected(failure)
            | Self::ModeTestFailed(failure)
            | Self::ModeNotOffered(failure)
            | Self::RefreshNotAvailable(failure)
            | Self::MonitorGone(failure)
            | Self::LayoutRestoreFailed(failure)
            | Self::NotNvidia(failure)
            | Self::NvapiMissing(failure)
            | Self::NvapiCallFailed(failure)
            | Self::CustomTimingRefused(failure)
            | Self::NvModesParseFailed(failure)
            | Self::RegistryReadFailed(failure)
            | Self::RegistryVerifyMismatch(failure)
            | Self::UacDeclined(failure)
            | Self::ElevatedToolFailed(failure)
            | Self::DriverRestartNeedsReboot(failure)
            | Self::DriverRestartRefusedGameRunning(failure)
            | Self::ScalingNotApplied(failure)
            | Self::JournalWriteFailed(failure)
            | Self::RecoveryIncomplete(failure)
            | Self::SettingsCorrupt(failure)
            | Self::SettingsWriteFailed(failure)
            | Self::ProfileInvalid(failure)
            | Self::ShortcutFailed(failure)
            | Self::HotkeyTaken(failure)
            | Self::AutostartFailed(failure)
            | Self::UpdateCheckFailed(failure)
            | Self::UpdateSignatureInvalid(failure) => Some(failure),
            Self::Internal { .. } => None,
        }
    }

    /// The shape the window receives.
    pub fn to_ipc(&self) -> IpcError {
        let failure = self.failure();
        IpcError {
            code: self.code(),
            context: failure.map(|f| f.context.clone()).unwrap_or_default(),
            steps: failure.map(|f| f.steps.clone()).unwrap_or_default(),
            detail: error_chain(self),
        }
    }
}

/// Commands return `Result<T, AppError>`; the window receives the `IpcError` shape.
impl Serialize for AppError {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.to_ipc().serialize(serializer)
    }
}

/// The Display text and every source after it, joined with ": ".
fn error_chain(error: &dyn std::error::Error) -> String {
    let mut text = error.to_string();
    let mut source = error.source();
    while let Some(cause) = source {
        text.push_str(": ");
        text.push_str(&cause.to_string());
        source = cause.source();
    }
    text
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;

    /// The contract's strings. An exhaustive match, so a new code fails to compile here
    /// until its string is listed.
    fn expected(code: ErrorCode) -> &'static str {
        match code {
            ErrorCode::ModeRejected => "MODE_REJECTED",
            ErrorCode::ModeTestFailed => "MODE_TEST_FAILED",
            ErrorCode::ModeNotOffered => "MODE_NOT_OFFERED",
            ErrorCode::RefreshNotAvailable => "REFRESH_NOT_AVAILABLE",
            ErrorCode::MonitorGone => "MONITOR_GONE",
            ErrorCode::LayoutRestoreFailed => "LAYOUT_RESTORE_FAILED",
            ErrorCode::NotNvidia => "NOT_NVIDIA",
            ErrorCode::NvapiMissing => "NVAPI_MISSING",
            ErrorCode::NvapiCallFailed => "NVAPI_CALL_FAILED",
            ErrorCode::CustomTimingRefused => "CUSTOM_TIMING_REFUSED",
            ErrorCode::NvModesParseFailed => "NV_MODES_PARSE_FAILED",
            ErrorCode::RegistryReadFailed => "REGISTRY_READ_FAILED",
            ErrorCode::RegistryVerifyMismatch => "REGISTRY_VERIFY_MISMATCH",
            ErrorCode::UacDeclined => "UAC_DECLINED",
            ErrorCode::ElevatedToolFailed => "ELEVATED_TOOL_FAILED",
            ErrorCode::DriverRestartNeedsReboot => "DRIVER_RESTART_NEEDS_REBOOT",
            ErrorCode::DriverRestartRefusedGameRunning => "DRIVER_RESTART_REFUSED_GAME_RUNNING",
            ErrorCode::ScalingNotApplied => "SCALING_NOT_APPLIED",
            ErrorCode::JournalWriteFailed => "JOURNAL_WRITE_FAILED",
            ErrorCode::RecoveryIncomplete => "RECOVERY_INCOMPLETE",
            ErrorCode::SettingsCorrupt => "SETTINGS_CORRUPT",
            ErrorCode::SettingsWriteFailed => "SETTINGS_WRITE_FAILED",
            ErrorCode::ProfileInvalid => "PROFILE_INVALID",
            ErrorCode::ShortcutFailed => "SHORTCUT_FAILED",
            ErrorCode::HotkeyTaken => "HOTKEY_TAKEN",
            ErrorCode::AutostartFailed => "AUTOSTART_FAILED",
            ErrorCode::UpdateCheckFailed => "UPDATE_CHECK_FAILED",
            ErrorCode::UpdateSignatureInvalid => "UPDATE_SIGNATURE_INVALID",
            ErrorCode::Internal => "INTERNAL",
        }
    }

    #[test]
    fn req_ui_08_error_codes_stable() {
        let mut seen = BTreeSet::new();
        for code in ErrorCode::ALL {
            let wire = serde_json::to_value(code).unwrap();
            assert_eq!(wire, serde_json::Value::from(expected(code)), "{code:?}");
            let back: ErrorCode = serde_json::from_value(wire).unwrap();
            assert_eq!(back, code);
            assert!(seen.insert(expected(code)), "{code:?} is listed twice");
        }
        assert_eq!(seen.len(), 29);
    }

    #[test]
    fn req_ui_08_every_code_has_its_own_variant() {
        let mut variants = std::collections::HashSet::new();
        for code in ErrorCode::ALL {
            let error = AppError::with_code(code, Failure::new("scripted"));
            assert_eq!(error.code(), code);
            variants.insert(std::mem::discriminant(&error));
        }
        assert_eq!(variants.len(), 29);
    }

    #[test]
    fn req_ui_09_errors_serialise_as_ipc_error() {
        let context = ErrorContext {
            monitor: Some(String::from("LG ULTRAGEAR")),
            exit_code: Some(5),
            ..ErrorContext::default()
        };
        let steps = vec![StepReport {
            step: StepKind::Test,
            status: StepStatus::Done,
        }];
        let error = AppError::ElevatedToolFailed(Box::new(
            Failure::new("reg.exe exited with 5")
                .with_context(context)
                .with_steps(steps),
        ));
        let json = serde_json::to_value(&error).unwrap();
        assert_eq!(json["code"], "ELEVATED_TOOL_FAILED");
        assert_eq!(json["context"]["monitor"], "LG ULTRAGEAR");
        assert_eq!(json["context"]["exitCode"], 5);
        // Option fields are always present, as null (the window sees T | null).
        assert!(json["context"]["nvapiStatus"].is_null());
        assert!(
            json["context"]
                .as_object()
                .unwrap()
                .contains_key("nvapiStatus")
        );
        assert_eq!(json["steps"][0]["step"], "test");
        assert_eq!(json["steps"][0]["status"], "done");
        assert_eq!(
            json["detail"],
            "an elevated Windows tool reported an error: reg.exe exited with 5"
        );
        let back: IpcError = serde_json::from_value(json).unwrap();
        assert_eq!(back.code, ErrorCode::ElevatedToolFailed);
    }

    #[test]
    fn req_ui_09_internal_keeps_its_detail_only() {
        let json = serde_json::to_value(AppError::internal("enumeration failed")).unwrap();
        assert_eq!(json["code"], "INTERNAL");
        assert_eq!(json["detail"], "internal error: enumeration failed");
        assert_eq!(json["steps"], serde_json::json!([]));
        assert_eq!(
            serde_json::from_value::<ErrorContext>(json["context"].clone()).unwrap(),
            ErrorContext::default()
        );
    }
}
