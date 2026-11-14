import { useEffect, useEffectEvent, useRef, useState } from 'react';
import { ActionButton } from '@/components/ActionButton';
import { ErrorAlert } from '@/components/ErrorAlert';
import { confirmMode, revertMode } from '@/core/client';
import type { MessageHints } from '@/core/errors';
import { sameMonitor } from '@/core/monitorId';
import type { IpcError } from '@/core/types/IpcError';
import type { ModeReverted } from '@/core/types/ModeReverted';
import type { PendingChange } from '@/core/types/PendingChange';
import { useCore, useCountdown, useLastReverted } from '@/core/useCore';
import { KEEP_MODE_TITLE, monitorPhrase, revertLine } from '@/lib/copy';

// The seconds left at which the window says once, out loud, what happens at 0.
const ANNOUNCE_AT_SECONDS = 5;

type Action = 'keeping' | 'reverting';

function secondsWord(seconds: number): string {
  return seconds === 1 ? 'second' : 'seconds';
}

/** What the core reported for the change this window showed, once it is no longer pending. */
function endLine(
  shown: PendingChange,
  reverted: ModeReverted | null,
  keptTooLate: boolean,
): string | null {
  if (reverted?.changeId === shown.changeId) {
    return revertLine(reverted);
  }
  if (keptTooLate) {
    // Keep arrived after the countdown had already put the mode back.
    return revertLine({
      changeId: shown.changeId,
      monitor: shown.monitor,
      monitorName: shown.monitorName,
      reason: 'timeout',
      restored: shown.from,
      steps: [],
    });
  }
  return null;
}

/**
 * The keep-this-mode prompt (REQ-SAFE-03, REQ-UI-06). It calls only get_state, confirm_mode and
 * revert_mode. The countdown runs in the core; this window only shows the seconds the core sends
 * (REQ-SAFE-05, REQ-UI-16). Keep has focus at open, Enter keeps, Escape reverts (REQ-A11Y-08).
 */
