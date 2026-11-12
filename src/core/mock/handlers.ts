import type { CommandName } from '../types/CommandName';
import type { Answer, MockCore } from './core';

type Handler = (core: MockCore, request: unknown) => Answer;

/**
 * One handler per command. A missing or extra name fails pnpm typecheck. Each one answers as
 * the core of this build does: the commands whose work belongs to a later phase answer INTERNAL
 * and change nothing, the same as the Rust core.
 */
export const HANDLERS: Record<CommandName, Handler> = {
  list_monitors: (core) => core.monitors(),
  get_state: (core) => core.state(),
  apply_mode: (core) => core.notAvailable('apply_mode'),
  confirm_mode: (core, request) => core.confirm(request),
  revert_mode: (core, request) => core.revert(request),
  restore_native: (core) => core.notAvailable('restore_native'),
  restore_everything: (core) => core.notAvailable('restore_everything'),
  list_profiles: (core) => core.profiles(),
  save_profile: (core) => core.notAvailable('save_profile'),
  delete_profile: (core) => core.notAvailable('delete_profile'),
  set_auto_switch: (core) => core.notAvailable('set_auto_switch'),
  list_custom_entries: (core) => core.notAvailable('list_custom_entries'),
  add_custom_mode: (core) => core.notAvailable('add_custom_mode'),
  remove_custom_entry: (core) => core.notAvailable('remove_custom_entry'),
  undo_all_changes: (core) => core.notAvailable('undo_all_changes'),
  get_settings: (core) => core.settings(),
  save_settings: (core) => core.notAvailable('save_settings'),
  set_hotkey: (core) => core.notAvailable('set_hotkey'),
  set_autostart: (core) => core.notAvailable('set_autostart'),
  create_play_shortcut: (core) => core.notAvailable('create_play_shortcut'),
  check_for_update: (core) => core.notAvailable('check_for_update'),
  install_update: (core) => core.notAvailable('install_update'),
  open_log_folder: (core) => core.notAvailable('open_log_folder'),
  get_diagnostics: (core) => core.notAvailable('get_diagnostics'),
  dismiss_error: (core, request) => core.dismissError(request),
};

/** True for the names of ResSwap's own commands, and for nothing else. */
export function isCommandName(cmd: string): cmd is CommandName {
  return Object.hasOwn(HANDLERS, cmd);
}
