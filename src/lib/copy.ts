import type { ActiveChange } from '@/core/types/ActiveChange';
import type { Aspect } from '@/core/types/Aspect';
import type { BusyAction } from '@/core/types/BusyAction';
import type { ChangeSource } from '@/core/types/ChangeSource';
import type { ErrorSource } from '@/core/types/ErrorSource';
import type { ModeReverted } from '@/core/types/ModeReverted';
import type { PendingChange } from '@/core/types/PendingChange';
import type { RevertReason } from '@/core/types/RevertReason';
import type { Scaling } from '@/core/types/Scaling';
import type { StepKind } from '@/core/types/StepKind';
import type { StepStatus } from '@/core/types/StepStatus';

// Fixed UI text and the labels for core values, kept in one place so every screen shows the
// same words (REQ-UI-10, REQ-UI-11). Error messages live in src/core/errors.ts only.

/** The keep-mode window's title, the same text its window configuration sets. */
export const KEEP_MODE_TITLE = 'ResSwap: keep this resolution?';

/** Shown where Windows reports no name for a monitor. */
export const UNNAMED_MONITOR = 'Unnamed monitor';

export const STEP_LABELS: Record<StepKind, string> = {
  test: 'Test the mode',
  journal: 'Save the undo record',
  backup: 'Back up the NVIDIA mode list',
  apply: 'Apply the mode',
  verify: 'Read back the result',
  scaling: 'Set full-screen scaling',
  layout: 'Change the monitor layout',
  nvapiTry: 'Try the custom resolution',
  nvapiSave: 'Save the custom resolution',
  nvapiDelete: 'Delete the custom resolution',
  registryWrite: 'Write the NVIDIA mode list (reg.exe, administrator)',
  driverRestart: 'Restart the graphics driver (pnputil.exe, administrator)',
  restore: 'Put back the previous state',
  settingsWrite: 'Save settings',
  download: 'Download the update',
  signatureCheck: "Check the update's signature",
  install: 'Start the installer',
};

export const STEP_STATUS_LABELS: Record<StepStatus, string> = {
  done: 'Done',
  failed: 'Failed',
  notRun: 'Not run',
  undone: 'Put back',
};

export const BUSY_LABELS: Record<BusyAction, string> = {
  applying: 'Applying the mode',
  reverting: 'Putting back the previous mode',
  restoring: 'Restoring',
  addingCustom: 'Adding the custom resolution',
  removingCustom: 'Removing the custom resolution',
  undoing: "Undoing ResSwap's changes",
  installingUpdate: 'Installing the update',
};

/** The words after "Changed" in the active change line. */
export const CHANGE_SOURCE_LABELS: Record<ChangeSource, string> = {
  window: 'from this window',
  tray: 'from the tray',
  hotkey: 'by hotkey',
  watcher: 'by auto-switch',
  playShortcut: 'by a Play shortcut',
  recovery: 'during recovery',
  update: 'before an update',
};

export const ERROR_SOURCE_LABELS: Record<ErrorSource, string> = {
  watcher: 'Auto-switch',
  hotkey: 'Hotkey',
  tray: 'Tray menu',
  playShortcut: 'Play shortcut',
  startup: 'Start-up',
  resume: 'After sleep',
  displayChange: 'Display change',
  updateCheck: 'Update check',
};

export const SCALING_LABELS: Record<Scaling, string> = {
  identity: 'No scaling',
  centered: 'Centred',
  stretched: 'Full screen',
  aspectRatio: 'Keep aspect ratio',
  custom: 'Custom',
  preferred: 'Driver default',
  unknown: 'Unknown',
};

export const ASPECT_LABELS: Record<Aspect, string> = {
  fourThree: '4:3',
  fiveFour: '5:4',
  sixteenTen: '16:10',
};

/**
 * A monitor name inside a sentence. A missing name falls back as the error placeholders do,
 * capitalised when it starts the sentence.
 */
export function monitorPhrase(name: string | null, atStart: boolean): string {
  if (name !== null) {
    return name;
  }
  return atStart ? 'This monitor' : 'this monitor';
}

/** "10 seconds", or "1 second". */
export function secondsPhrase(seconds: number): string {
  return seconds === 1 ? '1 second' : `${String(seconds)} seconds`;
}

/** The disabled reason while a change runs. */
export function busyReason(action: BusyAction): string {
  return `ResSwap is busy: ${BUSY_LABELS[action]}.`;
}

/** apply_mode answered `applied`. */
export function appliedLine(change: ActiveChange): string {
  return `${monitorPhrase(change.monitorName, true)} is now on ${change.now.label}. Restore native puts back ${change.before.label}.`;
}

/** apply_mode answered `pendingConfirm`. */
export function pendingLine(pending: PendingChange): string {
  return `${monitorPhrase(pending.monitorName, true)} is on ${pending.to.label} for now. Keep it, or ResSwap puts back ${pending.from.label} in ${secondsPhrase(pending.countdownSecs)}.`;
}

/** confirm_mode answered `kept`. */
export function keptLine(change: ActiveChange): string {
  return `Kept ${change.now.label} on ${monitorPhrase(change.monitorName, false)}. ResSwap will not ask again for this mode on this monitor.`;
}

const REVERT_LINES: Record<RevertReason, (restored: string, monitor: string) => string> = {
  timeout: (restored, monitor) =>
    `The countdown ended, so ResSwap put back ${restored} on ${monitor}.`,
  player: (restored, monitor) => `ResSwap put back ${restored} on ${monitor}.`,
  keepWindowClosed: (restored, monitor) =>
    `The prompt was closed, so ResSwap put back ${restored} on ${monitor}.`,
  superseded: (restored, monitor) =>
    `ResSwap put back ${restored} on ${monitor} before applying the new mode.`,
  quit: (restored, monitor) => `ResSwap put back ${restored} on ${monitor} before it closed.`,
};

/** The line for one mode-reverted payload. */
export function revertLine(reverted: ModeReverted): string {
  return REVERT_LINES[reverted.reason](
    reverted.restored.label,
    monitorPhrase(reverted.monitorName, false),
  );
}

/** restore_native, one line per restored monitor. */
export function restoredLine(monitorName: string | null, modeLabel: string): string {
  return `${monitorPhrase(monitorName, true)} is back on ${modeLabel}.`;
}

export const NOTHING_TO_RESTORE = 'Nothing to restore.';

/** In Presets for a monitor no request can name (REQ-DISP-26). */
export const NO_IDENTITY_REASON =
  'ResSwap cannot change this monitor, because Windows does not say which monitor it is.';
export const RESTORED_EVERYTHING = 'ResSwap put back everything it changed.';
export const NO_OPEN_CHANGES = 'Nothing to restore. ResSwap has no open changes.';
export const SAVED = 'Saved.';
export const FAKE_BACKEND_NOTICE =
  'Test build: ResSwap is using its fake display backend, so nothing on this PC changes.';
export const UAC_WAITING = 'Waiting for the administrator prompt. Answer it to continue.';

/** The line under the monitor facts while ResSwap has a change open on it. */
export function changedLine(change: ActiveChange): string {
  // The player's own locale and time zone, hours and minutes.
  const time = new Intl.DateTimeFormat(undefined, { timeStyle: 'short' }).format(
    new Date(change.sinceMs),
  );
  return `Changed ${CHANGE_SOURCE_LABELS[change.source]} at ${time}. Restore native puts back ${change.before.label}.`;
}
