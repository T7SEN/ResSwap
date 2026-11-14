import { clearMocks, mockIPC, mockWindows } from '@tauri-apps/api/mocks';
import { describe, expect, test } from 'vitest';
import * as client from './client';
import { PRIMARY_ID, mode } from './mock/fixtures';
import type { CommandName } from './types/CommandName';
import type { IpcError } from './types/IpcError';
import type { Profile } from './types/Profile';

type Call = { run: () => Promise<client.Result<unknown>>; request: unknown };

const PROFILE: Profile = {
  id: '3f6c2a1e-5b7d-4c8e-9a0f-1b2c3d4e5f60',
  name: 'My game',
  exe: 'game.exe',
  monitor: PRIMARY_ID,
  mode: mode(1440, 1080, 240),
  scaling: 'fullScreen',
  turnOff: [],
  autoSwitch: false,
  restoreDelaySecs: 10,
  launchPath: null,
};

function withRequest<T>(
  run: (request: T) => Promise<client.Result<unknown>>,
  request: NoInfer<T>,
): Call {
  return { run: () => run(request), request };
}

function bare(run: () => Promise<client.Result<unknown>>): Call {
  return { run, request: null };
}

// One entry per command; a missing or extra name fails pnpm typecheck.
const CALLS: Record<CommandName, Call> = {
  list_monitors: bare(client.listMonitors),
  get_state: bare(client.getState),
  apply_mode: withRequest(client.applyMode, {
    kind: 'mode',
    monitor: PRIMARY_ID,
    mode: mode(1440, 1080, 240),
    scaling: 'fullScreen',
  }),
  confirm_mode: withRequest(client.confirmMode, { changeId: 'change-1' }),
  revert_mode: withRequest(client.revertMode, { changeId: 'change-1' }),
  restore_native: withRequest(client.restoreNative, { monitor: null }),
  restore_everything: bare(client.restoreEverything),
  list_profiles: bare(client.listProfiles),
  save_profile: withRequest(client.saveProfile, PROFILE),
  delete_profile: withRequest(client.deleteProfile, { profileId: PROFILE.id }),
  set_auto_switch: withRequest(client.setAutoSwitch, { enabled: true }),
  list_custom_entries: bare(client.listCustomEntries),
  add_custom_mode: withRequest(client.addCustomMode, {
    monitor: PRIMARY_ID,
    width: 1440,
    height: 1080,
    approvedPlan: null,
  }),
  remove_custom_entry: withRequest(client.removeCustomEntry, {
    entryId: 'entry-1',
    approvedPlan: null,
  }),
  undo_all_changes: withRequest(client.undoAllChanges, { approvedPlan: null }),
  get_settings: bare(client.getSettings),
  save_settings: withRequest(client.saveSettings, {
    revertCountdownSecs: 10,
    checkForUpdates: true,
    showHiddenMonitors: false,
    selectedMonitor: null,
    playStretchedProfile: null,
  }),
  set_hotkey: withRequest(client.setHotkey, { action: 'restoreNative', accelerator: null }),
  set_autostart: withRequest(client.setAutostart, { enabled: false }),
  create_play_shortcut: withRequest(client.createPlayShortcut, { profileId: PROFILE.id }),
  check_for_update: bare(client.checkForUpdate),
  install_update: bare(client.installUpdate),
  open_log_folder: bare(client.openLogFolder),
  get_diagnostics: bare(client.getDiagnostics),
  dismiss_error: withRequest(client.dismissError, { errorId: 'error-1' }),
};

function expectInternal(result: client.Result<unknown>): IpcError {
  expect(result.ok).toBe(false);
  if (result.ok) {
    throw new Error('expected a failed result');
  }
  expect(result.error.code).toBe('INTERNAL');
  return result.error;
}

describe('the typed facade', () => {
  test('REQ-UI-15 each function calls its command by its exact name with the request key', async () => {
    const seen: { cmd: string; payload: unknown }[] = [];
    mockIPC((cmd, payload) => {
      seen.push({ cmd, payload });
      return null;
    });
    const names = Object.keys(CALLS) as CommandName[];
    expect(names).toHaveLength(25);
    for (const name of names) {
      const call = CALLS[name];
      seen.length = 0;
      const result = await call.run();
      expect(result, name).toEqual({ ok: true, value: null });
      // A command with no argument goes out with an empty object.
      expect(seen, name).toEqual([
        { cmd: name, payload: call.request === null ? {} : { request: call.request } },
      ]);
    }
  });

  test('REQ-UI-20 an error thrown on the way comes back as INTERNAL, never as a throw', async () => {
    mockIPC(() => {
      throw new Error('the window has no permission for this command');
    });
    const error = expectInternal(await client.getState());
    expect(error.detail).toBe(
      'In the window: Error: the window has no permission for this command',
    );
  });

  test("REQ-UI-09 Tauri's own string rejection becomes INTERNAL with the text in the detail", async () => {
    const text = 'invalid args `request` for command `apply_mode`: missing field `kind`';
    // eslint-disable-next-line @typescript-eslint/prefer-promise-reject-errors -- Tauri rejects with a plain string when it cannot read a request; the test sends the same.
    mockIPC(() => Promise.reject(text));
    const error = expectInternal(await client.applyMode({ kind: 'profile', profileId: 'x' }));
    expect(error.detail).toBe(`In the window: ${text}`);
  });

  test('REQ-UI-20 a command still settles when no IPC exists at all', async () => {
    clearMocks();
    Reflect.deleteProperty(window, '__TAURI_INTERNALS__');
    const error = expectInternal(await client.listMonitors());
    expect(error.detail).toMatch(/^In the window: TypeError: /);
  });

  test('REQ-UI-01 the window label comes from the metadata, and only keep-mode is special', () => {
    mockWindows('keep-mode');
    expect(client.currentWindowLabel()).toBe('keep-mode');
    mockWindows('main');
    expect(client.currentWindowLabel()).toBe('main');
    mockWindows('settings');
    expect(client.currentWindowLabel()).toBe('main');
  });

  test('REQ-UI-17 isTauri is false in a normal browser, mocks included', () => {
    mockIPC(() => null);
    expect(client.isTauri()).toBe(false);
  });
});
