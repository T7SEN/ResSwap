import { emit } from '@tauri-apps/api/event';
import { emptyContext, internalError } from '../errors';
import type { ActiveChange } from '../types/ActiveChange';
import type { AppState } from '../types/AppState';
import type { CommandName } from '../types/CommandName';
import type { CountdownTick } from '../types/CountdownTick';
import type { ErrorCode } from '../types/ErrorCode';
import type { ErrorContext } from '../types/ErrorContext';
import type { ErrorSource } from '../types/ErrorSource';
import type { IpcError } from '../types/IpcError';
import type { Mode } from '../types/Mode';
import type { ModeReverted } from '../types/ModeReverted';
import type { Monitor } from '../types/Monitor';
import type { MonitorId } from '../types/MonitorId';
import type { PendingChange } from '../types/PendingChange';
import type { RaisedError } from '../types/RaisedError';
import type { RefreshRate } from '../types/RefreshRate';
import type { RevertReason } from '../types/RevertReason';
import type { Scaling } from '../types/Scaling';
import type { ScalingChoice } from '../types/ScalingChoice';
import type { Settings } from '../types/Settings';
import type { StepReport } from '../types/StepReport';
import {
  FIXTURE_TIME_MS,
  mode,
  monitorOn,
  NVIDIA_ADAPTER,
  rate,
  rateLabel,
  sameMode,
  sameRate,
  stillChangedItems,
} from './fixtures';
import { HANDLERS, isCommandName } from './handlers';
import type { Faults, Scenario, WarningCommand } from './scenarios';

/** A handler's answer: the value the command returns, or the error it rejects with. */
export type Answer = { ok: true; value: unknown } | { ok: false; error: IpcError };

type EventPayloads = {
  'state-changed': AppState;
  'error-raised': RaisedError;
  'mode-pending-confirm': PendingChange;
  'countdown-tick': CountdownTick;
  'mode-reverted': ModeReverted;
};

/** One second between countdown ticks, as the Rust countdown sends them. */
const TICK_MS = 1000;

/** The steps of an apply that passed its test and its read-back. */
const APPLIED_STEPS: readonly StepReport[] = [
  { step: 'test', status: 'done' },
  { step: 'journal', status: 'done' },
  { step: 'apply', status: 'done' },
  { step: 'verify', status: 'done' },
];

function ok(value: unknown): Answer {
  return { ok: true, value };
}

function failed(error: IpcError): Answer {
  return { ok: false, error };
}

/** A copy through JSON, as the IPC sends it, so the window never shares an object with the mock. */
function overTheWire<T>(value: T): T {
  return JSON.parse(JSON.stringify(value)) as T;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null;
}

/** The `request` argument, or null for a command invoked without one. */
function readRequest(payload: unknown): unknown {
  return isRecord(payload) && 'request' in payload ? payload.request : null;
}

function readString(request: unknown, key: string): string | null {
  if (!isRecord(request)) {
    return null;
  }
  const value = request[key];
  return typeof value === 'string' ? value : null;
}

function isWhole(value: unknown): value is number {
  return typeof value === 'number' && Number.isInteger(value) && value >= 0;
}

function isNullableWhole(value: unknown): value is number | null {
  return value === null || isWhole(value);
}

/** A MonitorId with every field of the right type, as serde would accept it; else null. */
function readMonitorId(value: unknown): MonitorId | null {
  if (
    !isRecord(value) ||
    typeof value.devicePath !== 'string' ||
    !isNullableWhole(value.edidManufacturerId) ||
    !isNullableWhole(value.edidProductCode) ||
    !isWhole(value.connectorInstance)
  ) {
    return null;
  }
  return {
    devicePath: value.devicePath,
    edidManufacturerId: value.edidManufacturerId,
    edidProductCode: value.edidProductCode,
    connectorInstance: value.connectorInstance,
  };
}

