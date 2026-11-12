//! The commands the window calls. Each one checks nothing itself and forwards to `Core` with
//! `ChangeSource::Window`, inside a task, so a panic reaches the window as INTERNAL.

#![forbid(unsafe_code)]

use serde::{Deserialize, Serialize};
use tauri::{Runtime, State};
use ts_rs::TS;

use crate::Core;
use crate::display::types::Monitor;
use crate::error::{AppError, ErrorCode};
use crate::ipc::types::{
    AddCustomModeRequest, AppState, ApplyOutcome, ApplyRequest, AutoSwitchState, ChangeRef,
    ChangeSource, ConfirmOutcome, CustomChangeOutcome, Diagnostics, ErrorRef, ProfileRef,
    RemoveCustomEntryRequest, RestoreNativeRequest, RestoreOutcome, RevertOutcome, RevertReason,
    SetAutoSwitchRequest, SetAutostartRequest, SetHotkeyRequest, ShortcutCreated, UndoAllRequest,
    UpdateInfo,
};
use crate::profiles::Profile;
use crate::settings::{Settings, SettingsInput};
use crate::vendor::CustomEntryList;

/// Every command name, as build.rs lists it and the window invokes it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum CommandName {
    ListMonitors,
    GetState,
    ApplyMode,
    ConfirmMode,
    RevertMode,
    RestoreNative,
    RestoreEverything,
    ListProfiles,
    SaveProfile,
    DeleteProfile,
    SetAutoSwitch,
    ListCustomEntries,
    AddCustomMode,
    RemoveCustomEntry,
    UndoAllChanges,
    GetSettings,
    SaveSettings,
    SetHotkey,
    SetAutostart,
    CreatePlayShortcut,
    CheckForUpdate,
    InstallUpdate,
    OpenLogFolder,
    GetDiagnostics,
    DismissError,
}

impl CommandName {
    pub const ALL: [Self; 25] = [
        Self::ListMonitors,
        Self::GetState,
        Self::ApplyMode,
        Self::ConfirmMode,
        Self::RevertMode,
        Self::RestoreNative,
        Self::RestoreEverything,
        Self::ListProfiles,
        Self::SaveProfile,
        Self::DeleteProfile,
        Self::SetAutoSwitch,
        Self::ListCustomEntries,
        Self::AddCustomMode,
        Self::RemoveCustomEntry,
        Self::UndoAllChanges,
        Self::GetSettings,
        Self::SaveSettings,
        Self::SetHotkey,
        Self::SetAutostart,
        Self::CreatePlayShortcut,
        Self::CheckForUpdate,
        Self::InstallUpdate,
        Self::OpenLogFolder,
        Self::GetDiagnostics,
        Self::DismissError,
    ];

    /// The name the window invokes; the same string serde writes.
    pub const fn name(self) -> &'static str {
        match self {
            Self::ListMonitors => "list_monitors",
            Self::GetState => "get_state",
            Self::ApplyMode => "apply_mode",
            Self::ConfirmMode => "confirm_mode",
            Self::RevertMode => "revert_mode",
            Self::RestoreNative => "restore_native",
            Self::RestoreEverything => "restore_everything",
            Self::ListProfiles => "list_profiles",
            Self::SaveProfile => "save_profile",
            Self::DeleteProfile => "delete_profile",
            Self::SetAutoSwitch => "set_auto_switch",
            Self::ListCustomEntries => "list_custom_entries",
            Self::AddCustomMode => "add_custom_mode",
            Self::RemoveCustomEntry => "remove_custom_entry",
            Self::UndoAllChanges => "undo_all_changes",
            Self::GetSettings => "get_settings",
            Self::SaveSettings => "save_settings",
            Self::SetHotkey => "set_hotkey",
            Self::SetAutostart => "set_autostart",
            Self::CreatePlayShortcut => "create_play_shortcut",
            Self::CheckForUpdate => "check_for_update",
            Self::InstallUpdate => "install_update",
            Self::OpenLogFolder => "open_log_folder",
            Self::GetDiagnostics => "get_diagnostics",
            Self::DismissError => "dismiss_error",
        }
    }

    /// The capability permission that lets a window call this command.
    pub fn permission(self) -> String {
        format!("allow-{}", self.name().replace('_', "-"))
    }
}

