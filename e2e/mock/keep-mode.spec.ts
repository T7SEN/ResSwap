import type { Page } from '@playwright/test';
import { expectNoAxeViolations } from '../support/axe';
import { readCoreState, visibleLine } from '../support/core';
import { expect, test } from '../support/fixtures';
import { expectNoSidewaysScroll, expectTargetsAtLeast24 } from '../support/layout';
import { resultPath } from '../support/results';
import { KEEP_MODE_SIZE, sizeLabel, THEMES } from '../support/sizes';

const KEEP_URL = '/?scenario=pending-confirm&window=keep-mode';
const CHANGE_LINE = 'Primary monitor: changed from 1920 x 1080 at 240 Hz to 1280 x 960 at 240 Hz.';

/** One state of the keep-mode window: how to reach it and what it must show. */
type KeepState = {
  shot: string;
  state: string;
  url: string;
  /** True when the state needs Playwright's clock to move the mocked countdown. */
  usesClock: boolean;
  reach: (page: Page) => Promise<void>;
};

async function openKeepMode(page: Page, url: string, usesClock: boolean): Promise<void> {
  if (usesClock) {
    // Installed before the page loads, so the mocked core's countdown runs on this clock.
    await page.clock.install();
  }
  await page.goto(url);
  await expect(visibleLine(page, CHANGE_LINE)).toBeVisible();
  await expect(page.getByRole('button', { name: 'Keep' })).toBeFocused();
}

const STATES: readonly KeepState[] = [
  {
    shot: 'pending-confirm',
    state: 'keep-mode-load',
    url: KEEP_URL,
    usesClock: false,
    reach: async (page) => {
      await expect(page.getByRole('alertdialog', { name: 'Keep this resolution?' })).toBeVisible();
      await expect(page.getByText('Press Enter to keep, or Escape to revert.')).toBeVisible();
    },
  },
  {
    shot: 'pending-confirm',
    state: 'keep-mode-5-seconds',
    url: KEEP_URL,
    usesClock: true,
    reach: async (page) => {
      await expect(page.getByRole('alert')).toHaveText('');
      await page.clock.runFor(5000);
      await expect(page.getByRole('alert')).toHaveText(
        '5 seconds left. ResSwap will put back 1920 x 1080 at 240 Hz.',
      );
    },
  },
  {
    shot: 'pending-confirm',
    state: 'keep-mode-kept',
    url: KEEP_URL,
    usesClock: false,
    reach: async (page) => {
      await page.keyboard.press('Enter');
      await expect(visibleLine(page, 'Keeping 1280 x 960 at 240 Hz.')).toBeVisible();
    },
  },
  {
    shot: 'pending-confirm',
    state: 'keep-mode-reverted',
    url: KEEP_URL,
    usesClock: false,
    reach: async (page) => {
      await page.keyboard.press('Escape');
      await expect(visibleLine(page, 'Putting back 1920 x 1080 at 240 Hz.')).toBeVisible();
    },
  },
  {
    shot: 'MODE_REJECTED',
    state: 'keep-mode-revert-error',
    url: `${KEEP_URL}&fail=revert_mode:MODE_REJECTED`,
    usesClock: false,
    reach: async (page) => {
      await page.getByRole('button', { name: 'Revert' }).click();
      await expect(page.getByText('Open ResSwap and use Restore everything.')).toBeVisible();
    },
  },
];

for (const theme of THEMES) {
  const where = `${sizeLabel(KEEP_MODE_SIZE)} ${theme}`;

  test.describe(() => {
    test.use({ viewport: { ...KEEP_MODE_SIZE }, colorScheme: theme });

    for (const item of STATES) {
      test(`REQ-A11Y-06 ${item.shot} ${item.state} renders and passes axe at ${where}`, async ({
        page,
      }) => {
        // Also REQ-UI-06, REQ-A11Y-07, REQ-DOC-03
        await openKeepMode(page, item.url, item.usesClock);
        await item.reach(page);
        const label = `${item.shot} ${item.state} ${where}`;
        await expectNoAxeViolations(page, label);
        await expectNoSidewaysScroll(page, label);
        await expectTargetsAtLeast24(page, label);
        await page.screenshot({
          path: resultPath(
            `${item.shot}__${item.state}__${sizeLabel(KEEP_MODE_SIZE)}__${theme}.png`,
          ),
          animations: 'disabled',
          caret: 'hide',
        });
      });
    }
  });
}

test.describe(() => {
  test.use({ viewport: { ...KEEP_MODE_SIZE } });

  test('REQ-A11Y-08 Keep has focus at open, Tab reaches Revert, and the buttons fit the window', async ({
    page,
  }) => {
    await openKeepMode(page, KEEP_URL, false);
    await page.keyboard.press('Tab');
    await expect(page.getByRole('button', { name: 'Revert' })).toBeFocused();
    await expect(page.getByRole('button', { name: 'Keep' })).toBeInViewport();
    await expect(page.getByRole('button', { name: 'Revert' })).toBeInViewport();
  });

  test('REQ-SAFE-03 Enter keeps: the core stores the mode and nothing is pending', async ({
    page,
  }) => {
    await openKeepMode(page, KEEP_URL, false);
    await page.keyboard.press('Enter');
    await expect(visibleLine(page, 'Keeping 1280 x 960 at 240 Hz.')).toBeVisible();
    const state = await readCoreState(page);
    expect(state.pending).toBeNull();
    expect(state.settings.confirmedModes.map((item) => item.mode.label)).toEqual([
      '1280 x 960 at 240 Hz',
    ]);
  });

  test('REQ-SAFE-03 Escape reverts: the core puts back the mode from before', async ({ page }) => {
    await openKeepMode(page, KEEP_URL, false);
    await page.keyboard.press('Escape');
    await expect(visibleLine(page, 'Putting back 1920 x 1080 at 240 Hz.')).toBeVisible();
    const state = await readCoreState(page);
    expect(state.pending).toBeNull();
    expect(state.monitors[0]?.current?.label).toBe('1920 x 1080 at 240 Hz');
    expect(state.settings.confirmedModes).toEqual([]);
  });

  test('REQ-SAFE-03 Revert by pointer puts back the mode from before', async ({ page }) => {
    await openKeepMode(page, KEEP_URL, false);
    await page.getByRole('button', { name: 'Revert' }).click();
    await expect(visibleLine(page, 'Putting back 1920 x 1080 at 240 Hz.')).toBeVisible();
    expect((await readCoreState(page)).pending).toBeNull();
  });

  test('REQ-SAFE-05 the countdown follows the core and the mode goes back at 0', async ({
    page,
  }) => {
    await openKeepMode(page, KEEP_URL, true);
    await page.clock.runFor(3000);
    await expect(page.getByText('7', { exact: true })).toBeVisible();
    await page.clock.runFor(7000);
    await expect(
      visibleLine(
        page,
        'The countdown ended, so ResSwap put back 1920 x 1080 at 240 Hz on Primary monitor.',
      ),
    ).toBeVisible();
    await expect(page.getByRole('button', { name: 'Keep' })).toHaveAttribute(
      'aria-disabled',
      'true',
    );
    expect((await readCoreState(page)).pending).toBeNull();
  });
});