function readRate(value: unknown): RefreshRate | null {
  if (
    !isRecord(value) ||
    !isWhole(value.numerator) ||
    !isWhole(value.denominator) ||
    typeof value.label !== 'string'
  ) {
    return null;
  }
  return { numerator: value.numerator, denominator: value.denominator, label: value.label };
}

/** A Mode of the right shape. The label is read but never trusted: the core builds its own. */
function readMode(value: unknown): Mode | null {
  if (
    !isRecord(value) ||
    !isWhole(value.width) ||
    !isWhole(value.height) ||
    typeof value.label !== 'string'
  ) {
    return null;
  }
  const refresh = readRate(value.refresh);
  if (refresh === null) {
    return null;
  }
  return { width: value.width, height: value.height, refresh, label: value.label };
}

function isScalingChoice(value: unknown): value is ScalingChoice {
  return value === 'keep' || value === 'fullScreen';
}

/** The core's own label for a requested mode, whatever label the window sent. */
function labelled(requested: Mode): Mode {
  const refreshLabel = rateLabel(requested.refresh.numerator, requested.refresh.denominator);
  return {
    width: requested.width,
    height: requested.height,
    refresh: { ...requested.refresh, label: refreshLabel },
    label: `${String(requested.width)} x ${String(requested.height)} at ${refreshLabel}`,
  };
}

function sameMonitor(a: MonitorId, b: MonitorId): boolean {
  return a.devicePath === b.devicePath;
}

function isNativeSize(monitor: Monitor, target: Mode): boolean {
  return (
    monitor.native !== null &&
    monitor.native.width === target.width &&
    monitor.native.height === target.height
  );
}

/** Tauri refuses a request it cannot read before the command runs; the facade sees INTERNAL. */
function invalidRequest(cmd: CommandName): Answer {
  return failed(internalError(`invalid args \`request\` for command \`${cmd}\``));
}

/** Values that make each message read as it would on the test PC. */
function plausibleContext(code: ErrorCode): Partial<ErrorContext> {
  const monitor = 'Primary monitor';
  const stretched = mode(1440, 1080, 240);
  const table: Partial<Record<ErrorCode, Partial<ErrorContext>>> = {
    MODE_REJECTED: { monitor, mode: stretched, current: mode(1920, 1080, 240) },
    MODE_TEST_FAILED: { monitor, mode: stretched },
    MODE_NOT_OFFERED: { monitor, mode: stretched },
    // 1280 x 960 is listed at 240 Hz and 60 Hz only.
    REFRESH_NOT_AVAILABLE: {
      monitor,
      mode: mode(1280, 960, 144),
      offeredRefresh: [rate(240), rate(60)],
    },
    MONITOR_GONE: { monitor },
    NOT_NVIDIA: { monitor, adapter: 'Other graphics adapter' },
    NVAPI_MISSING: { adapter: NVIDIA_ADAPTER },
    NVAPI_CALL_FAILED: { nvapiFunction: 'NvAPI_DISP_TryCustomDisplay', nvapiStatus: -1 },
    CUSTOM_TIMING_REFUSED: { monitor, mode: stretched, adapter: NVIDIA_ADAPTER },
    REGISTRY_VERIFY_MISMATCH: { backupMs: FIXTURE_TIME_MS },
    UAC_DECLINED: { tool: 'regExe' },
    ELEVATED_TOOL_FAILED: { tool: 'pnputil', exitCode: 1 },
    DRIVER_RESTART_REFUSED_GAME_RUNNING: { exe: 'game.exe' },
    SCALING_NOT_APPLIED: { monitor, mode: stretched },
    RECOVERY_INCOMPLETE: { stillChanged: stillChangedItems() },
    SETTINGS_CORRUPT: { settingsSource: 'lastGoodCopy' },
    PROFILE_INVALID: { profile: 'My game', profileProblem: 'invalidFields', fields: ['exe'] },
    HOTKEY_TAKEN: { hotkey: 'Ctrl+Alt+F9' },
    AUTOSTART_FAILED: { autostartEnabled: false },
  };
  return table[code] ?? {};
}

