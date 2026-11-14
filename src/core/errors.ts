import type { ElevatedTool } from './types/ElevatedTool';
import type { ErrorCode } from './types/ErrorCode';
import type { ErrorContext } from './types/ErrorContext';
import type { IpcError } from './types/IpcError';
import type { ProfileField } from './types/ProfileField';

/** What a message may know besides the error itself. */
export type MessageHints = {
  /** True when an NVIDIA adapter drives the monitor the error is about. */
  monitorOnNvidia: boolean;
};

type MessageInput = {
  c: ErrorContext;
  /** True when the error carries a step list, which the window shows under the message. */
  hasSteps: boolean;
  hints: MessageHints;
  /** True for an INTERNAL the window built itself, which no log line records. */
  fromWindow: boolean;
};

/**
 * Starts the detail of an INTERNAL the window builds itself: a request Tauri refused before
 * any command ran, or an answer the window could not read. The window cannot write to the log
 * (D-003), so the message for these points at the details instead (REQ-UI-09).
 */
export const WINDOW_DETAIL_PREFIX = 'In the window: ';

type Message = (input: MessageInput) => string;

const NO_HINTS: MessageHints = { monitorOnNvidia: false };

/** Said only when the step list under the message is not empty (REQ-SAFE-17). */
const STEPS_SENTENCE = 'The steps below show what finished.';

// The labels a profile field has on screen, used in the PROFILE_INVALID message.
const FIELD_LABELS: Record<ProfileField, string> = {
  name: 'Name',
  exe: 'Game file',
  monitor: 'Monitor',
  mode: 'Resolution',
  restoreDelay: 'Restore delay',
  turnOff: 'Monitors to turn off',
  launchPath: 'Play shortcut target',
};

const TOOL_NAMES: Record<ElevatedTool, string> = {
  regExe: 'reg.exe',
  pnputil: 'pnputil.exe',
};

/*
 * Placeholders. A fallback that starts a sentence is capitalised; a value from the core keeps
 * its own spelling, because it is a name (a monitor's name, a file name such as reg.exe).
 */

function monitor(c: ErrorContext, atStart = false): string {
  return c.monitor ?? (atStart ? 'This monitor' : 'this monitor');
}

function mode(c: ErrorContext, atStart = false): string {
  return c.mode?.label ?? (atStart ? 'This mode' : 'this mode');
}

function size(c: ErrorContext): string {
  return c.mode === null ? 'this size' : `${String(c.mode.width)} x ${String(c.mode.height)}`;
}

function refresh(c: ErrorContext): string {
  return c.mode?.refresh.label ?? 'this refresh rate';
}

function current(c: ErrorContext): string {
  return c.current?.label ?? 'the mode it had before';
}

function adapter(c: ErrorContext): string {
  return c.adapter ?? 'another graphics card';
}

function tool(c: ErrorContext, atStart = false): string {
  if (c.tool !== null) {
    return TOOL_NAMES[c.tool];
  }
  return atStart ? 'The Windows tool' : 'the Windows tool';
}

function profile(c: ErrorContext): string {
  return c.profile === null ? 'this profile' : `the profile "${c.profile}"`;
}

function profileProblem(c: ErrorContext): string {
  switch (c.profileProblem) {
    case 'notFound':
      return 'it no longer exists';
    case 'invalidFields':
      return `these fields need fixing: ${c.fields.map((field) => FIELD_LABELS[field]).join(', ')}`;
    case null:
      return 'it is incomplete';
  }
}

function backupTime(c: ErrorContext): string {
  if (c.backupMs === null) {
    return 'before the change';
  }
  // The player's own locale and time zone, as every time on screen.
  const format = new Intl.DateTimeFormat(undefined, { dateStyle: 'medium', timeStyle: 'short' });
  return format.format(new Date(c.backupMs));
}

function settingsSource(c: ErrorContext): string {
  return c.settingsSource === 'lastGoodCopy' ? 'the last good copy' : 'the default settings';
}

function autostartState(c: ErrorContext): string {
  if (c.autostartEnabled === null) {
    return 'unchanged';
  }
  return c.autostartEnabled ? 'on' : 'off';
}

function steps(input: MessageInput): string {
  return input.hasSteps ? STEPS_SENTENCE : '';
}

/** Joins the sentences that apply, leaving out the empty ones. */
function sentences(...parts: string[]): string {
  return parts.filter((part) => part !== '').join(' ');
}

