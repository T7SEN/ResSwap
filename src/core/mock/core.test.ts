import { waitFor } from '@testing-library/react';
import { describe, expect, test } from 'vitest';
import {
  applyMode,
  confirmMode,
  dismissError,
  getSettings,
  getState,
  listMonitors,
  listProfiles,
  revertMode,
  type Result,
} from '../client';
import { ERROR_MESSAGES, errorMessage } from '../errors';
import { onCoreEvent } from '../events';
import type { RaisedError } from '../types/RaisedError';
import { MockCore } from './core';
import { installMockCore } from './index';
import { readParams } from './scenarios';

function install(query: string): void {
  installMockCore(new URLSearchParams(query));
}

function valueOf<T>(result: Result<T>): T {
  if (!result.ok) {
    throw new Error(`expected a value, got ${result.error.code}: ${result.error.detail}`);
  }
  return result.value;
}

describe('the mocked core', () => {
  test('REQ-UI-17 it answers get_state and list_monitors as the fake backend does at start', async () => {
    install('scenario=first-run');
    const state = valueOf(await getState());
    expect(state.revision).toBe(1);
    expect(state.profiles).toEqual([]);
    expect(state.active).toEqual([]);
    expect(state.pending).toBeNull();
    expect(state.autoSwitch).toEqual({ enabled: false, watched: [], matched: null });
    expect(state.recovery).toEqual({ status: 'clean', stillChanged: [] });
    expect(state.settings).toEqual({
      schemaVersion: 1,
      revertCountdownSecs: 10,
      checkForUpdates: true,
      showHiddenMonitors: false,
      selectedMonitor: null,
      playStretchedProfile: null,
      startWithWindows: false,
      hotkeys: [],
      confirmedModes: [],
    });

    const monitors = valueOf(await listMonitors());
    expect(monitors).toEqual(state.monitors);
    expect(monitors.map((monitor) => [monitor.name, monitor.hidden, monitor.hiddenReason])).toEqual(
      [
        ['Primary monitor', false, null],
        ['Virtual display', true, 'virtualAdapter'],
      ],
    );
    const [primary] = monitors;
    expect(primary?.current?.label).toBe('1920 x 1080 at 240 Hz');
    expect(primary?.presets.map((preset) => [preset.mode.label, preset.offered])).toEqual([
      ['1440 x 1080 at 240 Hz', false],
      ['1350 x 1080 at 240 Hz', false],
      ['1728 x 1080 at 240 Hz', false],
    ]);
    expect(valueOf(await listProfiles())).toEqual([]);
    expect(valueOf(await getSettings())).toEqual(state.settings);
  });

  test('REQ-UI-09 a command whose work belongs to a later phase answers INTERNAL', async () => {
    install('');
    const result = await applyMode({ kind: 'profile', profileId: 'x' });
    expect(result).toMatchObject({
      ok: false,
      error: {
        code: 'INTERNAL',
        detail: 'apply_mode is not available in this build; nothing was changed',
      },
    });
    expect(valueOf(await getState()).revision).toBe(1);
  });

  test('REQ-UI-17 Keep and Revert with nothing pending answer notPending', async () => {
    install('');
    expect(valueOf(await confirmMode({ changeId: 'change-1' }))).toEqual({ kind: 'notPending' });
    expect(valueOf(await revertMode({ changeId: 'change-1' }))).toEqual({ kind: 'notPending' });
  });

  test('REQ-UI-08 fail=<command>:<CODE> returns that error with a context that reads well', async () => {
    install('fail=get_state:MODE_REJECTED');
    const result = await getState();
    if (result.ok) {
      throw new Error('expected MODE_REJECTED');
    }
    expect(result.error.code).toBe('MODE_REJECTED');
    expect(errorMessage(result.error)).toBe(
      'Windows did not accept 1440 x 1080 at 240 Hz on Primary monitor. The display is on 1920 x 1080 at 240 Hz.',
    );
  });

  test('REQ-UI-08 a command named twice in fail= keeps its first code, as the Rust core does', async () => {
    install('fail=get_state:MODE_REJECTED&fail=get_state:INTERNAL');
    expect(document.querySelector('[role="alert"]')).toBeNull();
    const result = await getState();
    expect(result).toMatchObject({ ok: false, error: { code: 'MODE_REJECTED' } });
  });

  test('REQ-UI-08 every error code has a fail trigger and a raise trigger', () => {
    for (const code of Object.keys(ERROR_MESSAGES)) {
      const params = readParams(new URLSearchParams(`fail=apply_mode:${code}&raise=${code}`));
      expect(params.problems, code).toEqual([]);
    }
  });

  test('REQ-UI-08 raise=<CODE> fires error-raised once after the first read, then lists it', async () => {
    install('raise=SETTINGS_CORRUPT');
    const raised: RaisedError[] = [];
    await onCoreEvent('error-raised', (entry) => {
      raised.push(entry);
    });
    expect(valueOf(await getState()).errors).toEqual([]);
    await waitFor(() => {
      expect(raised).toHaveLength(1);
    });
    const state = valueOf(await getState());
    expect(state.revision).toBe(2);
    expect(state.errors).toEqual(raised);
    expect(raised[0]?.source).toBe('startup');
    expect(raised[0]?.error.code).toBe('SETTINGS_CORRUPT');
  });

  test('REQ-UI-16 dismiss_error removes the entry, sends state-changed and refuses an unknown ID', async () => {
    install('scenario=recovery-incomplete');
    const revisions: number[] = [];
    await onCoreEvent('state-changed', (state) => {
      revisions.push(state.revision);
    });
    const before = valueOf(await getState());
    expect(before.errors.map((entry) => entry.error.code)).toEqual(['RECOVERY_INCOMPLETE']);

    expect(valueOf(await dismissError({ errorId: 'error-1' }))).toBeNull();
    expect(revisions).toEqual([2]);
    expect(valueOf(await getState()).errors).toEqual([]);

    const again = await dismissError({ errorId: 'error-1' });
    expect(again).toMatchObject({
      ok: false,
      error: { code: 'INTERNAL', detail: 'no raised error has this ID' },
    });
  });

  test('REQ-UI-17 a call that is not a ResSwap command answers INTERNAL', async () => {
    const { scenario, faults } = readParams(new URLSearchParams());
    const core = new MockCore(scenario, faults);
    await expect(core.dispatch('plugin:window|title', {})).rejects.toMatchObject({
      code: 'INTERNAL',
      detail: 'not a ResSwap command: plugin:window|title',
    });
    core.dispose();
  });

  test('REQ-UI-17 an unknown scenario shows a banner that lists the valid IDs', () => {
    install('scenario=home-nativ&warn=apply_mode:SCALING_NOT_APPLIED');
    const banner = document.querySelector('[role="alert"]');
    expect(banner?.textContent).toContain(
      'Unknown scenario "home-nativ". Valid scenarios: fake-backend, first-run, hidden-monitors, home-native, home-stretched, recovery-incomplete.',
    );
    expect(banner?.textContent).toContain('Cannot use warn=apply_mode:SCALING_NOT_APPLIED');
  });

  test('REQ-UI-17 the page carries the marker, and the mocked core refuses to run inside ResSwap', () => {
    install('');
    expect(document.documentElement.dataset.core).toBe('RESSWAP_MOCK_CORE');
    Object.defineProperty(globalThis, 'isTauri', { value: true, configurable: true });
    try {
      expect(() => {
        install('');
      }).toThrow('The mocked core runs only in a normal browser, never inside ResSwap.');
    } finally {
      Reflect.deleteProperty(globalThis, 'isTauri');
    }
  });
});