/** The steps a multi-step failure reports; the rest report none. */
const STEPS: Partial<Record<ErrorCode, StepReport[]>> = {
  NVAPI_CALL_FAILED: [
    { step: 'journal', status: 'done' },
    { step: 'nvapiTry', status: 'failed' },
    { step: 'nvapiSave', status: 'notRun' },
  ],
  REGISTRY_VERIFY_MISMATCH: [
    { step: 'backup', status: 'done' },
    { step: 'registryWrite', status: 'done' },
    { step: 'verify', status: 'failed' },
    { step: 'driverRestart', status: 'notRun' },
  ],
  UAC_DECLINED: [
    { step: 'backup', status: 'done' },
    { step: 'registryWrite', status: 'notRun' },
    { step: 'driverRestart', status: 'notRun' },
  ],
  ELEVATED_TOOL_FAILED: [
    { step: 'backup', status: 'done' },
    { step: 'registryWrite', status: 'done' },
    { step: 'driverRestart', status: 'failed' },
  ],
  DRIVER_RESTART_REFUSED_GAME_RUNNING: [
    { step: 'backup', status: 'done' },
    { step: 'registryWrite', status: 'done' },
    { step: 'driverRestart', status: 'notRun' },
  ],
  RECOVERY_INCOMPLETE: [{ step: 'restore', status: 'failed' }],
};

/** What one apply asks for once its request has been read. */
type ApplyTarget = {
  monitor: MonitorId;
  mode: Mode;
  scaling: ScalingChoice;
  profileId: string | null;
};

/**
 * The in-memory core. It follows the Rust core's rules for this build: test, then a pending
 * change for a mode not confirmed on that monitor, a countdown from revertCountdownSecs with one
 * tick a second, a revert at 0, Keep adding to confirmedModes, the revision growing by one per
 * snapshot and state-changed after every change. A command whose work belongs to a later phase
 * answers INTERNAL and changes nothing. A difference from Rust is a bug here.
 */
export class MockCore {
  private current: AppState;
  private readonly faults: Faults;
  private readonly timers = new Set<ReturnType<typeof setTimeout>>();
  private countdown: ReturnType<typeof setInterval> | null = null;
  // The scaling read back before the pending change, which a revert puts back.
  private scalingBeforePending: Scaling | null = null;
  private changeCount = 1;
  private raisedCount: number;
  private raisesWaiting: ErrorCode[];
  private isDisposed = false;

  constructor(scenario: Scenario, faults: Faults) {
    this.current = scenario.state;
    this.faults = faults;
    this.raisedCount = scenario.state.errors.length;
    this.raisesWaiting = [...faults.raise];
    // A scenario that starts with a pending change starts with its countdown running.
    const pending = scenario.state.pending;
    if (pending !== null) {
      this.scalingBeforePending = 'identity';
      this.startCountdown(pending.changeId, false);
    }
  }

  /** Answers one invoke the way Tauri does: the value, or a rejection with an IpcError. */
  dispatch(cmd: string, payload: unknown): Promise<unknown> {
    const answer = this.answer(cmd, payload);
    if (answer.ok) {
      return Promise.resolve(overTheWire(answer.value));
    }
    // eslint-disable-next-line @typescript-eslint/prefer-promise-reject-errors -- Tauri rejects an invoke with the IpcError object itself, so the mocked core does the same.
    return Promise.reject(overTheWire(answer.error));
  }

  /** Stops every timer; nothing is sent after this. */
  dispose(): void {
    this.isDisposed = true;
    this.stopCountdown();
    for (const timer of this.timers) {
      clearTimeout(timer);
    }
    this.timers.clear();
  }

  state(): Answer {
    return ok(this.current);
  }

  monitors(): Answer {
    // The mocked monitors change only through the commands below, so a fresh read sends no
    // state-changed.
    return ok(this.current.monitors);
  }

  profiles(): Answer {
    return ok(this.current.profiles);
  }

