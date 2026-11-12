import { useSyncExternalStore } from 'react';
import { getState } from './client';
import { toIpcError } from './errors';
import { onCoreEvent, type UnlistenFn } from './events';
import type { AppState } from './types/AppState';
import type { IpcError } from './types/IpcError';

/** What the window knows about the core: loading, then the newest snapshot, or why it failed. */
export type CoreSnapshot =
  | { status: 'loading' }
  | { status: 'ready'; state: AppState }
  | { status: 'failed'; error: IpcError };

/** The pending change's countdown, or null when no change is pending. */
export type Countdown = { changeId: string; secondsLeft: number } | null;

const LOADING: CoreSnapshot = { status: 'loading' };

// The first-paint measurement reads this mark beside first contentful paint (REQ-PERF-05).
const READY_MARK = 'ResSwap:ready';

// One store per window. getSnapshot must hand back the same object until something changes,
// so each value below is replaced, never edited.
let snapshot: CoreSnapshot = LOADING;
let countdown: Countdown = null;
const coreListeners = new Set<() => void>();
const countdownListeners = new Set<() => void>();
let isStarted = false;
// A reset starts a new generation; work still running for an older one changes nothing.
let generation = 0;
let unlistenAll: UnlistenFn[] = [];

function notify(listeners: ReadonlySet<() => void>): void {
  for (const listener of listeners) {
    listener();
  }
}

function setCountdown(next: Countdown): void {
  if (next?.changeId === countdown?.changeId && next?.secondsLeft === countdown?.secondsLeft) {
    return;
  }
  countdown = next;
  notify(countdownListeners);
}

/** Starts or ends the countdown with the pending change in each snapshot the store keeps. */
function seedCountdown(state: AppState): void {
  const pending = state.pending;
  if (pending === null) {
    setCountdown(null);
  } else if (countdown?.changeId !== pending.changeId) {
    setCountdown({ changeId: pending.changeId, secondsLeft: pending.secondsLeft });
  }
}

function publish(next: CoreSnapshot): void {
  snapshot = next;
  notify(coreListeners);
}

/** Keeps a snapshot only when it is newer than the one held (REQ-UI-16). */
function accept(state: AppState): void {
  if (snapshot.status === 'ready' && state.revision <= snapshot.state.revision) {
    return;
  }
  // A store never leaves ready once it gets there, so this holds for its first snapshot only.
  const isFirstReady = snapshot.status !== 'ready';
  publish({ status: 'ready', state });
  if (isFirstReady) {
    performance.mark(READY_MARK);
  }
  seedCountdown(state);
}

function fail(error: IpcError): void {
  // A failure after a snapshot arrived changes nothing: the snapshot is still the truth.
  if (snapshot.status === 'loading') {
    publish({ status: 'failed', error });
  }
}

/**
 * Subscribes first, then reads, so a change between the two is never lost. After this, every
 * update arrives as an event; no timer ever calls the core.
 */
async function start(runFor: number): Promise<void> {
  const isCurrent = (): boolean => runFor === generation;
  try {
    const subscriptions = await Promise.all([
      onCoreEvent('state-changed', (state) => {
        if (isCurrent()) {
          accept(state);
        }
      }),
      onCoreEvent('countdown-tick', (tick) => {
        if (isCurrent() && tick.changeId === countdown?.changeId) {
          setCountdown({ changeId: tick.changeId, secondsLeft: tick.secondsLeft });
        }
      }),
      onCoreEvent('mode-reverted', (reverted) => {
        if (isCurrent() && reverted.changeId === countdown?.changeId) {
          setCountdown(null);
        }
      }),
    ]);
    if (!isCurrent()) {
      subscriptions.forEach(stopQuietly);
      return;
    }
    unlistenAll.push(...subscriptions);
    const result = await getState();
    if (!isCurrent()) {
      return;
    }
    if (result.ok) {
      accept(result.value);
    } else {
      fail(result.error);
    }
  } catch (raw) {
    // Only a subscription can land here; the facade itself never throws.
    if (isCurrent()) {
      fail(toIpcError(raw));
    }
  }
}

function ensureStarted(): void {
  if (!isStarted) {
    isStarted = true;
    void start(generation);
  }
}

function subscribeCore(listener: () => void): () => void {
  coreListeners.add(listener);
  ensureStarted();
  return () => {
    coreListeners.delete(listener);
  };
}

function subscribeCountdown(listener: () => void): () => void {
  countdownListeners.add(listener);
  ensureStarted();
  return () => {
    countdownListeners.delete(listener);
  };
}

function readSnapshot(): CoreSnapshot {
  return snapshot;
}

function readCountdown(): Countdown {
  return countdown;
}

/** The core's state as this window last received it. The core owns it; never edit it. */
export function useCore(): CoreSnapshot {
  return useSyncExternalStore(subscribeCore, readSnapshot);
}

/**
 * The pending change's seconds left. A second store, so a tick re-renders only the countdown.
 * The core counts; the window only shows what it sends.
 */
export function useCountdown(): Countdown {
  return useSyncExternalStore(subscribeCountdown, readCountdown);
}

// @tauri-apps/api 2.12.4 types the unsubscribe function as returning nothing, but it returns a
// promise (event.js), which rejects once the IPC it ran on is gone. Either way the subscription
// is gone, which is the result wanted.
function stopQuietly(unlisten: () => unknown): void {
  try {
    const done = unlisten();
    if (done instanceof Promise) {
      done.catch(() => undefined);
    }
  } catch {
    // Gone already, as above.
  }
}

/**
 * Vitest only: forgets the store, its ready mark and its subscriptions, so each test starts from
 * loading. Call it while the mocked IPC still answers, so the unsubscribe calls reach it.
 */
export function resetCoreStoreForTests(): void {
  generation += 1;
  isStarted = false;
  snapshot = LOADING;
  countdown = null;
  performance.clearMarks(READY_MARK);
  coreListeners.clear();
  countdownListeners.clear();
  const subscriptions = unlistenAll;
  unlistenAll = [];
  subscriptions.forEach(stopQuietly);
}