/**
 * The message for every error code, word for word as the IPC contract lists it (REQ-UI-08).
 * A missing or unknown code is a type error.
 */
export const ERROR_MESSAGES = {
  MODE_REJECTED: ({ c }) =>
    `Windows did not accept ${mode(c)} on ${monitor(c)}. The display is on ${current(c)}.`,
  MODE_TEST_FAILED: ({ c }) =>
    `${mode(c, true)} did not pass the test on ${monitor(c)}, so ResSwap did not apply it. Nothing changed.`,
  MODE_NOT_OFFERED: ({ c, hints }) =>
    sentences(
      `Windows does not offer ${size(c)} on ${monitor(c)}. Nothing changed.`,
      hints.monitorOnNvidia ? 'On an NVIDIA card you can add it under Custom resolutions.' : '',
    ),
  REFRESH_NOT_AVAILABLE: ({ c }) =>
    sentences(
      `${monitor(c, true)} offers ${size(c)} but not at ${refresh(c)}. Nothing changed.`,
      c.offeredRefresh.length === 0
        ? ''
        : `Pick one of the rates it offers: ${c.offeredRefresh.map((rate) => rate.label).join(', ')}.`,
    ),
  MONITOR_GONE: ({ c }) =>
    `${monitor(c, true)} is not connected any more, so ResSwap cannot change it. Reconnect it or choose another monitor.`,
  LAYOUT_RESTORE_FAILED: () =>
    'ResSwap could not turn your other monitors back on as they were. Use Restore everything, or set them up again in Windows display settings.',
  NOT_NVIDIA: ({ c }) =>
    `Custom resolutions need an NVIDIA graphics card. ${monitor(c, true)} runs on ${adapter(c)}. Everything else in ResSwap still works.`,
  NVAPI_MISSING: () =>
    'ResSwap found an NVIDIA card but could not load its driver interface (nvapi64.dll). Install or repair the NVIDIA driver, then try again. Nothing changed.',
  NVAPI_CALL_FAILED: (input) =>
    sentences(
      `The NVIDIA driver returned an error (${input.c.nvapiFunction ?? 'an NVIDIA call'}, status ${input.c.nvapiStatus === null ? 'unknown' : String(input.c.nvapiStatus)}).`,
      steps(input),
    ),
  CUSTOM_TIMING_REFUSED: ({ c }) =>
    `The NVIDIA driver refused ${size(c)} as a custom resolution on ${monitor(c)}. Nothing was added.`,
  NV_MODES_PARSE_FAILED: () =>
    'ResSwap could not read the NVIDIA mode list in the registry (NV_Modes), so it did not change it. Nothing changed.',
  REGISTRY_READ_FAILED: () =>
    'ResSwap could not read the graphics settings in the registry. Nothing changed.',
  REGISTRY_VERIFY_MISMATCH: ({ c }) =>
    `Windows reported the registry change as done, but the value ResSwap read back is different. ResSwap stopped before restarting the driver. The backup from ${backupTime(c)} is kept, and Undo can put it back.`,
  UAC_DECLINED: (input) =>
    sentences(
      `The administrator prompt was declined, so Windows did not run ${tool(input.c)}.`,
      steps(input),
    ),
  ELEVATED_TOOL_FAILED: (input) =>
    sentences(
      `${tool(input.c, true)} ran with administrator rights but reported an error (exit code ${input.c.exitCode === null ? 'unknown' : String(input.c.exitCode)}).`,
      steps(input),
    ),
  DRIVER_RESTART_NEEDS_REBOOT: () =>
    'The new resolution is saved, but the graphics driver needs a Windows restart before it appears. ResSwap did not restart Windows.',
  DRIVER_RESTART_REFUSED_GAME_RUNNING: (input) =>
    sentences(
      `ResSwap did not restart the graphics driver because ${input.c.exe ?? 'a watched game'} is running, and a driver restart can make a running game stop working. Close the game and try again.`,
      steps(input),
    ),
  SCALING_NOT_APPLIED: () =>
    "The mode changed, but the graphics driver did not switch to full-screen scaling, so the picture may show black bars. Check the scaling setting in your graphics driver's control panel.",
  JOURNAL_WRITE_FAILED: () =>
    'ResSwap could not write its undo record to disk, so it made no change. Check that the drive has free space.',
  RECOVERY_INCOMPLETE: () =>
    'ResSwap could not put everything back. The list below shows what is still changed. Try Restore everything again.',
  SETTINGS_CORRUPT: ({ c }) =>
    `ResSwap's settings file was damaged, so it loaded ${settingsSource(c)} instead. Check your profiles and settings.`,
  SETTINGS_WRITE_FAILED: () =>
    'ResSwap could not save your settings to disk, so the last saved settings stay in use. Check that the drive has free space.',
  PROFILE_INVALID: ({ c }) =>
    `ResSwap cannot use ${profile(c)}: ${profileProblem(c)}. Nothing changed.`,
  SHORTCUT_FAILED: () =>
    'ResSwap could not create the Play shortcut on your desktop. No shortcut was created.',
  HOTKEY_TAKEN: ({ c }) =>
    `${c.hotkey ?? 'This combination'} is already in use by another app. Choose a different combination.`,
  AUTOSTART_FAILED: ({ c }) =>
    `ResSwap could not change Start with Windows. It is still ${autostartState(c)}.`,
  UPDATE_CHECK_FAILED: () =>
    'ResSwap could not get update information from GitHub. Nothing was downloaded or installed. Try again later.',
  UPDATE_SIGNATURE_INVALID: () =>
    'The downloaded update failed the signature check, so ResSwap did not install it. Your current version is unchanged.',
  INTERNAL: ({ fromWindow }) =>
    fromWindow
      ? 'Something went wrong inside ResSwap. The details below show what failed.'
      : 'Something went wrong inside ResSwap. The log has the details.',
} satisfies Record<ErrorCode, Message>;