  settings(): Answer {
    return ok(this.current.settings);
  }

  /** Applies a mode or a profile through the same steps as the Rust pipeline. */
  apply(request: unknown): Answer {
    const target = this.readApplyTarget(request);
    if (target === null) {
      return invalidRequest('apply_mode');
    }
    if ('error' in target) {
      return failed(target.error);
    }
    // A pending change on the same monitor is put back before anything else (Superseded).
    if (
      this.current.pending !== null &&
      sameMonitor(this.current.pending.monitor, target.monitor)
    ) {
      this.revertPending('superseded');
    }
    const monitor = this.findMonitor(target.monitor);
    if (monitor === undefined) {
      return failed(this.natural('MONITOR_GONE', {}, 'no connected monitor matches this id'));
    }
    const resolved = this.resolve(monitor, target.mode);
    if ('error' in resolved) {
      return failed(resolved.error);
    }

    const listed = resolved.mode;
    const previous = monitor.current ?? monitor.native ?? listed;
    const previousScaling = monitor.scaling;
    const warnings = this.warningsFor('apply_mode');
    const scalingIgnored = warnings.some((warning) => warning.code === 'SCALING_NOT_APPLIED');
    let scaling: Scaling = previousScaling;
    if (isNativeSize(monitor, listed)) {
      scaling = 'identity';
    } else if (target.scaling === 'fullScreen' && !scalingIgnored) {
      scaling = 'stretched';
    }
    const steps: StepReport[] = [...APPLIED_STEPS];
    if (target.scaling === 'fullScreen' && !isNativeSize(monitor, listed)) {
      steps.push({ step: 'scaling', status: scalingIgnored ? 'failed' : 'done' });
    }

    const active = this.activeOn(target.monitor);
    const baseline = active?.before ?? previous;
    const monitors = this.withMonitor(target.monitor, (item) => monitorOn(item, listed, scaling));
    const changeId = this.nextChangeId();

    if (this.isConfirmed(monitor, listed, baseline)) {
      const change: ActiveChange = {
        changeId,
        monitor: monitor.id,
        monitorName: monitor.name,
        before: baseline,
        now: listed,
        scaling,
        turnedOff: [],
        profileId: target.profileId,
        source: 'window',
        sinceMs: FIXTURE_TIME_MS,
      };
      this.change({ ...this.current, monitors, active: this.withActive(change) });
      return ok({ kind: 'applied', change, steps, warnings });
    }

    const pending: PendingChange = {
      changeId,
      monitor: monitor.id,
      monitorName: monitor.name,
      from: previous,
      to: listed,
      scaling,
      countdownSecs: this.current.settings.revertCountdownSecs,
      secondsLeft: this.current.settings.revertCountdownSecs,
      source: 'window',
    };
    this.scalingBeforePending = previousScaling;
    this.change({ ...this.current, monitors, pending });
    this.send('mode-pending-confirm', pending);
    this.startCountdown(changeId, true);
    return ok({ kind: 'pendingConfirm', pending, steps, warnings });
  }

  /** Keeps the pending change. A change that is not pending is not an error. */
  confirm(request: unknown): Answer {
    const changeId = readString(request, 'changeId');
    if (changeId === null) {
      return invalidRequest('confirm_mode');
    }
    const pending = this.current.pending;
    if (pending?.changeId !== changeId) {
      return ok({ kind: 'notPending' });
    }
    this.stopCountdown();
    const warnings = this.warningsFor('confirm_mode');
    // Memory keeps the last saved settings when the write fails, so the mode asks again later.
    const isSaved = !warnings.some((warning) => warning.code === 'SETTINGS_WRITE_FAILED');
    const settings = this.current.settings;
    const isKnown = settings.confirmedModes.some(
      (item) => sameMonitor(item.monitor, pending.monitor) && sameMode(item.mode, pending.to),
    );
    const confirmedModes =
      isSaved && !isKnown
        ? [...settings.confirmedModes, { monitor: pending.monitor, mode: pending.to }]
        : settings.confirmedModes;
    // The kept change keeps the pending change's ID, so a window can follow it.
    const change: ActiveChange = {
      changeId: pending.changeId,
      monitor: pending.monitor,
      monitorName: pending.monitorName,
      before: this.activeOn(pending.monitor)?.before ?? pending.from,
      now: pending.to,
      scaling: pending.scaling,
      turnedOff: [],
      profileId: null,
      source: pending.source,
      sinceMs: FIXTURE_TIME_MS,
    };
    this.scalingBeforePending = null;
    this.change({
      ...this.current,
      pending: null,
      active: this.withActive(change),
      settings: { ...settings, confirmedModes },
    });
    return ok({ kind: 'kept', change, warnings });
  }

