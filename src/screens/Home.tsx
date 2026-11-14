import { type ReactNode, useEffect, useEffectEvent, useId, useState } from 'react';
import { ActionButton } from '@/components/ActionButton';
import { Announcer } from '@/components/Announcer';
import { ErrorAlert } from '@/components/ErrorAlert';
import { applyMode, restoreEverything, restoreNative, saveSettings } from '@/core/client';
import { errorMessage, type MessageHints } from '@/core/errors';
import { hasIdentity, monitorKey, sameMonitor } from '@/core/monitorId';
import type { ActiveChange } from '@/core/types/ActiveChange';
import type { AppState } from '@/core/types/AppState';
import type { BusyAction } from '@/core/types/BusyAction';
import type { IpcError } from '@/core/types/IpcError';
import type { Mode } from '@/core/types/Mode';
import type { ModeReverted } from '@/core/types/ModeReverted';
import type { Monitor } from '@/core/types/Monitor';
import type { MonitorId } from '@/core/types/MonitorId';
import type { PendingChange } from '@/core/types/PendingChange';
import type { Preset } from '@/core/types/Preset';
import type { Settings } from '@/core/types/Settings';
import type { SettingsInput } from '@/core/types/SettingsInput';
import { useLastReverted } from '@/core/useCore';
import {
  appliedLine,
  ASPECT_LABELS,
  BUSY_LABELS,
  busyReason,
  changedLine,
  ERROR_SOURCE_LABELS,
  FAKE_BACKEND_NOTICE,
  keptLine,
  NO_IDENTITY_REASON,
  NO_OPEN_CHANGES,
  NOTHING_TO_RESTORE,
  pendingLine,
  RESTORED_EVERYTHING,
  restoredLine,
  revertLine,
  SAVED,
  SCALING_LABELS,
  STEP_LABELS,
  UAC_WAITING,
  UNNAMED_MONITOR,
} from '@/lib/copy';
import { type Tone, useAnnouncer } from '@/lib/useAnnouncer';

/** Where an outcome shows: under the action that produced it. */
type Place = 'presets' | 'custom' | 'restore';

/** What a command answered, kept only until the next action. */
type OutcomeResult =
  | { kind: 'applied'; change: ActiveChange; warnings: IpcError[] }
  | { kind: 'pending'; pending: PendingChange; warnings: IpcError[] }
  | { kind: 'restoredNative'; restored: MonitorId[]; startRevision: number }
  | { kind: 'restoredEverything'; restored: MonitorId[] }
  | { kind: 'failed'; error: IpcError };

type Outcome = { place: Place; result: OutcomeResult };

type Running = { key: string; action: BusyAction };

type Line = { text: string; tone: Tone };

// A u32 on the Rust side, so a larger number could never reach the core.
const MAX_SIZE = 4_294_967_295;

function visibleMonitors(state: AppState): Monitor[] {
  return state.monitors.filter((monitor) => !monitor.hidden || state.settings.showHiddenMonitors);
}

/** The monitor Home shows: the chosen one, else the primary one, else the first. */
function shownMonitor(state: AppState): Monitor | undefined {
  const visible = visibleMonitors(state);
  const selected = state.settings.selectedMonitor;
  const chosen =
    selected === null ? undefined : visible.find((monitor) => sameMonitor(monitor.id, selected));
  return chosen ?? visible.find((monitor) => monitor.primary) ?? visible[0];
}

function monitorName(monitor: Monitor): string {
  return monitor.name ?? UNNAMED_MONITOR;
}

function monitorLine(monitor: Monitor | undefined): string {
  if (monitor === undefined) {
    return 'ResSwap found no monitor it can change.';
  }
  // The label comes from the core; the window never builds one.
  return monitor.current === null
    ? `${monitorName(monitor)} is connected but turned off.`
    : `${monitorName(monitor)} is on ${monitor.current.label}.`;
}

