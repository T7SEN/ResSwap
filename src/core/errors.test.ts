import { describe, expect, test } from 'vitest';
import { emptyContext, ERROR_MESSAGES, errorMessage, isErrorCode, toIpcError } from './errors';
import { mode, rate } from './mock/fixtures';
import errorCodeSource from './types/ErrorCode.ts?raw';
import type { ErrorCode } from './types/ErrorCode';
import type { ErrorContext } from './types/ErrorContext';
import type { IpcError } from './types/IpcError';
import type { StepReport } from './types/StepReport';

const CODES = Object.keys(ERROR_MESSAGES).filter(isErrorCode);

function errorOf(
  code: ErrorCode,
  context: Partial<ErrorContext> = {},
  steps: StepReport[] = [],
): IpcError {
  return { code, context: { ...emptyContext(), ...context }, steps, detail: '' };
}

// Every placeholder filled, so the check below also reads the filled form of each message.
const FULL_CONTEXT: Partial<ErrorContext> = {
  monitor: 'Primary monitor',
  mode: mode(1440, 1080, 240),
  current: mode(1920, 1080, 240),
  offeredRefresh: [rate(240), rate(60)],
  adapter: 'Other graphics adapter',
  nvapiFunction: 'NvAPI_DISP_TryCustomDisplay',
  nvapiStatus: -1,
  tool: 'pnputil',
  exitCode: 1,
  exe: 'game.exe',
  profile: 'My game',
  profileProblem: 'invalidFields',
  fields: ['exe', 'mode'],
  hotkey: 'Ctrl+Alt+F9',
  backupMs: Date.UTC(2026, 10, 12, 8, 0, 0),
  settingsSource: 'lastGoodCopy',
  autostartEnabled: true,
};

const ONE_STEP: StepReport[] = [{ step: 'backup', status: 'done' }];

