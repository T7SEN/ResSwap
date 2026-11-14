import { emit } from '@tauri-apps/api/event';
import { mockIPC } from '@tauri-apps/api/mocks';
import { act, renderHook, waitFor } from '@testing-library/react';
import { afterEach, describe, expect, test, vi } from 'vitest';
import { baseState, mode, PRIMARY_ID } from './mock/fixtures';
import type { AppState } from './types/AppState';
import type { ModeReverted } from './types/ModeReverted';
import type { PendingChange } from './types/PendingChange';
import { useCore, useCountdown, useLastReverted } from './useCore';

function stateAt(revision: number, pending: PendingChange | null = null): AppState {
  return { ...baseState(), revision, pending };
}

function pendingChange(changeId: string, secondsLeft: number): PendingChange {
  return {
    changeId,
    monitor: PRIMARY_ID,
    monitorName: 'Primary monitor',
    from: mode(1920, 1080, 240),
    to: mode(1440, 1080, 240),
    scaling: 'stretched',
    countdownSecs: 10,
    secondsLeft,
    source: 'window',
  };
}

/** Lets every promise the store started settle, without any timer. */
async function settle(): Promise<void> {
  await act(async () => {
    for (let turn = 0; turn < 50; turn += 1) {
      await Promise.resolve();
    }
  });
}

/** A core that answers get_state with the given state and records every command. */
function coreAnswering(state: () => AppState): string[] {
  const commands: string[] = [];
  mockIPC(
    (cmd) => {
      commands.push(cmd);
      return cmd === 'get_state' ? state() : null;
    },
    { shouldMockEvents: true },
  );
  return commands;
}

function revisionOf(snapshot: ReturnType<typeof useCore>): number | null {
  return snapshot.status === 'ready' ? snapshot.state.revision : null;
}

afterEach(() => {
  vi.useRealTimers();
});

describe('useCore', () => {
  test('REQ-UI-16 the window reads get_state once, then follows state-changed', async () => {
    const commands = coreAnswering(() => stateAt(1));
    const { result, rerender } = renderHook(() => useCore());
    expect(result.current.status).toBe('loading');
    await waitFor(() => {
      expect(revisionOf(result.current)).toBe(1);
    });

    // getSnapshot hands back the same object while nothing changes.
    const held = result.current;
    rerender();
    expect(result.current).toBe(held);

    await act(() => emit('state-changed', stateAt(3)));
    expect(revisionOf(result.current)).toBe(3);

    // An older snapshot never replaces a newer one.
    await act(() => emit('state-changed', stateAt(2)));
    expect(revisionOf(result.current)).toBe(3);
    expect(commands).toEqual(['get_state']);
  });

  test('REQ-UI-16 a change between the subscription and get_state is not lost', async () => {
    coreAnswering(() => {
      // The core changes after the window subscribed but before get_state answers.
      void emit('state-changed', stateAt(5));
      return stateAt(4);
    });
    const { result } = renderHook(() => useCore());
    await waitFor(() => {
      expect(revisionOf(result.current)).toBe(5);
    });
  });

  test('REQ-UI-16 no timer calls the core, however long the window stays open', async () => {
    vi.useFakeTimers({ toFake: ['setTimeout', 'setInterval'] });
    const commands = coreAnswering(() => stateAt(1));
    const { result } = renderHook(() => useCore());
    await settle();
    expect(revisionOf(result.current)).toBe(1);

    vi.advanceTimersByTime(60 * 60 * 1000);
    await settle();
    expect(commands).toEqual(['get_state']);
    expect(vi.getTimerCount()).toBe(0);
  });

  test('REQ-UI-08 a failed get_state leaves the window with that error', async () => {
    mockIPC(
      () => {
        throw new Error('no core');
      },
      { shouldMockEvents: true },
    );
    const { result } = renderHook(() => useCore());
    await waitFor(() => {
      expect(result.current.status).toBe('failed');
    });
    expect(result.current).toMatchObject({
      status: 'failed',
      error: { code: 'INTERNAL', detail: 'Error: no core' },
    });
  });
});

