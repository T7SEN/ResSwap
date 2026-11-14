import { render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, test } from 'vitest';
import { App } from '@/App';
import { confirmMode, getState, revertMode } from '@/core/client';
import { installMockCore } from '@/core/mock';
import { NO_PATH_SECOND_ID } from '@/core/mock/fixtures';

// The live regions repeat each line for screen readers; the tests read the visible line.
const NOT_LIVE = { ignore: '[aria-live], script, style' };

function findLine(text: string | RegExp): Promise<HTMLElement> {
  return screen.findByText(text, NOT_LIVE);
}

function withMockedCore(query: string): void {
  installMockCore(new URLSearchParams(query));
}

async function renderHome(query: string): Promise<void> {
  withMockedCore(query);
  render(<App hasCore />);
  await screen.findByRole('heading', { level: 2, name: 'Presets' });
}

/** The pending change's ID, read the way another window would read it. */
async function pendingId(): Promise<string> {
  const result = await getState();
  if (!result.ok || result.value.pending === null) {
    throw new Error('expected a pending change');
  }
  return result.value.pending.changeId;
}

describe('Home on the mocked core', () => {
  test('REQ-DISP-09 each preset applies by name, and one not offered says so and stays blocked', async () => {
    await renderHome('scenario=home-native');
    const preset = screen.getByRole('button', { name: 'Apply 4:3 1440 x 1080 at 240 Hz' });
    expect(preset).toHaveAttribute('aria-disabled', 'true');
    expect(preset).toHaveAccessibleDescription('Not offered on this monitor');
    expect(screen.getAllByText('Not offered on this monitor')).toHaveLength(3);

    const user = userEvent.setup();
    await user.click(preset);
    expect(await getState()).toMatchObject({ ok: true, value: { revision: 1 } });
  });

  test('REQ-SAFE-04 a preset already kept applies at once and says how to undo it', async () => {
    await renderHome('scenario=presets-offered');
    const user = userEvent.setup();
    await user.click(screen.getByRole('button', { name: 'Apply 4:3 1440 x 1080 at 240 Hz' }));
    expect(
      await findLine(
        'Primary monitor is now on 1440 x 1080 at 240 Hz. Restore native puts back 1920 x 1080 at 240 Hz.',
      ),
    ).toBeInTheDocument();
    expect(
      screen.getByText('Primary monitor is on 1440 x 1080 at 240 Hz.', NOT_LIVE),
    ).toBeInTheDocument();
    expect(
      screen.getByText(
        /^Changed from this window at .+\. Restore native puts back 1920 x 1080 at 240 Hz\.$/,
      ),
    ).toBeInTheDocument();
    // The polite region gets the same line on the next frame, so a screen reader reads it once.
    await waitFor(() => {
      expect(screen.getByRole('status')).toHaveTextContent(
        'Primary monitor is now on 1440 x 1080 at 240 Hz.',
      );
    });
  });

  test('REQ-SAFE-03 an unconfirmed preset is pending, then the line follows Keep', async () => {
    await renderHome('scenario=presets-offered');
    const user = userEvent.setup();
    await user.click(screen.getByRole('button', { name: 'Apply 5:4 1350 x 1080 at 240 Hz' }));
    expect(
      await findLine(
        'Primary monitor is on 1350 x 1080 at 240 Hz for now. Keep it, or ResSwap puts back 1920 x 1080 at 240 Hz in 10 seconds.',
      ),
    ).toBeInTheDocument();

    // Keep, as the keep-mode window sends it.
    expect(await confirmMode({ changeId: await pendingId() })).toMatchObject({ ok: true });
    expect(
      await findLine(
        'Kept 1350 x 1080 at 240 Hz on Primary monitor. ResSwap will not ask again for this mode on this monitor.',
      ),
    ).toBeInTheDocument();
  });

  test('REQ-SAFE-03 the pending line follows a Revert from the keep-mode window', async () => {
    await renderHome('scenario=presets-offered');
    const user = userEvent.setup();
    await user.click(screen.getByRole('button', { name: 'Apply 5:4 1350 x 1080 at 240 Hz' }));
    await findLine(/for now\. Keep it/);
    expect(await revertMode({ changeId: await pendingId() })).toMatchObject({ ok: true });
    expect(
      await findLine('ResSwap put back 1920 x 1080 at 240 Hz on Primary monitor.'),
    ).toBeInTheDocument();
  });

  test('REQ-DISP-11 a custom size the monitor does not offer shows MODE_NOT_OFFERED', async () => {
    await renderHome('scenario=home-native');
    const user = userEvent.setup();
    await user.type(screen.getByLabelText('Width'), '1440');
    await user.type(screen.getByLabelText('Height'), '1080');
    await user.click(screen.getByRole('button', { name: 'Apply size' }));
    expect(
      await findLine(
        'Windows does not offer 1440 x 1080 on Primary monitor. Nothing changed. On an NVIDIA card you can add it under Custom resolutions.',
      ),
    ).toBeInTheDocument();
    await waitFor(() => {
      expect(screen.getByRole('alert')).toHaveTextContent('Windows does not offer 1440 x 1080');
    });
  });

  test('REQ-DISP-11 Enter in the custom size runs Apply size, and a size the monitor lists is pending', async () => {
    await renderHome('scenario=home-native');
    const user = userEvent.setup();
    await user.type(screen.getByLabelText('Width'), '1280');
    await user.type(screen.getByLabelText('Height'), '960{Enter}');
    expect(
      await findLine(
        'Primary monitor is on 1280 x 960 at 240 Hz for now. Keep it, or ResSwap puts back 1920 x 1080 at 240 Hz in 10 seconds.',
      ),
    ).toBeInTheDocument();
  });

  test('REQ-DISP-11 a size that is not a whole number above 0 is refused in the window', async () => {
    await renderHome('scenario=home-native');
    const user = userEvent.setup();
    await user.type(screen.getByLabelText('Width'), '0');
    await user.type(screen.getByLabelText('Height'), '960');
    await user.click(screen.getByRole('button', { name: 'Apply size' }));
    expect(screen.getByLabelText('Width')).toHaveAttribute('aria-invalid', 'true');
    expect(screen.getByLabelText('Width')).toHaveAccessibleDescription(
      'Enter a whole number above 0.',
    );
    expect(screen.getByLabelText('Height')).not.toHaveAttribute('aria-invalid');
    expect(await getState()).toMatchObject({ ok: true, value: { revision: 1 } });
  });

  test('REQ-DISP-21 Restore native puts the monitor back and is blocked when nothing changed', async () => {
    await renderHome('scenario=home-stretched');
    const user = userEvent.setup();
    await user.click(screen.getByRole('button', { name: 'Restore native' }));
    expect(await findLine('Primary monitor is back on 1920 x 1080 at 240 Hz.')).toBeInTheDocument();
    const restore = screen.getByRole('button', { name: 'Restore native' });
    expect(restore).toHaveAttribute('aria-disabled', 'true');
    expect(restore).toHaveAccessibleDescription('ResSwap has not changed this monitor.');
  });

  test('REQ-DISP-22 Restore everything puts back every change and is blocked when none is open', async () => {
    await renderHome('scenario=home-stretched');
    const user = userEvent.setup();
    await user.click(screen.getByRole('button', { name: 'Restore everything' }));
    expect(await findLine('ResSwap put back everything it changed.')).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Restore everything' })).toHaveAccessibleDescription(
      'ResSwap has no open changes.',
    );
  });

  test('REQ-DISP-08 the show-hidden toggle saves, then lists the virtual display marked as virtual', async () => {
    await renderHome('scenario=home-native');
    expect(screen.queryByLabelText('Monitor')).not.toBeInTheDocument();
    const user = userEvent.setup();
    const toggle = screen.getByRole('checkbox', { name: 'Show hidden monitors' });
    expect(toggle).not.toBeChecked();
    await user.click(toggle);
    expect(await findLine('Saved.')).toBeInTheDocument();
    expect(toggle).toBeChecked();

    const picker = screen.getByLabelText('Monitor');
    const virtual = within(picker).getByRole('option', {
      name: 'Virtual display, Virtual display',
    });
    expect(virtual).toBeInTheDocument();
    // Entries are picked by their place in the list, never by a device path (REQ-DISP-26).
    await user.selectOptions(picker, virtual);
    expect(
      await screen.findByRole('heading', { level: 2, name: 'Virtual display' }),
    ).toBeInTheDocument();
    expect(
      within(screen.getByRole('list', { name: 'Monitor badges' })).getByText('Virtual display'),
    ).toBeInTheDocument();
  });

  test('REQ-DISP-26 two monitors without a device path are picked and changed apart', async () => {
    await renderHome('scenario=monitors-without-path');
    const user = userEvent.setup();
    const picker = screen.getByLabelText('Monitor');
    await user.selectOptions(
      picker,
      within(picker).getByRole('option', { name: 'Second monitor' }),
    );
    expect(
      await screen.findByRole('heading', { level: 2, name: 'Second monitor' }),
    ).toBeInTheDocument();
    await user.type(screen.getByLabelText('Width'), '1280');
    await user.type(screen.getByLabelText('Height'), '1024{Enter}');
    await waitFor(async () => {
      expect(await getState()).toMatchObject({
        ok: true,
        value: {
          settings: { selectedMonitor: NO_PATH_SECOND_ID },
          pending: { monitor: NO_PATH_SECOND_ID },
        },
      });
    });
  });

  test('REQ-DISP-26 a monitor no request can name has no presets and Apply size is blocked', async () => {
    await renderHome('scenario=monitor-without-identity');
    const reason =
      'ResSwap cannot change this monitor, because Windows does not say which monitor it is.';
    expect(screen.getByText(reason, NOT_LIVE)).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: /^Apply 4:3/ })).not.toBeInTheDocument();
    const applySize = screen.getByRole('button', { name: 'Apply size' });
    expect(applySize).toHaveAttribute('aria-disabled', 'true');
    expect(applySize).toHaveAccessibleDescription(reason);
  });

  test('REQ-UI-08 a failed save shows its message beside the toggle', async () => {
    await renderHome('scenario=home-native&fail=save_settings:SETTINGS_WRITE_FAILED');
    const user = userEvent.setup();
    await user.click(screen.getByRole('checkbox', { name: 'Show hidden monitors' }));
    expect(
      await findLine(
        'ResSwap could not save your settings to disk, so the last saved settings stay in use. Check that the drive has free space.',
      ),
    ).toBeInTheDocument();
    expect(screen.getByRole('checkbox', { name: 'Show hidden monitors' })).not.toBeChecked();
  });

  test('REQ-UI-08 a failed apply shows its message, its steps when it has any, and its detail', async () => {
    await renderHome('scenario=presets-offered&fail=apply_mode:MODE_REJECTED');
    const user = userEvent.setup();
    await user.click(screen.getByRole('button', { name: 'Apply 4:3 1440 x 1080 at 240 Hz' }));
    expect(
      await findLine(
        'Windows did not accept 1440 x 1080 at 240 Hz on Primary monitor. The display is on 1920 x 1080 at 240 Hz.',
      ),
    ).toBeInTheDocument();
    expect(screen.getByText('Mocked core: fail=apply_mode:MODE_REJECTED')).toBeInTheDocument();
  });

  test('REQ-UI-08 a warning from a successful apply sits under its success line', async () => {
    await renderHome('scenario=presets-offered&warn=apply_mode:SCALING_NOT_APPLIED');
    const user = userEvent.setup();
    await user.click(screen.getByRole('button', { name: 'Apply 4:3 1440 x 1080 at 240 Hz' }));
    expect(
      await findLine(/^The mode changed, but the graphics driver did not switch/),
    ).toBeInTheDocument();
    expect(
      screen.getByText(/^Primary monitor is now on 1440 x 1080 at 240 Hz\./, NOT_LIVE),
    ).toBeInTheDocument();
  });

  test('REQ-NV-25 a warning leaves with the pending change it belonged to', async () => {
    await renderHome('scenario=presets-offered&warn=apply_mode:SCALING_NOT_APPLIED');
    const user = userEvent.setup();
    await user.click(screen.getByRole('button', { name: 'Apply 5:4 1350 x 1080 at 240 Hz' }));
    await findLine(/^The mode changed, but the graphics driver did not switch/);
    expect(await revertMode({ changeId: await pendingId() })).toMatchObject({ ok: true });
    await findLine('ResSwap put back 1920 x 1080 at 240 Hz on Primary monitor.');
    expect(
      screen.queryByText(/^The mode changed, but the graphics driver did not switch/, NOT_LIVE),
    ).not.toBeInTheDocument();
  });

  test('REQ-DISP-22 the start-up line leaves once Restore everything leaves nothing changed', async () => {
    await renderHome('scenario=recovery-incomplete');
    const startUp = /^Start-up: ResSwap could not put everything back/;
    expect(await findLine(startUp)).toBeInTheDocument();
    const user = userEvent.setup();
    await user.click(screen.getByRole('button', { name: 'Restore everything' }));
    await waitFor(() => {
      expect(screen.queryByText(startUp, NOT_LIVE)).not.toBeInTheDocument();
    });
  });
});