/** True for the 29 code strings the core sends, and for nothing else. */
export function isErrorCode(value: unknown): value is ErrorCode {
  return typeof value === 'string' && Object.hasOwn(ERROR_MESSAGES, value);
}

/** The message the window shows for an error. Placeholders fall back as the contract says. */
export function errorMessage(error: IpcError, hints: MessageHints = NO_HINTS): string {
  return ERROR_MESSAGES[error.code]({
    c: error.context,
    hasSteps: error.steps.length > 0,
    hints,
    fromWindow: error.code === 'INTERNAL' && error.detail.startsWith(WINDOW_DETAIL_PREFIX),
  });
}

/** A context with every value absent, for errors that need none. */
export function emptyContext(): ErrorContext {
  return {
    monitor: null,
    mode: null,
    current: null,
    offeredRefresh: [],
    adapter: null,
    nvapiFunction: null,
    nvapiStatus: null,
    tool: null,
    exitCode: null,
    exe: null,
    profile: null,
    profileProblem: null,
    fields: [],
    hotkey: null,
    backupMs: null,
    settingsSource: null,
    autostartEnabled: null,
    stillChanged: [],
  };
}

/** INTERNAL with the given detail: the code for a failure no other code describes (REQ-UI-09). */
export function internalError(detail: string): IpcError {
  return { code: 'INTERNAL', context: emptyContext(), steps: [], detail };
}

/** INTERNAL built in the window from text that never reached the core's log. */
export function windowError(raw: string): IpcError {
  return internalError(`${WINDOW_DETAIL_PREFIX}${raw}`);
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null;
}

/** The raw rejection as text, for the Details fold and the log. */
function rawText(raw: unknown): string {
  if (typeof raw === 'string') {
    return raw;
  }
  if (raw instanceof Error) {
    return `${raw.name}: ${raw.message}`;
  }
  try {
    // JSON.stringify gives undefined for a function or for undefined itself.
    const text = JSON.stringify(raw) as string | undefined;
    return text ?? String(raw);
  } catch {
    return String(raw);
  }
}

/**
 * Turns whatever a command call rejected with into an IpcError. An object with a known code is the
 * core's own error; anything else, such as the string Tauri sends for a missing permission or a
 * request it could not read, becomes INTERNAL with the raw text in the detail, marked as built in
 * the window, because no log line records it.
 */
export function toIpcError(raw: unknown): IpcError {
  if (isRecord(raw) && isErrorCode(raw.code) && isRecord(raw.context)) {
    return {
      code: raw.code,
      // Any field the core left out reads as absent, so a message never fails on it.
      context: { ...emptyContext(), ...(raw.context as Partial<ErrorContext>) },
      steps: Array.isArray(raw.steps) ? (raw.steps as IpcError['steps']) : [],
      detail: typeof raw.detail === 'string' ? raw.detail : '',
    };
  }
  return windowError(rawText(raw));
}