describe('error messages', () => {
  test('REQ-UI-08 every error code has its own message, and the codes match ErrorCode.ts', () => {
    const declared = [...errorCodeSource.matchAll(/"([A-Z_]+)"/g)].map((match) => match[1]);
    expect(declared).toHaveLength(29);
    expect([...CODES].sort()).toEqual([...declared].sort());

    const bare = CODES.map((code) => errorMessage(errorOf(code)));
    expect(new Set(bare).size).toBe(29);
    const filled = CODES.map((code) => errorMessage(errorOf(code, FULL_CONTEXT, ONE_STEP)));
    expect(new Set(filled).size).toBe(29);
  });

  test('REQ-UI-11 no message holds an exclamation mark, an em dash or an en dash', () => {
    // Built from code points, so this file holds neither dash itself.
    const marks = ['!', String.fromCharCode(0x2014), String.fromCharCode(0x2013)];
    for (const code of CODES) {
      for (const error of [errorOf(code), errorOf(code, FULL_CONTEXT, ONE_STEP)]) {
        const text = errorMessage(error, { monitorOnNvidia: true });
        for (const mark of marks) {
          expect(text, code).not.toContain(mark);
        }
        expect(text, code).not.toContain(' - ');
      }
    }
  });

  test('REQ-UI-08 placeholders take the values from the context', () => {
    expect(errorMessage(errorOf('MODE_REJECTED', FULL_CONTEXT))).toBe(
      'Windows did not accept 1440 x 1080 at 240 Hz on Primary monitor. The display is on 1920 x 1080 at 240 Hz.',
    );
    expect(errorMessage(errorOf('REFRESH_NOT_AVAILABLE', FULL_CONTEXT))).toBe(
      'Primary monitor offers 1440 x 1080 but not at 240 Hz. Nothing changed. Pick one of the rates it offers: 240 Hz, 60 Hz.',
    );
    expect(errorMessage(errorOf('PROFILE_INVALID', FULL_CONTEXT))).toBe(
      'ResSwap cannot use the profile "My game": these fields need fixing: Game file, Resolution. Nothing changed.',
    );
    expect(errorMessage(errorOf('ELEVATED_TOOL_FAILED', FULL_CONTEXT, ONE_STEP))).toBe(
      'pnputil.exe ran with administrator rights but reported an error (exit code 1). The steps below show what finished.',
    );
  });

  test('REQ-UI-08 an absent value reads as its fallback, capitalised at the start of a sentence', () => {
    expect(errorMessage(errorOf('MODE_REJECTED'))).toBe(
      'Windows did not accept this mode on this monitor. The display is on the mode it had before.',
    );
    expect(errorMessage(errorOf('MONITOR_GONE'))).toBe(
      'This monitor is not connected any more, so ResSwap cannot change it. Reconnect it or choose another monitor.',
    );
    expect(errorMessage(errorOf('ELEVATED_TOOL_FAILED'))).toBe(
      'The Windows tool ran with administrator rights but reported an error (exit code unknown).',
    );
    expect(errorMessage(errorOf('HOTKEY_TAKEN'))).toBe(
      'This combination is already in use by another app. Choose a different combination.',
    );
    expect(errorMessage(errorOf('PROFILE_INVALID'))).toBe(
      'ResSwap cannot use this profile: it is incomplete. Nothing changed.',
    );
    expect(errorMessage(errorOf('AUTOSTART_FAILED'))).toBe(
      'ResSwap could not change Start with Windows. It is still unchanged.',
    );
  });

  test('REQ-SAFE-17 the steps sentence appears only when the error carries steps', () => {
    const sentence = 'The steps below show what finished.';
    for (const code of [
      'NVAPI_CALL_FAILED',
      'UAC_DECLINED',
      'ELEVATED_TOOL_FAILED',
      'DRIVER_RESTART_REFUSED_GAME_RUNNING',
    ] as const) {
      expect(errorMessage(errorOf(code)), code).not.toContain(sentence);
      expect(errorMessage(errorOf(code, {}, ONE_STEP)), code).toContain(sentence);
    }
  });

  test('REQ-UI-08 the NVIDIA sentence of MODE_NOT_OFFERED shows only for an NVIDIA monitor', () => {
    const error = errorOf('MODE_NOT_OFFERED', FULL_CONTEXT);
    const nvidia = 'On an NVIDIA card you can add it under Custom resolutions.';
    expect(errorMessage(error)).toBe(
      'Windows does not offer 1440 x 1080 on Primary monitor. Nothing changed.',
    );
    expect(errorMessage(error, { monitorOnNvidia: true })).toContain(nvidia);
  });
});

describe('toIpcError', () => {
  test('REQ-UI-08 an IpcError from the core passes through unchanged', () => {
    const error = errorOf('UAC_DECLINED', { tool: 'regExe' }, ONE_STEP);
    expect(toIpcError(JSON.parse(JSON.stringify(error)))).toEqual(error);
  });

  test('REQ-UI-09 anything that is not an IpcError becomes INTERNAL with the raw text', () => {
    const fromString = toIpcError('invalid args `request` for command `apply_mode`');
    expect(fromString.code).toBe('INTERNAL');
    expect(fromString.detail).toBe('invalid args `request` for command `apply_mode`');
    expect(fromString.context).toEqual(emptyContext());
    expect(fromString.steps).toEqual([]);

    expect(toIpcError(new TypeError('no IPC'))).toMatchObject({
      code: 'INTERNAL',
      detail: 'TypeError: no IPC',
    });
    expect(toIpcError({ code: 'NOT_A_CODE', context: {} })).toMatchObject({
      code: 'INTERNAL',
      detail: '{"code":"NOT_A_CODE","context":{}}',
    });
    expect(toIpcError(undefined)).toMatchObject({ code: 'INTERNAL', detail: 'undefined' });
  });

  test('REQ-UI-08 a known code with missing fields still gives a whole error', () => {
    const error = toIpcError({ code: 'MONITOR_GONE', context: { monitor: 'Primary monitor' } });
    expect(error).toEqual(errorOf('MONITOR_GONE', { monitor: 'Primary monitor' }));
    expect(errorMessage(error)).toMatch(/^Primary monitor is not connected any more/);
  });
});
