import type { Page } from '@playwright/test';
import { expectNoAxeViolations } from '../support/axe';
import { readCoreState, visibleLine } from '../support/core';
import { expect, test } from '../support/fixtures';
import { expectNoSidewaysScroll, expectTargetsAtLeast24 } from '../support/layout';
import { resultPath } from '../support/results';
import { SIZES, sizeLabel, THEMES } from '../support/sizes';

/** One state of the main window: how to reach it and what it must show. */
type ScreenState = {
  /** The scenario or error trigger part of the screenshot name. */
  shot: string;
  /** The state part of the screenshot name. */
  state: string;
  url: string;
  reach: (page: Page) => Promise<void>;
};

const APPLIED_4_3 =
  'Primary monitor is now on 1440 x 1080 at 240 Hz. Restore native puts back 1920 x 1080 at 240 Hz.';
const PENDING_5_4 =
  'Primary monitor is on 1350 x 1080 at 240 Hz for now. Keep it, or ResSwap puts back 1920 x 1080 at 240 Hz in 10 seconds.';
const NOT_OFFERED_1440 =
  'Windows does not offer 1440 x 1080 on Primary monitor. Nothing changed. On an NVIDIA card you can add it under Custom resolutions.';

function preset(page: Page, name: string) {
  return page.getByRole('button', { name, exact: true });
}

async function applyCustomSize(page: Page, width: string, height: string): Promise<void> {
  await page.getByLabel('Width').fill(width);
  await page.getByLabel('Height').fill(height);
  await page.getByRole('button', { name: 'Apply size' }).click();
}

const STATES: readonly ScreenState[] = [
  {
    shot: 'presets-offered',
    state: 'load',
    url: '/?scenario=presets-offered',
    reach: async (page) => {
      await expect(preset(page, 'Apply 4:3 1440 x 1080 at 240 Hz')).not.toHaveAttribute(
        'aria-disabled',
      );
      await expect(preset(page, 'Apply 16:10 1728 x 1080 at 240 Hz')).toHaveAttribute(
        'aria-disabled',
        'true',
      );
      await expect(page.getByText('Not offered on this monitor')).toHaveCount(1);
    },
  },
  {
    shot: 'presets-offered',
    state: 'applied',
    url: '/?scenario=presets-offered',
    reach: async (page) => {
      await preset(page, 'Apply 4:3 1440 x 1080 at 240 Hz').click();
      await expect(visibleLine(page, APPLIED_4_3)).toBeVisible();
    },
  },
  {
    shot: 'presets-offered',
    state: 'pending',
    url: '/?scenario=presets-offered',
    reach: async (page) => {
      await preset(page, 'Apply 5:4 1350 x 1080 at 240 Hz').click();
      await expect(visibleLine(page, PENDING_5_4)).toBeVisible();
    },
  },
  {
    shot: 'SCALING_NOT_APPLIED',
    state: 'applied-warning',
    url: '/?scenario=presets-offered&warn=apply_mode:SCALING_NOT_APPLIED',
    reach: async (page) => {
      await preset(page, 'Apply 4:3 1440 x 1080 at 240 Hz').click();
      await expect(visibleLine(page, APPLIED_4_3)).toBeVisible();
      await expect(
        visibleLine(page, /^The mode changed, but the graphics driver did not switch/),
      ).toBeVisible();
    },
  },
  {
    shot: 'MODE_REJECTED',
    state: 'preset-error',
    url: '/?scenario=presets-offered&fail=apply_mode:MODE_REJECTED',
    reach: async (page) => {
      await preset(page, 'Apply 4:3 1440 x 1080 at 240 Hz').click();
      await expect(
        visibleLine(
          page,
          'Windows did not accept 1440 x 1080 at 240 Hz on Primary monitor. The display is on 1920 x 1080 at 240 Hz.',
        ),
      ).toBeVisible();
      await page.getByText('Details', { exact: true }).click();
      await expect(page.getByText('Mocked core: fail=apply_mode:MODE_REJECTED')).toBeVisible();
    },
  },
  {
    shot: 'MODE_NOT_OFFERED',
    state: 'custom-size',
    url: '/?scenario=home-native',
    reach: async (page) => {
      await applyCustomSize(page, '1440', '1080');
      await expect(visibleLine(page, NOT_OFFERED_1440)).toBeVisible();
    },
  },
  {
    shot: 'home-native',
    state: 'custom-size-invalid',
    url: '/?scenario=home-native',
    reach: async (page) => {
      await applyCustomSize(page, '0', '960');
      await expect(page.getByLabel('Width')).toHaveAttribute('aria-invalid', 'true');
      await expect(page.getByText('Enter a whole number above 0.')).toBeVisible();
    },
  },
  {
    shot: 'home-stretched',
    state: 'restored-native',
    url: '/?scenario=home-stretched',
    reach: async (page) => {
      await page.getByRole('button', { name: 'Restore native' }).click();
      await expect(
        visibleLine(page, 'Primary monitor is back on 1920 x 1080 at 240 Hz.'),
      ).toBeVisible();
    },
  },
  {
    shot: 'home-stretched',
    state: 'restored-everything',
    url: '/?scenario=home-stretched',
    reach: async (page) => {
      await page.getByRole('button', { name: 'Restore everything' }).click();
      await expect(visibleLine(page, 'ResSwap put back everything it changed.')).toBeVisible();
    },
  },
  {
    shot: 'home-native',
    state: 'show-hidden',
    url: '/?scenario=home-native',
    reach: async (page) => {
      await page.getByRole('checkbox', { name: 'Show hidden monitors' }).click();
      await expect(visibleLine(page, 'Saved.')).toBeVisible();
      await expect(page.getByLabel('Monitor', { exact: true })).toBeVisible();
    },
  },
];

