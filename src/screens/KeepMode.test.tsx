import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, describe, expect, test, vi } from 'vitest';
import { App } from '@/App';
import { getState } from '@/core/client';
import { installMockCore } from '@/core/mock';
import { KEEP_MODE_TITLE } from '@/lib/copy';

const NOT_LIVE = { ignore: '[aria-live], script, style' };

async function renderKeepMode(query = ''): Promise<void> {
  installMockCore(new URLSearchParams(`scenario=pending-confirm&window=keep-mode&${query}`));
  render(<App hasCore />);
  await screen.findByText(
    'Primary monitor: changed from 1920 x 1080 at 240 Hz to 1280 x 960 at 240 Hz.',
  );
}

/** Whether the core still holds a pending change. */
async function isPending(): Promise<boolean> {
  const result = await getState();
  return result.ok && result.value.pending !== null;
}

afterEach(() => {
  vi.useRealTimers();
});

describe('the keep-this-mode window', () => {
  test('REQ-A11Y-08 Keep has focus when the window opens, and Tab reaches Revert', async () => {
    await renderKeepMode();
    const keep = screen.getByRole('button', { name: 'Keep' });
    await waitFor(() => {
      expect(keep).toHaveFocus();
    });
    expect(
      screen.getByRole('alertdialog', { name: 'Keep this resolution?' }),
    ).toHaveAccessibleDescription(
      'Primary monitor: changed from 1920 x 1080 at 240 Hz to 1280 x 960 at 240 Hz. ResSwap puts back 1920 x 1080 at 240 Hz in 10 seconds unless you keep it.',
    );
    expect(document.title).toBe(KEEP_MODE_TITLE);
    expect(screen.queryByRole('heading', { name: 'ResSwap' })).not.toBeInTheDocument();

    const user = userEvent.setup();
    await user.tab();
    expect(screen.getByRole('button', { name: 'Revert' })).toHaveFocus();
  });

  test('REQ-SAFE-03 Enter keeps the mode, and the window says so until the core closes it', async () => {
    await renderKeepMode();
    await waitFor(() => {
      expect(screen.getByRole('button', { name: 'Keep' })).toHaveFocus();
    });
    const user = userEvent.setup();
    await user.keyboard('{Enter}');
    expect(await screen.findByText('Keeping 1280 x 960 at 240 Hz.', NOT_LIVE)).toBeInTheDocument();
    expect(await isPending()).toBe(false);
    expect(screen.getByRole('button', { name: 'Keep' })).toHaveAttribute('aria-disabled', 'true');
  });

  test('REQ-SAFE-03 Escape reverts the mode', async () => {
    await renderKeepMode();
    await waitFor(() => {
      expect(screen.getByRole('button', { name: 'Keep' })).toHaveFocus();
    });
    const user = userEvent.setup();
    await user.keyboard('{Escape}');
    expect(
      await screen.findByText('Putting back 1920 x 1080 at 240 Hz.', NOT_LIVE),
    ).toBeInTheDocument();
    expect(await isPending()).toBe(false);
  });

  test('REQ-SAFE-03 Revert puts the mode back', async () => {
    await renderKeepMode();
    const user = userEvent.setup();
    await user.click(screen.getByRole('button', { name: 'Revert' }));
    expect(
      await screen.findByText('Putting back 1920 x 1080 at 240 Hz.', NOT_LIVE),
    ).toBeInTheDocument();
    expect(await isPending()).toBe(false);
  });

  test('REQ-A11Y-08 a repeated Enter or Space, held from before, presses nothing', async () => {
    await renderKeepMode();
    const keep = screen.getByRole('button', { name: 'Keep' });
    await waitFor(() => {
      expect(keep).toHaveFocus();
    });
    fireEvent.keyDown(keep, { key: 'Enter', repeat: true });
    fireEvent.keyDown(keep, { key: ' ', repeat: true });
    fireEvent.keyUp(keep, { key: ' ' });
    await act(async () => {
      await Promise.resolve();
    });
    expect(screen.queryByText('Keeping 1280 x 960 at 240 Hz.', NOT_LIVE)).not.toBeInTheDocument();
    expect(await isPending()).toBe(true);
  });

  test('REQ-SAFE-05 the seconds follow the core ticks, and the 5-second line is said once', async () => {
    // Time still moves while Testing Library waits, so its own polling works; the steps below
    // jump the core's countdown by whole seconds.
    vi.useFakeTimers({ shouldAdvanceTime: true });
    await renderKeepMode();
    const alert = screen.getByRole('alert');
    expect(alert).toHaveTextContent('');
    expect(screen.getByText('10')).toBeInTheDocument();

    await act(async () => {
      await vi.advanceTimersByTimeAsync(1000);
    });
    expect(screen.getByText('9')).toBeInTheDocument();
    expect(alert).toHaveTextContent('');

    await act(async () => {
      await vi.advanceTimersByTimeAsync(4000);
    });
    expect(screen.getByText('5')).toBeInTheDocument();
    expect(alert).toHaveTextContent('5 seconds left. ResSwap will put back 1920 x 1080 at 240 Hz.');

    await act(async () => {
      await vi.advanceTimersByTimeAsync(5000);
    });
    expect(
      screen.getByText(
        'The countdown ended, so ResSwap put back 1920 x 1080 at 240 Hz on Primary monitor.',
        NOT_LIVE,
      ),
    ).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Keep' })).toHaveAttribute('aria-disabled', 'true');
  });

  test('REQ-UI-08 a revert error shows its message and points to Restore everything', async () => {
    await renderKeepMode('fail=revert_mode:MODE_REJECTED');
    const user = userEvent.setup();
    await user.click(screen.getByRole('button', { name: 'Revert' }));
    expect(
      await screen.findByText(
        'Windows did not accept 1440 x 1080 at 240 Hz on Primary monitor. The display is on 1920 x 1080 at 240 Hz.',
      ),
    ).toBeInTheDocument();
    expect(screen.getByText('Open ResSwap and use Restore everything.')).toBeInTheDocument();
    // Still pending, so Keep and Revert work again.
    expect(screen.getByRole('button', { name: 'Revert' })).not.toHaveAttribute('aria-disabled');
  });

  test('REQ-UI-06 with nothing pending the window says there is nothing to confirm', async () => {
    installMockCore(new URLSearchParams('scenario=home-native&window=keep-mode'));
    render(<App hasCore />);
    expect(await screen.findByText('Nothing to confirm.')).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Keep' })).toHaveAttribute('aria-disabled', 'true');
  });
});