export function KeepMode() {
  const core = useCore();
  const countdown = useCountdown();
  const reverted = useLastReverted();
  const keepRef = useRef<HTMLButtonElement>(null);
  const [shown, setShown] = useState<PendingChange | null>(null);
  const [action, setAction] = useState<Action | null>(null);
  const [keptTooLate, setKeptTooLate] = useState(false);
  const [error, setError] = useState<{ error: IpcError; isRevert: boolean } | null>(null);
  const [warnings, setWarnings] = useState<IpcError[]>([]);

  const state = core.status === 'ready' ? core.state : null;
  const pending = state?.pending ?? null;
  // The change this window showed stays on screen after the core ends it, until the core closes
  // the window, so the player sees what happened.
  if (pending !== null && pending.changeId !== shown?.changeId) {
    setShown(pending);
  }
  const canAct = pending !== null && action === null && !keptTooLate;

  const monitor = state?.monitors.find(
    (item) => shown !== null && sameMonitor(item.id, shown.monitor),
  );
  const hints: MessageHints = { monitorOnNvidia: monitor?.vendor === 'nvidia' };

  useEffect(() => {
    document.title = KEEP_MODE_TITLE;
  }, []);

  // Keep takes focus as soon as it can act, so Enter keeps and Tab reaches Revert.
  useEffect(() => {
    if (canAct) {
      keepRef.current?.focus();
    }
  }, [canAct]);

  async function keep(change: PendingChange): Promise<void> {
    setAction('keeping');
    setError(null);
    const result = await confirmMode({ changeId: change.changeId });
    if (!result.ok) {
      setAction(null);
      setError({ error: result.error, isRevert: false });
      return;
    }
    if (result.value.kind === 'notPending') {
      setAction(null);
      setKeptTooLate(true);
      return;
    }
    // Kept: the core closes this window, so the keeping line stays until then.
    setWarnings(result.value.warnings);
  }

  async function revert(change: PendingChange): Promise<void> {
    setAction('reverting');
    setError(null);
    const result = await revertMode({ changeId: change.changeId });
    if (!result.ok) {
      setAction(null);
      setError({ error: result.error, isRevert: true });
      return;
    }
    if (result.value.kind === 'notPending') {
      setAction(null);
    }
    // Reverted: the core closes this window, so the putting-back line stays until then.
  }

  const onEscape = useEffectEvent(() => {
    if (canAct) {
      void revert(pending);
    }
  });

  // Keyboard input that began before this window had focus is ignored: a repeated keydown, or
  // an Enter or Space before the window gained focus, never presses a button. A held Enter from
  // another window therefore cannot keep the mode.
  useEffect(() => {
    let hasFocus = document.hasFocus();
    let isSpaceBlocked = false;
    const onFocus = (): void => {
      hasFocus = true;
    };
    const onKeyDown = (event: KeyboardEvent): void => {
      if (event.key === 'Escape') {
        event.preventDefault();
        onEscape();
        return;
      }
      if ((event.key === 'Enter' || event.key === ' ') && (event.repeat || !hasFocus)) {
        event.preventDefault();
        if (event.key === ' ') {
          isSpaceBlocked = true;
        }
      }
    };
    const onKeyUp = (event: KeyboardEvent): void => {
      // A button acts on Space at keyup, so a blocked Space is stopped there too.
      if (event.key === ' ' && isSpaceBlocked) {
        event.preventDefault();
        isSpaceBlocked = false;
      }
    };
    window.addEventListener('focus', onFocus);
    window.addEventListener('keydown', onKeyDown, true);
    window.addEventListener('keyup', onKeyUp, true);
    return () => {
      window.removeEventListener('focus', onFocus);
      window.removeEventListener('keydown', onKeyDown, true);
      window.removeEventListener('keyup', onKeyUp, true);
    };
  }, []);

  const secondsLeft =
    shown === null
      ? null
      : countdown?.changeId === shown.changeId
        ? countdown.secondsLeft
        : pending?.changeId === shown.changeId
          ? pending.secondsLeft
          : 0;

  let status: string | null = null;
  if (shown !== null) {
    if (action === 'keeping') {
      status = `Keeping ${shown.to.label}.`;
    } else if (action === 'reverting') {
      status = `Putting back ${shown.from.label}.`;
    } else if (pending === null || keptTooLate) {
      status = endLine(shown, reverted, keptTooLate);
    }
  } else if (state !== null) {
    status = 'Nothing to confirm.';
  }

  const fiveSecondText =
    shown !== null &&
    pending !== null &&
    secondsLeft !== null &&
    shown.countdownSecs > ANNOUNCE_AT_SECONDS &&
    secondsLeft <= ANNOUNCE_AT_SECONDS
      ? `${String(ANNOUNCE_AT_SECONDS)} seconds left. ResSwap will put back ${shown.from.label}.`
      : '';

  const progress =
    shown === null || secondsLeft === null || shown.countdownSecs === 0
      ? 0
      : (secondsLeft / shown.countdownSecs) * 100;

  return (
    <main id="content" tabIndex={-1} className="flex flex-col p-4 text-sm">
      <div
        role="alertdialog"
        aria-labelledby="keep-title"
        aria-describedby={shown === null ? undefined : 'keep-change keep-description'}
        className="flex flex-col gap-2"
      >
        <div className="flex items-start justify-between gap-3">
          <h1 id="keep-title" className="text-xl font-semibold">
            Keep this resolution?
          </h1>
          {secondsLeft === null ? null : (
            <p className="flex items-baseline gap-1 tabular-nums">
              <span className="text-3xl font-semibold">{secondsLeft}</span>
              <span>{secondsWord(secondsLeft)}</span>
            </p>
          )}
        </div>
        {shown === null ? null : (
          <>
            {/* Steps down once a second with the core's ticks; it never tweens. */}
            <div aria-hidden="true" className="h-1.5 w-full rounded-full bg-muted">
              <div
                className="h-full rounded-full bg-primary"
                style={{ width: `${String(progress)}%` }}
              />
            </div>
            <p id="keep-change" className="break-words tabular-nums">
              {monitorPhrase(shown.monitorName, true)}: changed from {shown.from.label} to{' '}
              {shown.to.label}.
            </p>
            <p className="break-words tabular-nums">
              ResSwap puts back {shown.from.label} when the countdown ends.
            </p>
            <p id="keep-description" className="sr-only">
              ResSwap puts back {shown.from.label} in {shown.countdownSecs}{' '}
              {secondsWord(shown.countdownSecs)} unless you keep it.
            </p>
          </>
        )}
        {status === null ? null : <p className="break-words tabular-nums">{status}</p>}
        <div className="flex flex-wrap items-center gap-2">
          <ActionButton
            ref={keepRef}
            variant="primary"
            isBlocked={!canAct}
            onPress={() => {
              if (pending !== null) {
                void keep(pending);
              }
            }}
          >
            Keep
          </ActionButton>
          <ActionButton
            variant="secondary"
            isBlocked={!canAct}
            onPress={() => {
              if (pending !== null) {
                void revert(pending);
              }
            }}
          >
            Revert
          </ActionButton>
          <p className="text-xs text-muted-foreground">Press Enter to keep, or Escape to revert.</p>
        </div>
        {core.status === 'failed' ? <ErrorAlert error={core.error} /> : null}
        {error === null ? null : (
          <ErrorAlert error={error.error} hints={hints}>
            {error.isRevert ? <p>Open ResSwap and use Restore everything.</p> : null}
          </ErrorAlert>
        )}
        {warnings.map((warning) => (
          <ErrorAlert key={warning.code} error={warning} hints={hints} isWarning />
        ))}
        <div className="sr-only" role="alert" aria-live="assertive" aria-atomic="true">
          {fiveSecondText}
        </div>
      </div>
    </main>
  );
}
