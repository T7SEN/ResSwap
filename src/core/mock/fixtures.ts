import type { ActiveChange } from '../types/ActiveChange';
import type { AppState } from '../types/AppState';
import type { Aspect } from '../types/Aspect';
import type { JournalItem } from '../types/JournalItem';
import type { Mode } from '../types/Mode';
import type { Monitor } from '../types/Monitor';
import type { MonitorId } from '../types/MonitorId';
import type { Preset } from '../types/Preset';
import type { Profile } from '../types/Profile';
import type { RefreshRate } from '../types/RefreshRate';
import type { Settings } from '../types/Settings';

/** One fixed time for every timestamp, so screenshots match from run to run. */
export const FIXTURE_TIME_MS = Date.UTC(2026, 10, 12, 8, 0, 0);

/** A whole-hertz rate, n / 1, labelled the way the core labels it. */
export function rate(hz: number): RefreshRate {
  return { numerator: hz, denominator: 1, label: `${String(hz)} Hz` };
}

/** A mode labelled the way the core labels it, for example "1280 x 960 at 240 Hz". */
export function mode(width: number, height: number, hz: number): Mode {
  const refresh = rate(hz);
  return {
    width,
    height,
    refresh,
    label: `${String(width)} x ${String(height)} at ${refresh.label}`,
  };
}

/** The core's preset rule: the width for an aspect at the native height, rounded half up. */
function presets(native: Mode, hz: number, modes: readonly Mode[]): Preset[] {
  const aspects: [Aspect, number, number][] = [
    ['fourThree', 4, 3],
    ['fiveFour', 5, 4],
    ['sixteenTen', 16, 10],
  ];
  return aspects.map(([aspect, num, den]) => {
    const width = Math.floor((2 * native.height * num + den) / (2 * den));
    return {
      aspect,
      mode: mode(width, native.height, hz),
      offered: modes.some(
        (listed) =>
          listed.width === width &&
          listed.height === native.height &&
          listed.refresh.numerator === hz &&
          listed.refresh.denominator === 1,
      ),
    };
  });
}

/** The visible monitor, with the test PC's numbers and a neutral name. */
export const PRIMARY_ID: MonitorId = {
  devicePath: 'mock-device-path:primary-monitor',
  edidManufacturerId: null,
  edidProductCode: null,
  connectorInstance: 0,
};

/** A virtual display, hidden by its adapter (REQ-DISP-07). */
export const VIRTUAL_ID: MonitorId = {
  devicePath: 'mock-device-path:virtual-display',
  edidManufacturerId: null,
  edidProductCode: null,
  connectorInstance: 0,
};

/** The NVIDIA adapter of the test PC. */
export const NVIDIA_ADAPTER = 'NVIDIA GeForce RTX 4060 Ti';

/** 1920 x 1080 native at the test PC's rates, and the same extra sizes the fake backend lists. */
export function primaryMonitor(current: Mode = mode(1920, 1080, 240)): Monitor {
  const native = mode(1920, 1080, 240);
  const modes: Mode[] = [240, 144, 120, 100, 75, 60, 48].map((hz) => mode(1920, 1080, hz));
  modes.push(mode(1280, 882, 240));
  for (const [width, height] of [
    [1280, 1024],
    [1280, 960],
    [1024, 768],
    [800, 600],
  ] as const) {
    modes.push(mode(width, height, 240), mode(width, height, 60));
  }
  return {
    id: PRIMARY_ID,
    name: 'Primary monitor',
    adapter: NVIDIA_ADAPTER,
    vendor: 'nvidia',
    primary: true,
    active: true,
    hidden: false,
    hiddenReason: null,
    native,
    current,
    scaling: current.width === native.width ? 'identity' : 'stretched',
    modes,
    presets: presets(native, current.refresh.numerator, modes),
  };
}

/** The hidden virtual display. */
export function virtualDisplay(): Monitor {
  const native = mode(1920, 1080, 60);
  const modes = [native];
  return {
    id: VIRTUAL_ID,
    name: 'Virtual display',
    adapter: 'Virtual display adapter',
    vendor: 'other',
    primary: false,
    active: true,
    hidden: true,
    hiddenReason: 'virtualAdapter',
    native,
    current: native,
    scaling: 'identity',
    modes,
    presets: presets(native, 60, modes),
  };
}

/** The settings at their defaults (D-007). */
export function defaultSettings(): Settings {
  return {
    schemaVersion: 1,
    revertCountdownSecs: 10,
    checkForUpdates: true,
    showHiddenMonitors: false,
    selectedMonitor: null,
    playStretchedProfile: null,
    startWithWindows: false,
    hotkeys: [],
    confirmedModes: [],
  };
}

/** Two profiles with neutral names; neither has auto-switch on. */
export function twoProfiles(): Profile[] {
  const stretched = mode(1440, 1080, 240);
  return [
    {
      id: '3f6c2a1e-5b7d-4c8e-9a0f-1b2c3d4e5f60',
      name: 'My game',
      exe: 'game.exe',
      monitor: PRIMARY_ID,
      mode: stretched,
      scaling: 'fullScreen',
      turnOff: [],
      autoSwitch: false,
      restoreDelaySecs: 10,
      launchPath: null,
    },
    {
      id: '8a9b0c1d-2e3f-4a5b-8c7d-9e0f1a2b3c4d',
      name: 'My second game',
      exe: 'second-game.exe',
      monitor: PRIMARY_ID,
      mode: mode(1280, 960, 240),
      scaling: 'fullScreen',
      turnOff: [],
      autoSwitch: false,
      restoreDelaySecs: 10,
      launchPath: null,
    },
  ];
}

/** The change ResSwap made in the stretched scenario. */
export function stretchedChange(): ActiveChange {
  return {
    changeId: 'mock-change-1',
    monitor: PRIMARY_ID,
    monitorName: 'Primary monitor',
    before: mode(1920, 1080, 240),
    now: mode(1280, 960, 240),
    scaling: 'stretched',
    turnedOff: [],
    profileId: null,
    source: 'window',
    sinceMs: FIXTURE_TIME_MS,
  };
}

/** Two things left changed after a recovery that did not finish. */
export function stillChangedItems(): JournalItem[] {
  return [
    { kind: 'mode', monitorName: 'Primary monitor', mode: mode(1280, 960, 240) },
    { kind: 'scaling', monitorName: 'Primary monitor', mode: null },
  ];
}

/** The state a release build has after start on the test PC: no profiles, nothing changed. */
export function baseState(): AppState {
  return {
    revision: 1,
    monitors: [primaryMonitor(), virtualDisplay()],
    settings: defaultSettings(),
    profiles: [],
    active: [],
    pending: null,
    autoSwitch: { enabled: false, watched: [], matched: null },
    gpus: [],
    busy: null,
    errors: [],
    recovery: { status: 'clean', stillChanged: [] },
    update: null,
    build: { version: '0.1.0', debug: false, backend: 'real' },
  };
}