  /** Puts back the state from before the pending change, reason Player. */
  revert(request: unknown): Answer {
    const changeId = readString(request, 'changeId');
    if (changeId === null) {
      return invalidRequest('revert_mode');
    }
    if (this.current.pending?.changeId !== changeId) {
      return ok({ kind: 'notPending' });
    }
    const reverted = this.revertPending('player');
    return ok(reverted === null ? { kind: 'notPending' } : { kind: 'reverted', reverted });
  }

  /** One monitor, or every monitor ResSwap changed, back on its native size. */
  restoreNative(request: unknown): Answer {
    if (!isRecord(request) || !('monitor' in request)) {
      return invalidRequest('restore_native');
    }
    const asked = request.monitor === null ? null : readMonitorId(request.monitor);
    if (request.monitor !== null && asked === null) {
      return invalidRequest('restore_native');
    }
    const pending = this.current.pending;
    if (pending !== null && (asked === null || sameMonitor(pending.monitor, asked))) {
      this.revertPending('superseded');
    }
    let targets: Monitor[];
    if (asked === null) {
      targets = this.current.monitors.filter((item) => this.activeOn(item.id) !== undefined);
    } else {
      const found = this.findMonitor(asked);
      if (found === undefined) {
        return failed(this.natural('MONITOR_GONE', {}, 'no connected monitor matches this id'));
      }
      targets = [found];
    }

    const restored: MonitorId[] = [];
    let monitors = this.current.monitors;
    let active = this.current.active;
    for (const target of targets) {
      const change = this.activeOn(target.id);
      const native = target.native;
      if (native === null) {
        continue;
      }
      // The native size at the rate the monitor had before the first change, when offered.
      const wantedRate = change?.before.refresh ?? target.current?.refresh ?? native.refresh;
      const goal =
        target.modes.find(
          (item) =>
            item.width === native.width &&
            item.height === native.height &&
            sameRate(item.refresh, wantedRate),
        ) ?? native;
      const isThere = target.current !== null && sameMode(target.current, goal);
      if (isThere && change === undefined) {
        continue;
      }
      monitors = monitors.map((item) =>
        sameMonitor(item.id, target.id) ? monitorOn(item, goal, 'identity') : item,
      );
      active = active.filter((item) => !sameMonitor(item.monitor, target.id));
      restored.push(target.id);
    }
    if (restored.length > 0) {
      this.change({ ...this.current, monitors, active });
    }
    return ok({ restored, steps: restored.length > 0 ? [...APPLIED_STEPS] : [] });
  }

  /** Every change ResSwap made, put back to the state before its first change. */
  restoreEverything(): Answer {
    if (this.current.pending !== null) {
      this.revertPending('superseded');
    }
    const restored: MonitorId[] = [];
    let monitors = this.current.monitors;
    for (const change of this.current.active) {
      monitors = monitors.map((item) =>
        sameMonitor(item.id, change.monitor)
          ? monitorOn(
              item,
              change.before,
              isNativeSize(item, change.before) ? 'identity' : item.scaling,
            )
          : item,
      );
      restored.push(change.monitor);
    }
    const recovery = this.current.recovery;
    if (recovery.status === 'incomplete') {
      // The scenario's monitors already show their baseline, so only the record is cleared.
      for (const item of monitors) {
        const isListed = recovery.stillChanged.some((left) => left.monitorName === item.name);
        if (isListed && !restored.some((id) => sameMonitor(id, item.id))) {
          restored.push(item.id);
        }
      }
    }
    if (restored.length > 0) {
      this.change({
        ...this.current,
        monitors,
        active: [],
        recovery: { status: 'clean', stillChanged: [] },
      });
    }
    return ok({
      restored,
      steps: restored.length > 0 ? [{ step: 'restore', status: 'done' }] : [],
    });
  }