/// Whole-command faults for a debug run on the fakes: each listed command answers its code
/// on every call, the same grammar as the mocked core's `fail` parameter. Only the fakes
/// carry them, and a release build never reads them from the environment (REQ-SEC-05).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CommandFaults {
    faults: Vec<(CommandName, ErrorCode)>,
    refused: Vec<String>,
}

impl CommandFaults {
    /// Parses `<command>:<CODE>` entries separated by commas, with the wire names of the
    /// command and the code. An entry that names no command or no code goes to `refused`,
    /// so start-up can report it instead of ignoring a typo.
    pub fn parse(text: &str) -> Self {
        let mut parsed = Self::default();
        for entry in text
            .split(',')
            .map(str::trim)
            .filter(|entry| !entry.is_empty())
        {
            let fault = entry.split_once(':').and_then(|(command, code)| {
                let command: CommandName = serde_json::from_value(command.trim().into()).ok()?;
                let code: ErrorCode = serde_json::from_value(code.trim().into()).ok()?;
                Some((command, code))
            });
            match fault {
                Some(fault) => parsed.faults.push(fault),
                None => parsed.refused.push(entry.to_owned()),
            }
        }
        parsed
    }

    /// The code `command` answers with, if a fault names it. The first entry for a command
    /// wins.
    pub fn code_for(&self, command: CommandName) -> Option<ErrorCode> {
        self.faults
            .iter()
            .find(|(named, _)| *named == command)
            .map(|(_, code)| *code)
    }

    /// The entries `parse` could not read.
    pub fn refused(&self) -> &[String] {
        &self.refused
    }
}

/// Runs a command's work as its own task, so a panic inside it becomes INTERNAL. Without
/// it, a panicking async command drops its responder and the window's promise never settles.
async fn guarded<T, F>(work: F) -> Result<T, AppError>
where
    F: Future<Output = Result<T, AppError>> + Send + 'static,
    T: Send + 'static,
{
    match tauri::async_runtime::spawn(work).await {
        Ok(result) => result,
        Err(join_error) => {
            log::error!("a command task ended without a result: {join_error}");
            Err(AppError::internal("a command stopped before it finished"))
        }
    }
}

#[tauri::command]
pub async fn list_monitors(core: State<'_, Core>) -> Result<Vec<Monitor>, AppError> {
    let core = core.inner().clone();
    guarded(async move { core.list_monitors().await }).await
}

#[tauri::command]
pub async fn get_state(core: State<'_, Core>) -> Result<AppState, AppError> {
    let core = core.inner().clone();
    guarded(async move { core.state().await }).await
}

#[tauri::command]
pub async fn apply_mode(
    core: State<'_, Core>,
    request: ApplyRequest,
) -> Result<ApplyOutcome, AppError> {
    let core = core.inner().clone();
    guarded(async move { core.apply(request, ChangeSource::Window).await }).await
}

#[tauri::command]
pub async fn confirm_mode(
    core: State<'_, Core>,
    request: ChangeRef,
) -> Result<ConfirmOutcome, AppError> {
    let core = core.inner().clone();
    guarded(async move { core.confirm(request, ChangeSource::Window).await }).await
}

#[tauri::command]
pub async fn revert_mode(
    core: State<'_, Core>,
    request: ChangeRef,
) -> Result<RevertOutcome, AppError> {
    let core = core.inner().clone();
    guarded(async move { core.revert(request, RevertReason::Player).await }).await
}