describe('the ready mark', () => {
  function readyMarks(): PerformanceEntryList {
    return performance.getEntriesByName('ResSwap:ready', 'mark');
  }

  test('REQ-PERF-05 the store marks ResSwap:ready once, when it first reaches ready', async () => {
    coreAnswering(() => stateAt(1));
    const { result } = renderHook(() => useCore());
    expect(result.current.status).toBe('loading');
    expect(readyMarks()).toHaveLength(0);
    await waitFor(() => {
      expect(revisionOf(result.current)).toBe(1);
    });
    expect(readyMarks()).toHaveLength(1);

    // A later snapshot is not a first one.
    await act(() => emit('state-changed', stateAt(2)));
    expect(revisionOf(result.current)).toBe(2);
    expect(readyMarks()).toHaveLength(1);
  });

  test('REQ-PERF-05 a failed get_state sets no ready mark, and a later snapshot sets it', async () => {
    mockIPC(
      () => {
        throw new Error('no core yet');
      },
      { shouldMockEvents: true },
    );
    const { result } = renderHook(() => useCore());
    await waitFor(() => {
      expect(result.current.status).toBe('failed');
    });
    expect(readyMarks()).toHaveLength(0);

    await act(() => emit('state-changed', stateAt(1)));
    expect(revisionOf(result.current)).toBe(1);
    expect(readyMarks()).toHaveLength(1);
  });
});

describe('useCountdown', () => {
  test('REQ-SAFE-05 the countdown follows the pending change, its ticks and its revert', async () => {
    coreAnswering(() => stateAt(1, pendingChange('change-1', 10)));
    const { result } = renderHook(() => useCountdown());
    await waitFor(() => {
      expect(result.current).toEqual({ changeId: 'change-1', secondsLeft: 10 });
    });

    await act(() => emit('countdown-tick', { changeId: 'change-1', secondsLeft: 9 }));
    expect(result.current).toEqual({ changeId: 'change-1', secondsLeft: 9 });

    // A tick for another change is not this countdown's.
    await act(() => emit('countdown-tick', { changeId: 'change-2', secondsLeft: 3 }));
    expect(result.current).toEqual({ changeId: 'change-1', secondsLeft: 9 });

    await act(() =>
      emit('mode-reverted', {
        changeId: 'change-1',
        monitor: PRIMARY_ID,
        monitorName: 'Primary monitor',
        reason: 'timeout',
        restored: mode(1920, 1080, 240),
        steps: [],
      }),
    );
    expect(result.current).toBeNull();
  });

  test('REQ-UI-06 a snapshot with no pending change ends the countdown', async () => {
    coreAnswering(() => stateAt(1, pendingChange('change-1', 10)));
    const { result } = renderHook(() => useCountdown());
    await waitFor(() => {
      expect(result.current).not.toBeNull();
    });
    await act(() => emit('state-changed', stateAt(2)));
    expect(result.current).toBeNull();
  });
});

describe('useLastReverted', () => {
  test('REQ-SAFE-03 it holds the newest mode-reverted payload, whatever change it names', async () => {
    coreAnswering(() => stateAt(1));
    const { result, rerender } = renderHook(() => useLastReverted());
    await settle();
    expect(result.current).toBeNull();

    const reverted: ModeReverted = {
      changeId: 'change-7',
      monitor: PRIMARY_ID,
      monitorName: 'Primary monitor',
      reason: 'keepWindowClosed',
      restored: mode(1920, 1080, 240),
      steps: [{ step: 'restore', status: 'done' }],
    };
    await act(() => emit('mode-reverted', reverted));
    expect(result.current).toEqual(reverted);

    // getSnapshot hands back the same object while nothing changes.
    const held = result.current;
    rerender();
    expect(result.current).toBe(held);
  });
});