for (const size of SIZES) {
  for (const theme of THEMES) {
    const where = `${sizeLabel(size)} ${theme}`;

    test.describe(() => {
      test.use({ viewport: { width: size.width, height: size.height }, colorScheme: theme });

      for (const item of STATES) {
        test(`REQ-A11Y-06 ${item.shot} ${item.state} renders and passes axe at ${where}`, async ({
          page,
        }) => {
          // Also REQ-UI-14, REQ-A11Y-07, REQ-DOC-03
          await page.goto(item.url);
          await expect(page.getByRole('heading', { level: 2, name: 'Presets' })).toBeVisible();
          await item.reach(page);
          const label = `${item.shot} ${item.state} ${where}`;
          await expectNoAxeViolations(page, label);
          await expectNoSidewaysScroll(page, label);
          await expectTargetsAtLeast24(page, label);
          await page.screenshot({
            path: resultPath(`${item.shot}__${item.state}__${sizeLabel(size)}__${theme}.png`),
            fullPage: true,
            animations: 'disabled',
            caret: 'hide',
          });
        });
      }
    });
  }
}

test('REQ-SAFE-04 a preset already kept applies with no pending change and no countdown', async ({
  page,
}) => {
  await page.goto('/?scenario=presets-offered');
  await preset(page, 'Apply 4:3 1440 x 1080 at 240 Hz').click();
  await expect(visibleLine(page, APPLIED_4_3)).toBeVisible();
  const state = await readCoreState(page);
  expect(state.pending).toBeNull();
  expect(state.active.map((change) => change.now.label)).toEqual(['1440 x 1080 at 240 Hz']);
  await expect(page.getByText(/for now\. Keep it/)).toHaveCount(0);
});

test('REQ-SAFE-03 a preset not yet kept is pending, with the countdown running in the core', async ({
  page,
}) => {
  await page.goto('/?scenario=presets-offered');
  await preset(page, 'Apply 5:4 1350 x 1080 at 240 Hz').click();
  await expect(visibleLine(page, PENDING_5_4)).toBeVisible();
  const state = await readCoreState(page);
  expect(state.pending).not.toBeNull();
  expect(state.monitors[0]?.current?.label).toBe('1350 x 1080 at 240 Hz');
});

test('REQ-DISP-21 Restore native puts the monitor back on its native mode', async ({ page }) => {
  await page.goto('/?scenario=home-stretched');
  await expect(visibleLine(page, 'Primary monitor is on 1280 x 960 at 240 Hz.')).toBeVisible();
  await page.getByRole('button', { name: 'Restore native' }).click();
  await expect(visibleLine(page, 'Primary monitor is on 1920 x 1080 at 240 Hz.')).toBeVisible();
  expect((await readCoreState(page)).active).toEqual([]);
  await expect(page.getByRole('button', { name: 'Restore native' })).toHaveAttribute(
    'aria-disabled',
    'true',
  );
});

test('REQ-DISP-08 the show-hidden toggle saves through the core and lists the virtual display', async ({
  page,
}) => {
  await page.goto('/?scenario=home-native');
  await expect(page.getByLabel('Monitor', { exact: true })).toHaveCount(0);
  await page.getByRole('checkbox', { name: 'Show hidden monitors' }).click();
  await expect(page.getByRole('checkbox', { name: 'Show hidden monitors' })).toBeChecked();
  expect((await readCoreState(page)).settings.showHiddenMonitors).toBe(true);
  const picker = page.getByLabel('Monitor', { exact: true });
  await picker.selectOption({ label: 'Virtual display, Virtual display' });
  await expect(page.getByRole('heading', { level: 2, name: 'Virtual display' })).toBeVisible();
  await expect(
    page.getByRole('list', { name: 'Monitor badges' }).getByText('Virtual display'),
  ).toBeVisible();
});

test('REQ-DISP-11 a custom size the monitor does not offer answers MODE_NOT_OFFERED', async ({
  page,
}) => {
  await page.goto('/?scenario=home-native');
  await applyCustomSize(page, '1440', '1080');
  await expect(visibleLine(page, NOT_OFFERED_1440)).toBeVisible();
  await expect(page.getByRole('alert')).toHaveText(NOT_OFFERED_1440);
  expect((await readCoreState(page)).revision).toBe(1);
});