#[tauri::command]
pub async fn restore_native(
    core: State<'_, Core>,
    request: RestoreNativeRequest,
) -> Result<RestoreOutcome, AppError> {
    let core = core.inner().clone();
    guarded(async move { core.restore_native(request, ChangeSource::Window).await }).await
}

#[tauri::command]
pub async fn restore_everything(core: State<'_, Core>) -> Result<RestoreOutcome, AppError> {
    let core = core.inner().clone();
    guarded(async move { core.restore_everything(ChangeSource::Window).await }).await
}

#[tauri::command]
pub async fn list_profiles(core: State<'_, Core>) -> Result<Vec<Profile>, AppError> {
    let core = core.inner().clone();
    guarded(async move { core.profiles().await }).await
}

#[tauri::command]
pub async fn save_profile(core: State<'_, Core>, request: Profile) -> Result<Profile, AppError> {
    let core = core.inner().clone();
    guarded(async move { core.save_profile(request).await }).await
}

#[tauri::command]
pub async fn delete_profile(core: State<'_, Core>, request: ProfileRef) -> Result<(), AppError> {
    let core = core.inner().clone();
    guarded(async move { core.delete_profile(request).await }).await
}

#[tauri::command]
pub async fn set_auto_switch(
    core: State<'_, Core>,
    request: SetAutoSwitchRequest,
) -> Result<AutoSwitchState, AppError> {
    let core = core.inner().clone();
    guarded(async move { core.set_auto_switch(request).await }).await
}

#[tauri::command]
pub async fn list_custom_entries(core: State<'_, Core>) -> Result<CustomEntryList, AppError> {
    let core = core.inner().clone();
    guarded(async move { core.custom_entries().await }).await
}

#[tauri::command]
pub async fn add_custom_mode(
    core: State<'_, Core>,
    request: AddCustomModeRequest,
) -> Result<CustomChangeOutcome, AppError> {
    let core = core.inner().clone();
    guarded(async move { core.add_custom_mode(request, ChangeSource::Window).await }).await
}

#[tauri::command]
pub async fn remove_custom_entry(
    core: State<'_, Core>,
    request: RemoveCustomEntryRequest,
) -> Result<CustomChangeOutcome, AppError> {
    let core = core.inner().clone();
    guarded(async move {
        core.remove_custom_entry(request, ChangeSource::Window)
            .await
    })
    .await
}

#[tauri::command]
pub async fn undo_all_changes(
    core: State<'_, Core>,
    request: UndoAllRequest,
) -> Result<CustomChangeOutcome, AppError> {
    let core = core.inner().clone();
    guarded(async move { core.undo_all_changes(request, ChangeSource::Window).await }).await
}

#[tauri::command]
pub async fn get_settings(core: State<'_, Core>) -> Result<Settings, AppError> {
    let core = core.inner().clone();
    guarded(async move { core.settings().await }).await
}

#[tauri::command]
pub async fn save_settings(
    core: State<'_, Core>,
    request: SettingsInput,
) -> Result<Settings, AppError> {
    let core = core.inner().clone();
    guarded(async move { core.save_settings(request).await }).await
}

#[tauri::command]
pub async fn set_hotkey(
    core: State<'_, Core>,
    request: SetHotkeyRequest,
) -> Result<Settings, AppError> {
    let core = core.inner().clone();
    guarded(async move { core.set_hotkey(request).await }).await
}

#[tauri::command]
pub async fn set_autostart(
    core: State<'_, Core>,
    request: SetAutostartRequest,
) -> Result<Settings, AppError> {
    let core = core.inner().clone();
    guarded(async move { core.set_autostart(request).await }).await
}

#[tauri::command]
pub async fn create_play_shortcut(
    core: State<'_, Core>,
    request: ProfileRef,
) -> Result<ShortcutCreated, AppError> {
    let core = core.inner().clone();
    guarded(async move { core.create_play_shortcut(request).await }).await
}

#[tauri::command]
pub async fn check_for_update(core: State<'_, Core>) -> Result<Option<UpdateInfo>, AppError> {
    let core = core.inner().clone();
    guarded(async move { core.check_for_update().await }).await
}