  /** Saves the fields the window may change, then sends the new state. */
  saveSettings(request: unknown): Answer {
    const input = this.readSettingsInput(request);
    if (input === null) {
      return invalidRequest('save_settings');
    }
    if (input.revertCountdownSecs < 1 || input.revertCountdownSecs > 60) {
      return failed(internalError('revertCountdownSecs is outside its allowed range'));
    }
    const settings: Settings = { ...this.current.settings, ...input };
    this.change({ ...this.current, settings });
    return ok(settings);
  }

  /** The window sends only IDs it received, so an unknown ID is INTERNAL. */
  dismissError(request: unknown): Answer {
    const errorId = readString(request, 'errorId');
    if (errorId === null) {
      return invalidRequest('dismiss_error');
    }
    const errors = this.current.errors.filter((entry) => entry.id !== errorId);
    if (errors.length === this.current.errors.length) {
      return failed(internalError('no raised error has this ID'));
    }
    this.change({ ...this.current, errors });
    return ok(null);
  }

  /** The honest answer of a command whose work this build does not hold. */
  notAvailable(cmd: CommandName): Answer {
    return failed(internalError(`${cmd} is not available in this build; nothing was changed`));
  }

  private answer(cmd: string, payload: unknown): Answer {
    // A call that is not a ResSwap command also catches a component that reached past the facade.
    if (!isCommandName(cmd)) {
      return failed(internalError(`not a ResSwap command: ${cmd}`));
    }
    if (cmd === 'get_state') {
      this.raiseAfterFirstRead();
    }
    const code = this.faults.fail.get(cmd);
    if (code !== undefined) {
      return failed(this.errorFor(code, `fail=${cmd}:${code}`));
    }
    return HANDLERS[cmd](this, readRequest(payload));
  }

  /** Reads an ApplyRequest; null when serde would refuse it, an error when the core would. */
  private readApplyTarget(request: unknown): ApplyTarget | { error: IpcError } | null {
    if (!isRecord(request)) {
      return null;
    }
    if (request.kind === 'profile') {
      const profileId = readString(request, 'profileId');
      if (profileId === null) {
        return null;
      }
      const profile = this.current.profiles.find((item) => item.id === profileId);
      if (profile === undefined) {
        return {
          error: this.natural(
            'PROFILE_INVALID',
            { profileProblem: 'notFound' },
            'no profile has this id',
          ),
        };
      }
      return {
        monitor: profile.monitor,
        mode: profile.mode,
        scaling: profile.scaling,
        profileId: profile.id,
      };
    }
    if (request.kind !== 'mode') {
      return null;
    }
    const monitor = readMonitorId(request.monitor);
    const requested = readMode(request.mode);
    if (monitor === null || requested === null || !isScalingChoice(request.scaling)) {
      return null;
    }
    // The core's own checks before it reads the PC: a size above 0 and a rate it can divide.
    if (requested.width === 0 || requested.height === 0 || requested.refresh.denominator === 0) {
      return { error: internalError('the requested mode has a zero size or a zero denominator') };
    }
    return { monitor, mode: requested, scaling: request.scaling, profileId: null };
  }