/** The fields save_settings takes, from the settings the core holds. */
function settingsInput(settings: Settings): SettingsInput {
  return {
    revertCountdownSecs: settings.revertCountdownSecs,
    checkForUpdates: settings.checkForUpdates,
    showHiddenMonitors: settings.showHiddenMonitors,
    selectedMonitor: settings.selectedMonitor,
    playStretchedProfile: settings.playStretchedProfile,
  };
}

/**
 * The line an outcome shows now. A pending change follows the core: the revert it reported, or
 * the kept change in a newer snapshot. A restore line waits for the snapshot that follows it, so
 * the mode it names is the one the core read back.
 */
function outcomeLine(
  result: OutcomeResult,
  state: AppState,
  reverted: ModeReverted | null,
  hints: MessageHints,
): Line | null {
  switch (result.kind) {
    case 'applied':
      return { text: appliedLine(result.change), tone: 'polite' };
    case 'pending': {
      const changeId = result.pending.changeId;
      if (reverted?.changeId === changeId) {
        return { text: revertLine(reverted), tone: 'polite' };
      }
      const kept =
        state.pending?.changeId === changeId
          ? undefined
          : state.active.find((change) => change.changeId === changeId);
      return kept === undefined
        ? { text: pendingLine(result.pending), tone: 'polite' }
        : { text: keptLine(kept), tone: 'polite' };
    }
    case 'restoredNative': {
      if (result.restored.length === 0) {
        return { text: NOTHING_TO_RESTORE, tone: 'polite' };
      }
      if (state.revision <= result.startRevision) {
        return null;
      }
      const lines = result.restored.map((id) => {
        const monitor = state.monitors.find((item) => sameMonitor(item.id, id));
        const label = monitor?.current?.label ?? monitor?.native?.label ?? '';
        return restoredLine(monitor?.name ?? null, label);
      });
      return { text: lines.join(' '), tone: 'polite' };
    }
    case 'restoredEverything':
      return {
        text: result.restored.length === 0 ? NO_OPEN_CHANGES : RESTORED_EVERYTHING,
        tone: 'polite',
      };
    case 'failed':
      return { text: errorMessage(result.error, hints), tone: 'assertive' };
  }
}

type OutcomeViewProps = {
  result: OutcomeResult;
  line: Line | null;
  hints: MessageHints;
  reverted: ModeReverted | null;
};

/**
 * The status line under an action, then one warning alert per warning, or the error. A pending
 * change that went back takes its warnings with it: they were about a mode no longer on screen.
 */
function OutcomeView({ result, line, hints, reverted }: OutcomeViewProps) {
  if (result.kind === 'failed') {
    return <ErrorAlert error={result.error} hints={hints} />;
  }
  const wentBack = result.kind === 'pending' && reverted?.changeId === result.pending.changeId;
  const warnings =
    !wentBack && (result.kind === 'applied' || result.kind === 'pending') ? result.warnings : [];
  return (
    <div className="flex flex-col gap-2">
      {line === null ? null : <p className="break-words tabular-nums">{line.text}</p>}
      {warnings.map((warning) => (
        <ErrorAlert key={warning.code} error={warning} hints={hints} isWarning />
      ))}
    </div>
  );
}

type SectionProps = {
  title: string;
  children: ReactNode;
};

function Section({ title, children }: SectionProps) {
  const headingId = useId();
  return (
    <section aria-labelledby={headingId} className="flex flex-col gap-3">
      <h2 id={headingId} className="text-base font-semibold">
        {title}
      </h2>
      {children}
    </section>
  );
}

type SaveResult = { kind: 'saved' } | { kind: 'failed'; error: IpcError } | null;

type SaveLineProps = { result: SaveResult };

function SaveLine({ result }: SaveLineProps) {
  if (result === null) {
    return null;
  }
  return result.kind === 'saved' ? <p>{SAVED}</p> : <ErrorAlert error={result.error} />;
}

export type HomeProps = {
  state: AppState;
};