#[tauri::command]
pub async fn install_update(core: State<'_, Core>) -> Result<(), AppError> {
    let core = core.inner().clone();
    guarded(async move { core.install_update(ChangeSource::Window).await }).await
}

#[tauri::command]
pub async fn open_log_folder(core: State<'_, Core>) -> Result<(), AppError> {
    let core = core.inner().clone();
    guarded(async move { core.open_log_folder().await }).await
}

#[tauri::command]
pub async fn get_diagnostics(core: State<'_, Core>) -> Result<Diagnostics, AppError> {
    let core = core.inner().clone();
    guarded(async move { core.diagnostics().await }).await
}

#[tauri::command]
pub async fn dismiss_error(core: State<'_, Core>, request: ErrorRef) -> Result<(), AppError> {
    let core = core.inner().clone();
    guarded(async move { core.dismiss_error(request).await }).await
}

/// The invoke handler with every command, in the order of `CommandName::ALL`.
pub fn handler<R: Runtime>() -> impl Fn(tauri::ipc::Invoke<R>) -> bool + Send + Sync + 'static {
    tauri::generate_handler![
        list_monitors,
        get_state,
        apply_mode,
        confirm_mode,
        revert_mode,
        restore_native,
        restore_everything,
        list_profiles,
        save_profile,
        delete_profile,
        set_auto_switch,
        list_custom_entries,
        add_custom_mode,
        remove_custom_entry,
        undo_all_changes,
        get_settings,
        save_settings,
        set_hotkey,
        set_autostart,
        create_play_shortcut,
        check_for_update,
        install_update,
        open_log_folder,
        get_diagnostics,
        dismiss_error,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The quoted names inside the COMMANDS list of build.rs.
    fn build_rs_commands() -> Vec<String> {
        let build_rs = include_str!("../../build.rs");
        let start = build_rs
            .find("const COMMANDS")
            .expect("build.rs lists COMMANDS");
        let list = &build_rs[start..];
        let end = list.find("];").expect("the COMMANDS list ends");
        list[..end]
            .split('"')
            .skip(1)
            .step_by(2)
            .map(str::to_owned)
            .collect()
    }

    #[test]
    fn req_sec_07_command_names_match() {
        let names: Vec<String> = CommandName::ALL
            .iter()
            .map(|c| c.name().to_owned())
            .collect();
        assert_eq!(names, build_rs_commands());
        for command in CommandName::ALL {
            assert_eq!(
                serde_json::to_value(command).unwrap(),
                serde_json::Value::from(command.name())
            );
        }
        assert_eq!(
            CommandName::ListCustomEntries.permission(),
            "allow-list-custom-entries"
        );
    }

    #[test]
    fn req_ui_08_command_faults_parse_wire_names() {
        let faults = CommandFaults::parse(
            " apply_mode:MODE_REJECTED, set_hotkey : HOTKEY_TAKEN,,apply_mode:INTERNAL",
        );
        assert_eq!(
            faults.code_for(CommandName::ApplyMode),
            Some(ErrorCode::ModeRejected)
        );
        assert_eq!(
            faults.code_for(CommandName::SetHotkey),
            Some(ErrorCode::HotkeyTaken)
        );
        assert_eq!(faults.code_for(CommandName::GetState), None);
        assert!(faults.refused().is_empty());

        let typos = CommandFaults::parse("apply_mode:mode_rejected,applyMode:INTERNAL,apply_mode");
        assert_eq!(faults.code_for(CommandName::RestoreNative), None);
        assert_eq!(typos.code_for(CommandName::ApplyMode), None);
        assert_eq!(
            typos.refused(),
            [
                "apply_mode:mode_rejected",
                "applyMode:INTERNAL",
                "apply_mode"
            ]
        );
        assert_eq!(CommandFaults::parse(""), CommandFaults::default());
    }
}