  /** The listed mode for a request: the same size at the same rational (REQ-DISP-17). */
  private resolve(monitor: Monitor, requested: Mode): { mode: Mode } | { error: IpcError } {
    const asked = labelled(requested);
    const sameSize = monitor.modes.filter(
      (item) => item.width === asked.width && item.height === asked.height,
    );
    if (sameSize.length === 0) {
      return {
        error: this.natural(
          'MODE_NOT_OFFERED',
          { monitor: monitor.name, mode: asked },
          'the size is not in the mode list',
        ),
      };
    }
    const listed = sameSize.find((item) => sameRate(item.refresh, asked.refresh));
    if (listed === undefined) {
      return {
        error: this.natural(
          'REFRESH_NOT_AVAILABLE',
          {
            monitor: monitor.name,
            mode: asked,
            offeredRefresh: sameSize.map((item) => item.refresh),
          },
          'the size is listed, but not at this rate',
        ),
      };
    }
    return { mode: listed };
  }

  /** Kept on this monitor before, its baseline mode, or its preferred mode at its own rate. */
  private isConfirmed(monitor: Monitor, target: Mode, baseline: Mode): boolean {
    return (
      (monitor.native !== null && sameMode(monitor.native, target)) ||
      sameMode(baseline, target) ||
      this.current.settings.confirmedModes.some(
        (item) => sameMonitor(item.monitor, monitor.id) && sameMode(item.mode, target),
      )
    );
  }

  private readSettingsInput(
    request: unknown,
  ): Omit<Settings, 'schemaVersion' | 'startWithWindows' | 'hotkeys' | 'confirmedModes'> | null {
    if (
      !isRecord(request) ||
      !isWhole(request.revertCountdownSecs) ||
      typeof request.checkForUpdates !== 'boolean' ||
      typeof request.showHiddenMonitors !== 'boolean' ||
      !(request.playStretchedProfile === null || typeof request.playStretchedProfile === 'string')
    ) {
      return null;
    }
    const selectedMonitor =
      request.selectedMonitor === null ? null : readMonitorId(request.selectedMonitor);
    if (request.selectedMonitor !== null && selectedMonitor === null) {
      return null;
    }
    return {
      revertCountdownSecs: request.revertCountdownSecs,
      checkForUpdates: request.checkForUpdates,
      showHiddenMonitors: request.showHiddenMonitors,
      selectedMonitor,
      playStretchedProfile: request.playStretchedProfile,
    };
  }

  /** Puts the pending change back, sends mode-reverted, and returns what it sent. */
  private revertPending(reason: RevertReason): ModeReverted | null {
    const pending = this.current.pending;
    if (pending === null) {
      return null;
    }
    this.stopCountdown();
    const scaling = this.scalingBeforePending ?? 'identity';
    this.scalingBeforePending = null;
    const monitors = this.withMonitor(pending.monitor, (item) =>
      monitorOn(item, pending.from, scaling),
    );
    const reverted: ModeReverted = {
      changeId: pending.changeId,
      monitor: pending.monitor,
      monitorName: pending.monitorName,
      reason,
      restored: pending.from,
      steps: [{ step: 'restore', status: 'done' }],
    };
    this.change({ ...this.current, monitors, pending: null });
    this.send('mode-reverted', reverted);
    return reverted;
  }

  /**
   * One tick a second from countdownSecs down to 0, then the revert (reason Timeout). The seconds
   * left travel in countdown-tick; get_state also shows them, with no state-changed per tick.
   */
  private startCountdown(changeId: string, sendFirstTick: boolean): void {
    this.stopCountdown();
    const pending = this.current.pending;
    if (pending === null) {
      return;
    }
    if (sendFirstTick) {
      this.send('countdown-tick', { changeId, secondsLeft: pending.secondsLeft });
    }
    this.countdown = setInterval(() => {
      this.tick(changeId);
    }, TICK_MS);
  }

  private tick(changeId: string): void {
    const pending = this.current.pending;
    if (this.isDisposed || pending?.changeId !== changeId) {
      this.stopCountdown();
      return;
    }
    const secondsLeft = Math.max(pending.secondsLeft - 1, 0);
    this.current = { ...this.current, pending: { ...pending, secondsLeft } };
    this.send('countdown-tick', { changeId, secondsLeft });
    if (secondsLeft === 0) {
      this.revertPending('timeout');
    }
  }

