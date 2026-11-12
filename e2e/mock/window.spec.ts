import { expectNoAxeViolations } from '../support/axe';
import { expect, test } from '../support/fixtures';
import { resultPath } from '../support/results';
import { SIZES, sizeLabel, THEMES } from '../support/sizes';

// The mocked core's default scenario, named in the URL so the screenshot name matches it.
const SCENARIO = 'home-native';
const START_URL = `/?scenario=${SCENARIO}`;

for (const size of SIZES) {
  for (const theme of THEMES) {
    const where = `${sizeLabel(size)} ${theme}`;

    test.describe(() => {
      test.use({ viewport: { width: size.width, height: size.height }, colorScheme: theme });

      test(`REQ-UI-17 the window renders on the mocked core at ${where}`, async ({ page }) => {
        // Also REQ-UI-12, REQ-UI-14
        await page.goto(START_URL);
        const main = page.getByRole('main');
        await expect(main.getByRole('heading', { level: 1, name: 'ResSwap' })).toBeVisible();
        await expect(page.locator('html')).toHaveAttribute('data-core', 'RESSWAP_MOCK_CORE');
      });

      test(`REQ-A11Y-06 the axe scan passes and the screenshot is saved at ${where}`, async ({
        page,
      }) => {
        // Also REQ-DOC-03
        await page.goto(START_URL);
        await expect(page.getByRole('main')).toBeVisible();
        await expect(page.getByRole('main')).toHaveAttribute('aria-busy', 'false');
        await expectNoAxeViolations(page, `${SCENARIO} load ${where}`);
        await page.screenshot({
          path: resultPath(`${SCENARIO}__load__${sizeLabel(size)}__${theme}.png`),
          animations: 'disabled',
          caret: 'hide',
        });
      });
    });
  }
}

test('REQ-PRIV-03 no request leaves the local origin', async ({ page, network }) => {
  await page.goto(START_URL, { waitUntil: 'load' });
  await expect(page.getByRole('main')).toBeVisible();
  expect(network.seen.length).toBeGreaterThan(0);
  expect(network.outside()).toEqual([]);
});