/**
 * The smallest Home this build needs: the shown monitor, its presets, a custom size, Restore
 * native and Restore everything, the show-hidden toggle, and the outcome of each action. Every
 * value on screen comes from the core; the window never patches its snapshot.
 */
export function Home({ state }: HomeProps) {
  const reverted = useLastReverted();
  const { polite, assertive, announce } = useAnnouncer();
  const [running, setRunning] = useState<Running | null>(null);
  const [outcome, setOutcome] = useState<Outcome | null>(null);
  const [saveResult, setSaveResult] = useState<SaveResult>(null);
  const [isSaving, setIsSaving] = useState(false);
  const [width, setWidth] = useState('');
  const [height, setHeight] = useState('');
  const [sizeErrors, setSizeErrors] = useState({ width: false, height: false });
  const ids = useId();
  const busyReasonId = `${ids}-busy`;
  const monitorLineId = `${ids}-monitor-line`;
  const sizeErrorId = `${ids}-size-error`;
  const noIdentityId = `${ids}-no-identity`;
  const restoreNativeReasonId = `${ids}-restore-native-reason`;
  const restoreEverythingReasonId = `${ids}-restore-everything-reason`;
  const hiddenHelpId = `${ids}-hidden-help`;

  const monitor = shownMonitor(state);
  const visible = visibleMonitors(state);
  const hints: MessageHints = { monitorOnNvidia: monitor?.vendor === 'nvidia' };
  const busyAction = state.busy?.action ?? running?.action ?? null;
  const isBusy = busyAction !== null;
  const activeHere =
    monitor === undefined
      ? undefined
      : state.active.find((change) => sameMonitor(change.monitor, monitor.id));
  // A pending change is open too: Restore everything puts it back first (reason Superseded).
  const canRestoreEverything =
    state.active.length > 0 || state.pending !== null || state.recovery.status !== 'clean';
  const sizeRefresh = monitor?.current?.refresh ?? monitor?.native?.refresh;
  const badges: string[] = [];
  if (monitor?.primary === true) {
    badges.push('Primary');
  }
  if (monitor?.hidden === true) {
    badges.push('Virtual display');
  }
  if (monitor?.active === false) {
    badges.push('Off');
  }

  const line = outcome === null ? null : outcomeLine(outcome.result, state, reverted, hints);
  const lineText = line?.text ?? '';
  const lineTone = line?.tone ?? 'polite';
  useEffect(() => {
    if (lineText !== '') {
      announce(lineText, lineTone);
    }
  }, [lineText, lineTone, announce]);

  // A revert of a change this window did not start, such as one from the tray, is read out too.
  // The window's own pending change is read out through its outcome line instead.
  const ownPendingId =
    outcome?.result.kind === 'pending' ? outcome.result.pending.changeId : undefined;
  const announceOtherRevert = useEffectEvent((item: ModeReverted) => {
    if (item.changeId !== ownPendingId) {
      announce(revertLine(item), 'polite');
    }
  });
  useEffect(() => {
    if (reverted !== null) {
      announceOtherRevert(reverted);
    }
  }, [reverted]);

  async function run(
    key: string,
    action: BusyAction,
    place: Place,
    call: () => Promise<OutcomeResult>,
  ): Promise<void> {
    setRunning({ key, action });
    setOutcome(null);
    const result = await call();
    setRunning(null);
    setOutcome({ place, result });
  }

  function applyOn(target: Monitor, mode: Mode, key: string, place: Place): void {
    void run(key, 'applying', place, async () => {
      const result = await applyMode({
        kind: 'mode',
        monitor: target.id,
        mode,
        scaling: 'fullScreen',
      });
      if (!result.ok) {
        return { kind: 'failed', error: result.error };
      }
      const value = result.value;
      return value.kind === 'applied'
        ? { kind: 'applied', change: value.change, warnings: value.warnings }
        : { kind: 'pending', pending: value.pending, warnings: value.warnings };
    });
  }

  function applyPreset(target: Monitor, preset: Preset): void {
    // The mode goes back to the core exactly as it came, so the core matches its own rate.
    applyOn(target, preset.mode, `preset-${preset.aspect}`, 'presets');
  }

  function applySize(): void {
    if (monitor === undefined || sizeRefresh === undefined || isBusy || !hasIdentity(monitor.id)) {
      return;
    }
    const parsedWidth = Number(width);
    const parsedHeight = Number(height);
    const isWhole = (value: number): boolean =>
      Number.isInteger(value) && value >= 1 && value <= MAX_SIZE;
    const errors = { width: !isWhole(parsedWidth), height: !isWhole(parsedHeight) };
    setSizeErrors(errors);
    if (errors.width || errors.height) {
      return;
    }
    // A typed size carries the current rate and no label: the core finds the size in its own
    // list and builds the label, or answers MODE_NOT_OFFERED (REQ-DISP-11).
    applyOn(
      monitor,
      { width: parsedWidth, height: parsedHeight, refresh: sizeRefresh, label: '' },
      'custom',
      'custom',
    );
  }

  function restoreHere(target: Monitor): void {
    const startRevision = state.revision;
    void run('restore-native', 'restoring', 'restore', async () => {
      const result = await restoreNative({ monitor: target.id });
      return result.ok
        ? { kind: 'restoredNative', restored: result.value.restored, startRevision }
        : { kind: 'failed', error: result.error };
    });
  }

  function restoreAll(): void {
    void run('restore-everything', 'restoring', 'restore', async () => {
      const result = await restoreEverything();
      return result.ok
        ? { kind: 'restoredEverything', restored: result.value.restored }
        : { kind: 'failed', error: result.error };
    });
  }

  async function save(input: SettingsInput): Promise<void> {
    setIsSaving(true);
    setSaveResult(null);
    const result = await saveSettings(input);
    setIsSaving(false);
    if (result.ok) {
      setSaveResult({ kind: 'saved' });
      announce(SAVED, 'polite');
    } else {
      setSaveResult({ kind: 'failed', error: result.error });
      announce(errorMessage(result.error), 'assertive');
    }
  }

  function buttonText(key: string, idle: string): string {
    return running?.key === key ? BUSY_LABELS[running.action] : idle;
  }

  function outcomeAt(place: Place) {
    return outcome?.place === place ? (
      <OutcomeView result={outcome.result} line={line} hints={hints} reverted={reverted} />
    ) : null;
  }

  return (
    <>
      <Announcer polite={polite} assertive={assertive} />
      {busyAction === null ? null : (
        <div className="flex flex-col gap-1">
          <p id={busyReasonId}>{busyReason(busyAction)}</p>
          {state.busy === null ? null : <p>Step: {STEP_LABELS[state.busy.step]}.</p>}
          {state.busy?.waitingForUac === true ? <p>{UAC_WAITING}</p> : null}
        </div>
      )}
      {state.errors.length === 0 ? null : (
        <ul className="flex flex-col gap-2">
          {state.errors.map((raised) => (
            <li key={raised.id}>
              {ERROR_SOURCE_LABELS[raised.source]}: {errorMessage(raised.error)}
              {raised.count > 1 ? ` (${String(raised.count)} times)` : null}
            </li>
          ))}
        </ul>
      )}
      {state.build.backend === 'fake' ? (
        <p className="text-muted-foreground">{FAKE_BACKEND_NOTICE}</p>
      ) : null}

      {monitor === undefined ? (
        <p>{monitorLine(monitor)}</p>
      ) : (
        <>
          <Section title={monitorName(monitor)}>
            {visible.length > 1 ? (
              <div className="flex flex-col gap-1">
                <label htmlFor={`${ids}-monitor`} className="font-medium">
                  Monitor
                </label>
                <select
                  id={`${ids}-monitor`}
                  value={String(visible.indexOf(monitor))}
                  disabled={isSaving}
                  onChange={(event) => {
                    // Entries are keyed by their place in the list: a monitor whose device is
                    // disabled in Device Manager has no device path (REQ-DISP-26).
                    const picked = visible[Number(event.target.value)];
                    if (picked !== undefined) {
                      void save({ ...settingsInput(state.settings), selectedMonitor: picked.id });
                    }
                  }}
                  className="min-h-9 w-fit rounded-md border border-input bg-background px-3 text-foreground"
                >
                  {visible.map((item, index) => (
                    <option key={monitorKey(item.id)} value={String(index)}>
                      {monitorName(item)}
                      {item.primary ? ', Primary' : ''}
                      {item.hidden ? ', Virtual display' : ''}
                    </option>
                  ))}
                </select>
              </div>
            ) : null}
            {badges.length === 0 ? null : (
              <ul className="flex flex-wrap gap-2 text-xs" aria-label="Monitor badges">
                {badges.map((badge) => (
                  <li key={badge} className="rounded-md border border-border px-2 py-0.5">
                    {badge}
                  </li>
                ))}
              </ul>
            )}
            <p id={monitorLineId} className="tabular-nums">
              {monitorLine(monitor)}
            </p>
            {/* Two columns sized by their content: the scale has no auto plus fraction pair. */}
            <dl className="grid w-fit grid-cols-[auto_1fr] gap-x-4 gap-y-1 tabular-nums">
              <dt className="font-medium">Native</dt>
              <dd>{monitor.native?.label ?? 'Not reported'}</dd>
              <dt className="font-medium">Current</dt>
              <dd>{monitor.current?.label ?? 'Off'}</dd>
              <dt className="font-medium">Scaling</dt>
              <dd>{SCALING_LABELS[monitor.scaling]}</dd>
            </dl>
            {activeHere === undefined ? null : (
              <p className="break-words tabular-nums">{changedLine(activeHere)}</p>
            )}
          </Section>

          <Section title="Presets">
            {hasIdentity(monitor.id) ? null : (
              <p id={noIdentityId} className="text-muted-foreground">
                {NO_IDENTITY_REASON}
              </p>
            )}
            <ul className="flex flex-wrap gap-3">
              {monitor.presets.map((preset) => {
                const key = `preset-${preset.aspect}`;
                const notOfferedId = `${ids}-${key}-reason`;
                return (
                  <li key={preset.aspect} className="flex flex-col gap-1">
                    {/* The name joins the two visible lines with a space, and so does the space
                        between them, so the name holds the visible text word for word
                        (WCAG 2.5.3). A flex container draws no space between its items. */}
                    <ActionButton
                      isBlocked={!preset.offered || isBusy}
                      reasonId={preset.offered ? busyReasonId : notOfferedId}
                      label={`${buttonText(key, `Apply ${ASPECT_LABELS[preset.aspect]}`)} ${preset.mode.label}`}
                      onPress={() => {
                        applyPreset(monitor, preset);
                      }}
                      className="flex-col items-start gap-0"
                    >
                      <span>{buttonText(key, `Apply ${ASPECT_LABELS[preset.aspect]}`)}</span>{' '}
                      <span className="text-xs font-normal tabular-nums">{preset.mode.label}</span>
                    </ActionButton>
                    {preset.offered ? null : (
                      <p id={notOfferedId} className="text-xs text-muted-foreground">
                        Not offered on this monitor
                      </p>
                    )}
                  </li>
                );
              })}
            </ul>
            {outcomeAt('presets')}
          </Section>

          <Section title="Custom size">
            <form
              noValidate
              onSubmit={(event) => {
                event.preventDefault();
                applySize();
              }}
              className="flex flex-col gap-2"
            >
              <div className="flex flex-wrap items-end gap-3">
                <div className="flex flex-col gap-1">
                  <label htmlFor={`${ids}-width`} className="font-medium">
                    Width
                  </label>
                  <input
                    id={`${ids}-width`}
                    type="number"
                    inputMode="numeric"
                    min={1}
                    step={1}
                    value={width}
                    onChange={(event) => {
                      setWidth(event.target.value);
                    }}
                    aria-invalid={sizeErrors.width ? true : undefined}
                    aria-describedby={sizeErrors.width ? sizeErrorId : undefined}
                    className="min-h-9 w-28 rounded-md border border-input bg-background px-3 text-foreground tabular-nums"
                  />
                </div>
                <div className="flex flex-col gap-1">
                  <label htmlFor={`${ids}-height`} className="font-medium">
                    Height
                  </label>
                  <input
                    id={`${ids}-height`}
                    type="number"
                    inputMode="numeric"
                    min={1}
                    step={1}
                    value={height}
                    onChange={(event) => {
                      setHeight(event.target.value);
                    }}
                    aria-invalid={sizeErrors.height ? true : undefined}
                    aria-describedby={sizeErrors.height ? sizeErrorId : undefined}
                    className="min-h-9 w-28 rounded-md border border-input bg-background px-3 text-foreground tabular-nums"
                  />
                </div>
                <ActionButton
                  type="submit"
                  onPress={applySize}
                  isBlocked={isBusy || sizeRefresh === undefined || !hasIdentity(monitor.id)}
                  reasonId={
                    !hasIdentity(monitor.id)
                      ? noIdentityId
                      : sizeRefresh === undefined
                        ? monitorLineId
                        : busyReasonId
                  }
                >
                  {buttonText('custom', 'Apply size')}
                </ActionButton>
              </div>

              {sizeErrors.width || sizeErrors.height ? (
                <p id={sizeErrorId} className="text-destructive">
                  Enter a whole number above 0.
                </p>
              ) : null}
            </form>
            {outcomeAt('custom')}
          </Section>

          <Section title="Restore">
            <div className="flex flex-wrap items-start gap-3">
              <div className="flex flex-col gap-1">
                <ActionButton
                  variant="secondary"
                  isBlocked={activeHere === undefined || isBusy}
                  reasonId={activeHere === undefined ? restoreNativeReasonId : busyReasonId}
                  onPress={() => {
                    restoreHere(monitor);
                  }}
                >
                  {buttonText('restore-native', 'Restore native')}
                </ActionButton>
                {activeHere === undefined ? (
                  <p id={restoreNativeReasonId} className="text-xs text-muted-foreground">
                    ResSwap has not changed this monitor.
                  </p>
                ) : null}
              </div>
              <div className="flex flex-col gap-1">
                <ActionButton
                  isBlocked={!canRestoreEverything || isBusy}
                  reasonId={canRestoreEverything ? busyReasonId : restoreEverythingReasonId}
                  onPress={restoreAll}
                >
                  {buttonText('restore-everything', 'Restore everything')}
                </ActionButton>
                {canRestoreEverything ? null : (
                  <p id={restoreEverythingReasonId} className="text-xs text-muted-foreground">
                    ResSwap has no open changes.
                  </p>
                )}
              </div>
            </div>
            {outcomeAt('restore')}
          </Section>
        </>
      )}

      <Section title="Monitors">
        <label className="flex min-h-6 w-fit items-center gap-2">
          <input
            type="checkbox"
            checked={state.settings.showHiddenMonitors}
            disabled={isSaving}
            aria-describedby={hiddenHelpId}
            onChange={() => {
              void save({
                ...settingsInput(state.settings),
                showHiddenMonitors: !state.settings.showHiddenMonitors,
              });
            }}
            className="size-4 accent-primary"
          />
          Show hidden monitors
        </label>
        <p id={hiddenHelpId} className="text-muted-foreground">
          Virtual displays, such as remote-desktop adapters, stay hidden unless this is on.
        </p>
        <SaveLine result={saveResult} />
      </Section>
    </>
  );
}