  private stopCountdown(): void {
    if (this.countdown !== null) {
      clearInterval(this.countdown);
      this.countdown = null;
    }
  }

  private findMonitor(id: MonitorId): Monitor | undefined {
    return this.current.monitors.find((item) => sameMonitor(item.id, id));
  }

  private activeOn(id: MonitorId): ActiveChange | undefined {
    return this.current.active.find((item) => sameMonitor(item.monitor, id));
  }

  /** The active list with this change replacing any earlier one on the same monitor. */
  private withActive(change: ActiveChange): ActiveChange[] {
    return [
      ...this.current.active.filter((item) => !sameMonitor(item.monitor, change.monitor)),
      change,
    ];
  }

  private withMonitor(id: MonitorId, update: (monitor: Monitor) => Monitor): Monitor[] {
    return this.current.monitors.map((item) => (sameMonitor(item.id, id) ? update(item) : item));
  }

  private nextChangeId(): string {
    this.changeCount += 1;
    return `mock-change-${String(this.changeCount)}`;
  }

  private warningsFor(cmd: WarningCommand): IpcError[] {
    return (this.faults.warn.get(cmd) ?? []).map((code) =>
      this.errorFor(code, `warn=${cmd}:${code}`),
    );
  }

  /** An error the mocked core finds by itself, with the context the Rust core would send. */
  private natural(code: ErrorCode, context: Partial<ErrorContext>, detail: string): IpcError {
    return {
      code,
      context: { ...emptyContext(), ...context },
      steps: [],
      detail: `Mocked core: ${detail}`,
    };
  }

  private errorFor(code: ErrorCode, trigger: string): IpcError {
    return {
      code,
      context: { ...emptyContext(), ...plausibleContext(code) },
      steps: STEPS[code] ?? [],
      detail: `Mocked core: ${trigger}`,
    };
  }

  /** The window has subscribed once it reads the state, so a raised error now reaches it. */
  private raiseAfterFirstRead(): void {
    const codes = this.raisesWaiting;
    this.raisesWaiting = [];
    if (codes.length > 0) {
      this.later(() => {
        for (const code of codes) {
          this.raise(code, 'startup');
        }
      });
    }
  }

  /** Repeats of one code from one source collapse into one entry, as in the Rust core. */
  private raise(code: ErrorCode, source: ErrorSource): void {
    const error = this.errorFor(code, `raise=${code}`);
    const repeat = this.current.errors.find(
      (entry) => entry.source === source && entry.error.code === code,
    );
    let entry: RaisedError;
    if (repeat === undefined) {
      this.raisedCount += 1;
      entry = {
        id: `error-${String(this.raisedCount)}`,
        source,
        atMs: FIXTURE_TIME_MS,
        count: 1,
        error,
      };
    } else {
      entry = { ...repeat, count: repeat.count + 1, error };
    }
    const others = this.current.errors.filter((item) => item.id !== entry.id);
    this.current = {
      ...this.current,
      errors: [entry, ...others],
      revision: this.current.revision + 1,
    };
    this.send('error-raised', entry);
    this.send('state-changed', this.current);
  }

  /** Every change gets a new revision, because the window keeps only the highest it has seen. */
  private change(next: AppState): void {
    this.current = { ...next, revision: this.current.revision + 1 };
    this.send('state-changed', this.current);
  }

  private send<E extends keyof EventPayloads>(name: E, payload: EventPayloads[E]): void {
    if (!this.isDisposed) {
      void emit(name, overTheWire(payload));
    }
  }

  private later(run: () => void): void {
    const timer = setTimeout(() => {
      this.timers.delete(timer);
      if (!this.isDisposed) {
        run();
      }
    }, 0);
    this.timers.add(timer);
  }
}
