import { waitFor } from '@testing-library/react';
import { afterEach, describe, expect, test, vi } from 'vitest';
import {
  applyMode,
  confirmMode,
  dismissError,
  getSettings,
  getState,
  listMonitors,
  listProfiles,
  restoreEverything,
  restoreNative,
  revertMode,
  type Result,
  saveProfile,
  saveSettings,
} from '../client';
import { ERROR_MESSAGES, errorMessage } from '../errors';
import { type EventMap, onCoreEvent } from '../events';
import type { ApplyRequest } from '../types/ApplyRequest';
import type { CoreEvent } from '../types/CoreEvent';
import type { Mode } from '../types/Mode';
import type { RaisedError } from '../types/RaisedError';
import { MockCore } from './core';
import { mode, PRIMARY_ID, rate, twoProfiles } from './fixtures';
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

/** Every payload of one event from now on, in arrival order. */
async function record<E extends CoreEvent>(name: E): Promise<EventMap[E][]> {
  const seen: EventMap[E][] = [];
  await onCoreEvent(name, (payload) => {
    seen.push(payload);
  });
  return seen;
}

/** An apply request for the primary monitor, sent the way Home sends it. */
function onPrimary(target: Mode): ApplyRequest {
  return { kind: 'mode', monitor: PRIMARY_ID, mode: target, scaling: 'fullScreen' };
}

/** Lets the mocked events reach their listeners. */
async function settle(): Promise<void> {
  for (let turn = 0; turn < 20; turn += 1) {
    await Promise.resolve();
  }
}

