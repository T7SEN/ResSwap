import { invoke, isTauri } from '@tauri-apps/api/core';
import { getCurrentWindow } from '@tauri-apps/api/window';
import { toIpcError } from './errors';
import type { AddCustomModeRequest } from './types/AddCustomModeRequest';
import type { AppState } from './types/AppState';
import type { ApplyOutcome } from './types/ApplyOutcome';
import type { ApplyRequest } from './types/ApplyRequest';
import type { AutoSwitchState } from './types/AutoSwitchState';
import type { ChangeRef } from './types/ChangeRef';
import type { CommandName } from './types/CommandName';
import type { ConfirmOutcome } from './types/ConfirmOutcome';
import type { CustomChangeOutcome } from './types/CustomChangeOutcome';
import type { CustomEntryList } from './types/CustomEntryList';
import type { Diagnostics } from './types/Diagnostics';
import type { ErrorRef } from './types/ErrorRef';
import type { IpcError } from './types/IpcError';
import type { Monitor } from './types/Monitor';
import type { Profile } from './types/Profile';
import type { ProfileRef } from './types/ProfileRef';
import type { RemoveCustomEntryRequest } from './types/RemoveCustomEntryRequest';
import type { RestoreNativeRequest } from './types/RestoreNativeRequest';
import type { RestoreOutcome } from './types/RestoreOutcome';
import type { RevertOutcome } from './types/RevertOutcome';
import type { SetAutostartRequest } from './types/SetAutostartRequest';
import type { SetAutoSwitchRequest } from './types/SetAutoSwitchRequest';
import type { SetHotkeyRequest } from './types/SetHotkeyRequest';
import type { Settings } from './types/Settings';
import type { SettingsInput } from './types/SettingsInput';
import type { ShortcutCreated } from './types/ShortcutCreated';
import type { UndoAllRequest } from './types/UndoAllRequest';
import type { UpdateInfo } from './types/UpdateInfo';

/** What every facade call gives back. The facade never throws. */
export type Result<T> = { ok: true; value: T } | { ok: false; error: IpcError };

type CommandSpec = { request: unknown; response: unknown };

/**
 * Accepts the map only when it holds exactly the names of the Rust CommandName enum: a missing
 * name fails the first part, an extra one the second, so pnpm typecheck catches both.
 */
type EveryCommand<
  T extends Record<CommandName, CommandSpec> & Record<Exclude<keyof T, CommandName>, never>,
> = T;

// A command with no argument takes `null` here and is invoked with no `request` key. A Rust
// `()` answer arrives as null.
type CommandMap = EveryCommand<{
  list_monitors: { request: null; response: Monitor[] };
  get_state: { request: null; response: AppState };
  apply_mode: { request: ApplyRequest; response: ApplyOutcome };
  confirm_mode: { request: ChangeRef; response: ConfirmOutcome };
  revert_mode: { request: ChangeRef; response: RevertOutcome };
  restore_native: { request: RestoreNativeRequest; response: RestoreOutcome };
  restore_everything: { request: null; response: RestoreOutcome };
  list_profiles: { request: null; response: Profile[] };
  save_profile: { request: Profile; response: Profile };
  delete_profile: { request: ProfileRef; response: null };
  set_auto_switch: { request: SetAutoSwitchRequest; response: AutoSwitchState };
  list_custom_entries: { request: null; response: CustomEntryList };
  add_custom_mode: { request: AddCustomModeRequest; response: CustomChangeOutcome };
  remove_custom_entry: { request: RemoveCustomEntryRequest; response: CustomChangeOutcome };
  undo_all_changes: { request: UndoAllRequest; response: CustomChangeOutcome };
  get_settings: { request: null; response: Settings };
  save_settings: { request: SettingsInput; response: Settings };
  set_hotkey: { request: SetHotkeyRequest; response: Settings };
  set_autostart: { request: SetAutostartRequest; response: Settings };
  create_play_shortcut: { request: ProfileRef; response: ShortcutCreated };
  check_for_update: { request: null; response: UpdateInfo | null };
  install_update: { request: null; response: null };
  open_log_folder: { request: null; response: null };
  get_diagnostics: { request: null; response: Diagnostics };
  dismiss_error: { request: ErrorRef; response: null };
}>;

