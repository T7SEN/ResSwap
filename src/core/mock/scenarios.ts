import { emptyContext, isErrorCode } from '../errors';
import type { AppState } from '../types/AppState';
import type { CommandName } from '../types/CommandName';
import type { ErrorCode } from '../types/ErrorCode';
import {
  baseState,
  FIXTURE_TIME_MS,
  mode,
  primaryMonitor,
  stillChangedItems,
  stretchedChange,
  twoProfiles,
} from './fixtures';
import { isCommandName } from './handlers';

/** A starting state and the ID that names it. */
export type Scenario = {
  id: string;
  state: AppState;
};

/** What the URL asks the mocked core to do besides its starting state. */
export type Faults = {
  /** fail=<command>:<CODE>: that command returns that error on every call; its first entry wins. */
  fail: ReadonlyMap<CommandName, ErrorCode>;
  /** raise=<CODE>: error-raised fires once after the window first reads the state. */
  raise: readonly ErrorCode[];
};

type WindowLabel = 'main' | 'keep-mode';

/** Everything the URL parameters ask for, and what in them could not be used. */
export type MockParams = {
  label: WindowLabel;
  scenario: Scenario;
  faults: Faults;
  /** Shown in a banner, so a typo in a test URL fails the test instead of passing quietly. */
  problems: string[];
};

/** The scenario a URL without `scenario` starts. */
export const DEFAULT_SCENARIO = 'home-native';

function homeNative(): AppState {
  return { ...baseState(), profiles: twoProfiles() };
}

// The scenarios this build can show. The rest of the contract's list arrives with the screens
// and behaviour that need it; until then their IDs are reported as unknown.
const SCENARIOS: Readonly<Record<string, () => AppState>> = {
  'first-run': () => baseState(),
  'home-native': homeNative,
  'home-stretched': () => {
    const state = baseState();
    return {
      ...state,
      profiles: twoProfiles(),
      monitors: [primaryMonitor(mode(1280, 960, 240)), ...state.monitors.slice(1)],
      active: [stretchedChange()],
    };
  },
  'hidden-monitors': () => {
    const state = baseState();
    return { ...state, settings: { ...state.settings, showHiddenMonitors: true } };
  },
  'recovery-incomplete': () => {
    const state = baseState();
    const stillChanged = stillChangedItems();
    return {
      ...state,
      recovery: { status: 'incomplete', stillChanged },
      errors: [
        {
          id: 'error-1',
          source: 'startup',
          atMs: FIXTURE_TIME_MS,
          count: 1,
          error: {
            code: 'RECOVERY_INCOMPLETE',
            context: { ...emptyContext(), stillChanged },
            steps: [{ step: 'restore', status: 'failed' }],
            detail: 'Mocked core: recovery left two items changed',
          },
        },
      ],
    };
  },
  'fake-backend': () => {
    const state = baseState();
    return { ...state, build: { ...state.build, debug: true, backend: 'fake' } };
  },
};

/** The scenario IDs this build knows, in a stable order for the banner. */
export function scenarioIds(): string[] {
  return Object.keys(SCENARIOS).sort();
}

function scenarioFor(id: string): Scenario | undefined {
  const build = Object.hasOwn(SCENARIOS, id) ? SCENARIOS[id] : undefined;
  return build === undefined ? undefined : { id, state: build() };
}

/** Reads `scenario`, `window`, `fail`, `warn` and `raise` from the page's URL. */
export function readParams(params: URLSearchParams): MockParams {
  const problems: string[] = [];

  const requested = params.get('scenario') ?? DEFAULT_SCENARIO;
  let scenario = scenarioFor(requested);
  if (scenario === undefined) {
    problems.push(`Unknown scenario "${requested}". Valid scenarios: ${scenarioIds().join(', ')}.`);
    scenario = { id: DEFAULT_SCENARIO, state: homeNative() };
  }

  const windowParam = params.get('window') ?? 'main';
  let label: WindowLabel = 'main';
  if (windowParam === 'main' || windowParam === 'keep-mode') {
    label = windowParam;
  } else {
    problems.push(`Unknown window "${windowParam}". Valid windows: main, keep-mode.`);
  }

  const fail = new Map<CommandName, ErrorCode>();
  for (const value of params.getAll('fail')) {
    const [command = '', code = ''] = value.split(':');
    if (isCommandName(command) && isErrorCode(code)) {
      // The first entry for a command wins, as in the Rust core's fault list.
      if (!fail.has(command)) {
        fail.set(command, code);
      }
    } else {
      problems.push(`Cannot use fail=${value}. The form is fail=<command>:<CODE>.`);
    }
  }

  // No command of this build succeeds with a warnings list, so a warning has nowhere to go.
  for (const value of params.getAll('warn')) {
    problems.push(`Cannot use warn=${value}: no command in this build returns warnings.`);
  }

  const raise: ErrorCode[] = [];
  for (const value of params.getAll('raise')) {
    if (isErrorCode(value)) {
      raise.push(value);
    } else {
      problems.push(`Cannot use raise=${value}. The form is raise=<CODE>.`);
    }
  }

  return { label, scenario, faults: { fail, raise }, problems };
}