afterEach(() => {
  vi.useRealTimers();
});

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
    const [profile] = twoProfiles();
    if (profile === undefined) {
      throw new Error('the fixture has two profiles');
    }
    const result = await saveProfile(profile);
    expect(result).toMatchObject({
      ok: false,
      error: {
        code: 'INTERNAL',
        detail: 'save_profile is not available in this build; nothing was changed',
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
      detail: 'In the window: not a ResSwap command: plugin:window|title',
    });
    core.dispose();
  });

  test('REQ-UI-17 an unknown scenario shows a banner that lists the valid IDs', () => {
    install('scenario=home-nativ&warn=restore_native:SCALING_NOT_APPLIED');
    const banner = document.querySelector('[role="alert"]');
    expect(banner?.textContent).toContain(
      'Unknown scenario "home-nativ". Valid scenarios: fake-backend, first-run, hidden-monitors, home-native, home-stretched, monitor-without-identity, monitors-without-path, pending-confirm, presets-offered, recovery-incomplete.',
    );
    expect(banner?.textContent).toContain(
      'Cannot use warn=restore_native:SCALING_NOT_APPLIED. The form is warn=<command>:<CODE>, and only apply_mode and confirm_mode return warnings.',
    );
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

describe('the mocked core on the P1 display paths', () => {
  test('REQ-SAFE-04 a mode already kept on the monitor applies at once, with no pending change', async () => {
    install('scenario=presets-offered');
    const pendingEvents = await record('mode-pending-confirm');
    const ticks = await record('countdown-tick');
    const outcome = valueOf(await applyMode(onPrimary(mode(1440, 1080, 240))));
    if (outcome.kind !== 'applied') {
      throw new Error(`expected applied, got ${outcome.kind}`);
    }
    expect(outcome.change.now.label).toBe('1440 x 1080 at 240 Hz');
    expect(outcome.change.before.label).toBe('1920 x 1080 at 240 Hz');
    expect(outcome.steps.map((step) => [step.step, step.status])).toEqual([
      ['test', 'done'],
      ['journal', 'done'],
      ['apply', 'done'],
      ['verify', 'done'],
      ['scaling', 'done'],
    ]);
    expect(outcome.warnings).toEqual([]);
    const state = valueOf(await getState());
    expect(state.revision).toBe(2);
    expect(state.pending).toBeNull();
    expect(state.active).toEqual([outcome.change]);
    expect(state.monitors[0]?.current?.label).toBe('1440 x 1080 at 240 Hz');
    expect(state.monitors[0]?.scaling).toBe('stretched');
    await settle();
    expect(pendingEvents).toEqual([]);
    expect(ticks).toEqual([]);
  });

  test('REQ-SAFE-03 an unconfirmed mode is pending, ticks once a second and reverts at 0', async () => {
    vi.useFakeTimers();
    install('scenario=home-native');
    const pendingEvents = await record('mode-pending-confirm');
    const ticks = await record('countdown-tick');
    const reverts = await record('mode-reverted');
    const outcome = valueOf(await applyMode(onPrimary(mode(1280, 960, 240))));
    if (outcome.kind !== 'pendingConfirm') {
      throw new Error(`expected pendingConfirm, got ${outcome.kind}`);
    }
    const { pending } = outcome;
    expect([pending.from.label, pending.to.label]).toEqual([
      '1920 x 1080 at 240 Hz',
      '1280 x 960 at 240 Hz',
    ]);
    expect([pending.countdownSecs, pending.secondsLeft]).toEqual([10, 10]);
    expect(valueOf(await getState()).pending).toEqual(pending);
    await settle();
    expect(pendingEvents).toEqual([pending]);
    expect(ticks.map((tick) => tick.secondsLeft)).toEqual([10]);

    await vi.advanceTimersByTimeAsync(3000);
    expect(ticks.map((tick) => tick.secondsLeft)).toEqual([10, 9, 8, 7]);
    // get_state shows the seconds the ticks carry, with no snapshot per tick.
    const during = valueOf(await getState());
    expect(during.pending?.secondsLeft).toBe(7);
    expect(during.revision).toBe(2);
    expect(reverts).toEqual([]);

    await vi.advanceTimersByTimeAsync(7000);
    expect(ticks.at(-1)?.secondsLeft).toBe(0);
    expect(reverts.map((item) => [item.changeId, item.reason, item.restored.label])).toEqual([
      [pending.changeId, 'timeout', '1920 x 1080 at 240 Hz'],
    ]);
    const after = valueOf(await getState());
    expect(after.pending).toBeNull();
    expect(after.active).toEqual([]);
    expect(after.monitors[0]?.current?.label).toBe('1920 x 1080 at 240 Hz');
    expect(after.monitors[0]?.scaling).toBe('identity');
    const tickCount = ticks.length;
    await vi.advanceTimersByTimeAsync(3000);
    expect(ticks).toHaveLength(tickCount);
  });

  test('REQ-DISP-19 Keep stores the mode, keeps the change ID, and the same mode then applies at once', async () => {
    install('scenario=home-native');
    const first = valueOf(await applyMode(onPrimary(mode(1280, 960, 240))));
    if (first.kind !== 'pendingConfirm') {
      throw new Error(`expected pendingConfirm, got ${first.kind}`);
    }
    const kept = valueOf(await confirmMode({ changeId: first.pending.changeId }));
    if (kept.kind !== 'kept') {
      throw new Error(`expected kept, got ${kept.kind}`);
    }
    expect(kept.change.changeId).toBe(first.pending.changeId);
    expect(kept.warnings).toEqual([]);
    const state = valueOf(await getState());
    expect(state.pending).toBeNull();
    expect(state.active).toEqual([kept.change]);
    expect(state.settings.confirmedModes).toEqual([
      { monitor: PRIMARY_ID, mode: mode(1280, 960, 240) },
    ]);
    // Keep again, too late: the change is not pending any more.
    expect(valueOf(await confirmMode({ changeId: first.pending.changeId }))).toEqual({
      kind: 'notPending',
    });

    valueOf(await restoreNative({ monitor: PRIMARY_ID }));
    const again = valueOf(await applyMode(onPrimary(mode(1280, 960, 240))));
    expect(again.kind).toBe('applied');
  });

  test('REQ-SAFE-03 Revert puts back the previous mode and sends mode-reverted with reason player', async () => {
    install('scenario=home-native');
    const reverts = await record('mode-reverted');
    const first = valueOf(await applyMode(onPrimary(mode(1280, 960, 240))));
    if (first.kind !== 'pendingConfirm') {
      throw new Error(`expected pendingConfirm, got ${first.kind}`);
    }
    const outcome = valueOf(await revertMode({ changeId: first.pending.changeId }));
    if (outcome.kind !== 'reverted') {
      throw new Error(`expected reverted, got ${outcome.kind}`);
    }
    expect(outcome.reverted.reason).toBe('player');
    expect(outcome.reverted.restored.label).toBe('1920 x 1080 at 240 Hz');
    expect(outcome.reverted.steps).toEqual([{ step: 'restore', status: 'done' }]);
    await settle();
    expect(reverts).toEqual([outcome.reverted]);
    const state = valueOf(await getState());
    expect(state.pending).toBeNull();
    expect(state.settings.confirmedModes).toEqual([]);
    expect(state.monitors[0]?.current?.label).toBe('1920 x 1080 at 240 Hz');
  });

  test('REQ-SAFE-03 the pending-confirm scenario starts with its countdown running', async () => {
    vi.useFakeTimers();
    install('scenario=pending-confirm&window=keep-mode');
    const ticks = await record('countdown-tick');
    const state = valueOf(await getState());
    expect(state.pending?.changeId).toBe('mock-change-1');
    expect(state.monitors[0]?.current?.label).toBe('1280 x 960 at 240 Hz');
    await vi.advanceTimersByTimeAsync(2000);
    expect(ticks.map((tick) => [tick.changeId, tick.secondsLeft])).toEqual([
      ['mock-change-1', 9],
      ['mock-change-1', 8],
    ]);
  });

  test('REQ-SAFE-03 a new apply on the same monitor puts the pending change back first', async () => {
    install('scenario=presets-offered');
    const reverts = await record('mode-reverted');
    const first = valueOf(await applyMode(onPrimary(mode(1350, 1080, 240))));
    expect(first.kind).toBe('pendingConfirm');
    const second = valueOf(await applyMode(onPrimary(mode(1440, 1080, 240))));
    if (second.kind !== 'applied') {
      throw new Error(`expected applied, got ${second.kind}`);
    }
    // The baseline stays the mode from before the first change.
    expect(second.change.before.label).toBe('1920 x 1080 at 240 Hz');
    await settle();
    expect(reverts.map((item) => item.reason)).toEqual(['superseded']);
  });

  test('REQ-DISP-11 a typed size the monitor does not list answers MODE_NOT_OFFERED and changes nothing', async () => {
    install('scenario=home-native');
    const typed: Mode = { width: 1440, height: 1080, refresh: rate(240), label: '' };
    const result = await applyMode(onPrimary(typed));
    if (result.ok) {
      throw new Error('expected MODE_NOT_OFFERED');
    }
    expect(result.error.code).toBe('MODE_NOT_OFFERED');
    expect(result.error.context.mode?.label).toBe('1440 x 1080 at 240 Hz');
    expect(errorMessage(result.error, { monitorOnNvidia: true })).toBe(
      'Windows does not offer 1440 x 1080 on Primary monitor. Nothing changed. On an NVIDIA card you can add it under Custom resolutions.',
    );
    expect(valueOf(await getState()).revision).toBe(1);
  });

  test('REQ-DISP-17 a listed size at another rate answers REFRESH_NOT_AVAILABLE with the rates it offers', async () => {
    install('scenario=home-native');
    const typed: Mode = { width: 1280, height: 960, refresh: rate(144), label: '' };
    const result = await applyMode(onPrimary(typed));
    if (result.ok) {
      throw new Error('expected REFRESH_NOT_AVAILABLE');
    }
    expect(errorMessage(result.error)).toBe(
      'Primary monitor offers 1280 x 960 but not at 144 Hz. Nothing changed. Pick one of the rates it offers: 240 Hz, 60 Hz.',
    );
  });

  test('REQ-DISP-11 a typed size the monitor lists takes the same path as a preset', async () => {
    install('scenario=home-native');
    const typed: Mode = { width: 1280, height: 960, refresh: rate(240), label: '' };
    const outcome = valueOf(await applyMode(onPrimary(typed)));
    if (outcome.kind !== 'pendingConfirm') {
      throw new Error(`expected pendingConfirm, got ${outcome.kind}`);
    }
    // The core's own label, never the empty one the window sent.
    expect(outcome.pending.to.label).toBe('1280 x 960 at 240 Hz');
  });

  test('REQ-UI-09 an apply the core cannot use answers its own code, and INTERNAL only when no code fits', async () => {
    install('scenario=home-native');
    const zero: Mode = { width: 0, height: 960, refresh: rate(240), label: '' };
    expect(await applyMode(onPrimary(zero))).toMatchObject({
      ok: false,
      error: { code: 'INTERNAL' },
    });
    const gone = await applyMode({
      kind: 'mode',
      monitor: { ...PRIMARY_ID, devicePath: 'mock-device-path:unplugged' },
      mode: mode(1280, 960, 240),
      scaling: 'fullScreen',
    });
    expect(gone).toMatchObject({ ok: false, error: { code: 'MONITOR_GONE' } });
    const unknown = await applyMode({ kind: 'profile', profileId: 'not-a-profile' });
    expect(unknown).toMatchObject({ ok: false, error: { code: 'PROFILE_INVALID' } });
    expect(valueOf(await getState()).revision).toBe(1);
  });

  test('REQ-DISP-21 restore_native puts the monitor back on its native size at the rate it had', async () => {
    install('scenario=home-stretched');
    const outcome = valueOf(await restoreNative({ monitor: PRIMARY_ID }));
    expect(outcome.restored).toEqual([PRIMARY_ID]);
    const state = valueOf(await getState());
    expect(state.active).toEqual([]);
    expect(state.monitors[0]?.current?.label).toBe('1920 x 1080 at 240 Hz');
    expect(state.monitors[0]?.scaling).toBe('identity');
    // Nothing is left to restore.
    expect(valueOf(await restoreNative({ monitor: null }))).toEqual({ restored: [], steps: [] });
  });

  test('REQ-DISP-22 restore_everything puts back every open change, and says so when there is none', async () => {
    install('scenario=home-stretched');
    const outcome = valueOf(await restoreEverything());
    expect(outcome.restored).toEqual([PRIMARY_ID]);
    expect(outcome.steps).toEqual([{ step: 'restore', status: 'done' }]);
    expect(valueOf(await getState()).monitors[0]?.current?.label).toBe('1920 x 1080 at 240 Hz');
    expect(valueOf(await restoreEverything())).toEqual({ restored: [], steps: [] });
  });

  test('REQ-DISP-22 restore_everything clears a recovery that did not finish', async () => {
    install('scenario=recovery-incomplete');
    const outcome = valueOf(await restoreEverything());
    expect(outcome.restored).toEqual([PRIMARY_ID]);
    expect(valueOf(await getState()).recovery).toEqual({ status: 'clean', stillChanged: [] });
  });

  test('REQ-DISP-08 save_settings stores the show-hidden toggle and sends the new state', async () => {
    install('scenario=home-native');
    const snapshots = await record('state-changed');
    const before = valueOf(await getSettings());
    const saved = valueOf(
      await saveSettings({
        revertCountdownSecs: before.revertCountdownSecs,
        checkForUpdates: before.checkForUpdates,
        showHiddenMonitors: true,
        selectedMonitor: null,
        playStretchedProfile: null,
      }),
    );
    expect(saved).toEqual({ ...before, showHiddenMonitors: true });
    await settle();
    expect(snapshots.map((state) => state.settings.showHiddenMonitors)).toEqual([true]);
  });

  test('REQ-UI-08 warn= puts the warning in a successful outcome', async () => {
    install(
      'scenario=presets-offered&warn=apply_mode:SCALING_NOT_APPLIED&warn=confirm_mode:SETTINGS_WRITE_FAILED',
    );
    const applied = valueOf(await applyMode(onPrimary(mode(1440, 1080, 240))));
    expect(applied.warnings.map((warning) => warning.code)).toEqual(['SCALING_NOT_APPLIED']);
    expect(applied.steps.at(-1)).toEqual({ step: 'scaling', status: 'failed' });
    expect(valueOf(await getState()).monitors[0]?.scaling).toBe('identity');

    const pending = valueOf(await applyMode(onPrimary(mode(1350, 1080, 240))));
    if (pending.kind !== 'pendingConfirm') {
      throw new Error(`expected pendingConfirm, got ${pending.kind}`);
    }
    const kept = valueOf(await confirmMode({ changeId: pending.pending.changeId }));
    if (kept.kind !== 'kept') {
      throw new Error(`expected kept, got ${kept.kind}`);
    }
    expect(kept.warnings.map((warning) => warning.code)).toEqual(['SETTINGS_WRITE_FAILED']);
    // The write failed, so the mode is kept now but not stored.
    const stored = valueOf(await getState()).settings.confirmedModes.map((item) => item.mode.label);
    expect(stored).toEqual(['1440 x 1080 at 240 Hz']);
  });
});