async function call<K extends CommandName>(
  cmd: K,
  request: CommandMap[K]['request'],
): Promise<Result<CommandMap[K]['response']>> {
  try {
    // Every command takes no argument or exactly one named `request`.
    const args = request === null ? undefined : { request };
    const value = await invoke<CommandMap[K]['response']>(cmd, args);
    return { ok: true, value };
  } catch (raw) {
    // Anything that is not an IpcError, such as a missing permission, becomes INTERNAL.
    return { ok: false, error: toIpcError(raw) };
  }
}

/** Reads every monitor again and returns them, hidden ones included. */
export const listMonitors = () => call('list_monitors', null);
/** The current AppState. Called once at start, after the state-changed subscription. */
export const getState = () => call('get_state', null);
/** Applies a mode or a profile through the core's pipeline. */
export const applyMode = (request: ApplyRequest) => call('apply_mode', request);
/** Keeps the pending change. */
export const confirmMode = (request: ChangeRef) => call('confirm_mode', request);
/** Puts back the state from before the pending change. */
export const revertMode = (request: ChangeRef) => call('revert_mode', request);
/** Puts one monitor, or every monitor ResSwap changed, back on its mode from before. */
export const restoreNative = (request: RestoreNativeRequest) => call('restore_native', request);
/** Puts back everything ResSwap changed. */
export const restoreEverything = () => call('restore_everything', null);
/** Every saved profile. */
export const listProfiles = () => call('list_profiles', null);
/** Saves a new or changed profile. */
export const saveProfile = (request: Profile) => call('save_profile', request);
/** Deletes one profile. */
export const deleteProfile = (request: ProfileRef) => call('delete_profile', request);
/** Turns auto-switch on or off. */
export const setAutoSwitch = (request: SetAutoSwitchRequest) => call('set_auto_switch', request);
/** The custom resolution entries the NVIDIA driver holds. */
export const listCustomEntries = () => call('list_custom_entries', null);
/** Adds a custom resolution, or returns the plan that needs the player's approval first. */
export const addCustomMode = (request: AddCustomModeRequest) => call('add_custom_mode', request);
/** Removes one custom resolution entry, or returns the plan that needs approval first. */
export const removeCustomEntry = (request: RemoveCustomEntryRequest) =>
  call('remove_custom_entry', request);
/** Undoes every custom resolution change ResSwap made, or returns the plan first. */
export const undoAllChanges = (request: UndoAllRequest) => call('undo_all_changes', request);
/** The settings the core holds. */
export const getSettings = () => call('get_settings', null);
/** Saves the settings the window may change. */
export const saveSettings = (request: SettingsInput) => call('save_settings', request);
/** Sets or removes a global hotkey. */
export const setHotkey = (request: SetHotkeyRequest) => call('set_hotkey', request);
/** Turns Start with Windows on or off. */
export const setAutostart = (request: SetAutostartRequest) => call('set_autostart', request);
/** Creates a Play shortcut for one profile on the desktop. */
export const createPlayShortcut = (request: ProfileRef) => call('create_play_shortcut', request);
/** Asks for update information; null when this is the newest version. */
export const checkForUpdate = () => call('check_for_update', null);
/** Puts back everything ResSwap changed, then installs the update it found. */
export const installUpdate = () => call('install_update', null);
/** Opens the log folder in Explorer. */
export const openLogFolder = () => call('open_log_folder', null);
/** A read-only summary for bug reports. */
export const getDiagnostics = () => call('get_diagnostics', null);
/** Removes one raised error from the attention list. */
export const dismissError = (request: ErrorRef) => call('dismiss_error', request);

/** True inside ResSwap's own window; false in a normal browser, with or without the mocked core. */
export { isTauri };

/** The two windows ResSwap opens. */
export type WindowLabel = 'main' | 'keep-mode';

/** Reads the window label from Tauri's metadata; no IPC call and no permission. */
export const currentWindowLabel = (): WindowLabel =>
  getCurrentWindow().label === 'keep-mode' ? 'keep-mode' : 'main';
