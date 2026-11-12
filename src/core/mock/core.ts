import { emit } from '@tauri-apps/api/event';
import { emptyContext, internalError } from '../errors';
import type { AppState } from '../types/AppState';
import type { CommandName } from '../types/CommandName';
import type { ErrorCode } from '../types/ErrorCode';
import type { ErrorContext } from '../types/ErrorContext';
import type { ErrorSource } from '../types/ErrorSource';
import type { IpcError } from '../types/IpcError';
import type { RaisedError } from '../types/RaisedError';
import type { StepReport } from '../types/StepReport';
import { FIXTURE_TIME_MS, mode, NVIDIA_ADAPTER, rate, stillChangedItems } from './fixtures';
import { HANDLERS, isCommandName } from './handlers';
import type { Faults, Scenario } from './scenarios';

/** A handler's answer: the value the command returns, or the error it rejects with. */
export type Answer = { ok: true; value: unknown } | { ok: false; error: IpcError };

type EventPayloads = {
  'state-changed': AppState;
  'error-raised': RaisedError;
};

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

/**
 * The in-memory core. It follows the Rust core's rules for this build: the revision grows by
 * one per snapshot, state-changed follows every change, and a command whose work belongs to a
 * later phase answers INTERNAL and changes nothing. A difference from Rust is a bug here.
 */
export class MockCore {
  private current: AppState;
  private readonly faults: Faults;
  private readonly timers = new Set<ReturnType<typeof setTimeout>>();
  private raisedCount: number;
  private raisesWaiting: ErrorCode[];
  private isDisposed = false;

  constructor(scenario: Scenario, faults: Faults) {
    this.current = scenario.state;
    this.faults = faults;
    this.raisedCount = scenario.state.errors.length;
    this.raisesWaiting = [...faults.raise];
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
    for (const timer of this.timers) {
      clearTimeout(timer);
    }
    this.timers.clear();
  }

  state(): Answer {
    return ok(this.current);
  }

  monitors(): Answer {
    // The mocked monitors never change, so a fresh read sends no state-changed.
    return ok(this.current.monitors);
  }

  profiles(): Answer {
    return ok(this.current.profiles);
  }

  settings(): Answer {
    return ok(this.current.settings);
  }

  /** A change that is not pending is not an error: the window shows the revert instead. */
  confirm(request: unknown): Answer {
    const changeId = readString(request, 'changeId');
    if (changeId === null) {
      return invalidRequest('confirm_mode');
    }
    if (this.current.pending?.changeId === changeId) {
      return this.notAvailable('confirm_mode');
    }
    return ok({ kind: 'notPending' });
  }

  revert(request: unknown): Answer {
    const changeId = readString(request, 'changeId');
    if (changeId === null) {
      return invalidRequest('revert_mode');
    }
    if (this.current.pending?.changeId === changeId) {
      return this.notAvailable('revert_mode');
    }
    return ok({ kind: 'notPending' });
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
