import { join } from 'node:path';
import { expect, type Page, test } from '@playwright/test';
import { type RunningApp, startApp, stopApp } from '../support/app';
import { visibleLine } from '../support/core';
import { resultPath } from '../support/results';

// The P1 rows of the real-window checks, against the fake backend's test-pc fixture: the LG on
// 1920 x 1080 at 240 Hz, which lists 1280 x 960 at 240 Hz. None of the three preset sizes is
// listed there, so the unconfirmed change goes through the custom size, which takes the same
// test-then-apply path as a preset (REQ-DISP-11).

// The shape of the debug build's window; read only inside page.evaluate.
type TauriWindow = {
  __TAURI_INTERNALS__: { metadata: { currentWindow: { label: string } } };
};

const MONITOR = 'LG ULTRAGEAR';
const NATIVE = '1920 x 1080 at 240 Hz';
const STRETCHED = '1280 x 960 at 240 Hz';
const KEEP_PAGE_LIMIT_MS = 15_000;

// One app run per describe block, one at a time: one debugging port, one single-instance mutex.
test.describe.configure({ mode: 'serial' });

function windowLabel(page: Page): Promise<string> {
  return page.evaluate(
    () => (window as unknown as TauriWindow).__TAURI_INTERNALS__.metadata.currentWindow.label,
  );
}

async function themeName(page: Page): Promise<'dark' | 'light'> {
  const isDark = await page.evaluate(() => matchMedia('(prefers-color-scheme: dark)').matches);
  return isDark ? 'dark' : 'light';
}

/** Types the stretched size into the main window and applies it. */
async function applyStretched(main: Page): Promise<void> {
  await main.getByLabel('Width').fill('1280');
  await main.getByLabel('Height').fill('960');
  await main.getByRole('button', { name: 'Apply size' }).click();
}

/**
 * Applies the stretched size and returns the keep-mode window the core opens for it, as a second
 * page of the same WebView2 connection.
 */
async function applyAndWaitForKeepMode(app: RunningApp): Promise<Page> {
  const opened = app.page.context().waitForEvent('page', { timeout: KEEP_PAGE_LIMIT_MS });
  await applyStretched(app.page);
  const keepPage = await opened;
  // A new WebView2 target starts on about:blank, as the main page does in startApp, so wait for
  // the bundled page before reading its label.
  await keepPage.waitForURL(/^https?:\/\/tauri\.localhost\//);
  await keepPage.waitForLoadState('load');
  expect(await windowLabel(keepPage)).toBe('keep-mode');
  return keepPage;
}

/** The pages of the connection other than the main window. */
function otherPages(app: RunningApp): Page[] {
  return app.page
    .context()
    .pages()
    .filter((page) => page !== app.page);
}

test.describe('the keep-mode window on the fake backend', () => {
  let app: RunningApp | undefined;

  function running(): RunningApp {
    if (app === undefined) {
      throw new Error('The debug build did not start.');
    }
    return app;
  }

  test.beforeAll(async () => {
    const info = test.info();
    app = await startApp(join(info.project.outputDir, 'app', `display-${String(Date.now())}`));
  });

  test.afterAll(async () => {
    if (app !== undefined) {
      await stopApp(app);
    }
  });

  test('REQ-SAFE-03 an unconfirmed mode opens the keep-mode window with focus, and Revert closes it', async () => {
    // Also REQ-UI-06, REQ-A11Y-08
    const current = running();
    await expect(visibleLine(current.page, `${MONITOR} is on ${NATIVE}.`)).toBeVisible();
    const keepPage = await applyAndWaitForKeepMode(current);

    await expect(keepPage.getByRole('button', { name: 'Keep' })).toBeFocused();
    expect(await keepPage.evaluate(() => document.hasFocus())).toBe(true);
    await expect(
      visibleLine(keepPage, `${MONITOR}: changed from ${NATIVE} to ${STRETCHED}.`),
    ).toBeVisible();
    await keepPage.screenshot({
      path: resultPath(`app__keep-mode__${await themeName(keepPage)}.png`),
    });

    const closed = keepPage.waitForEvent('close');
    await keepPage.getByRole('button', { name: 'Revert' }).click();
    await closed;
    await expect(
      visibleLine(current.page, `ResSwap put back ${NATIVE} on ${MONITOR}.`),
    ).toBeVisible();
    await expect(visibleLine(current.page, `${MONITOR} is on ${NATIVE}.`)).toBeVisible();
  });

  test('REQ-SAFE-04 after Keep, the same mode applies again with no keep-mode window', async () => {
    // Also REQ-DISP-19
    const current = running();
    const keepPage = await applyAndWaitForKeepMode(current);
    const closed = keepPage.waitForEvent('close');
    await keepPage.getByRole('button', { name: 'Keep' }).click();
    await closed;
    await expect(
      visibleLine(
        current.page,
        `Kept ${STRETCHED} on ${MONITOR}. ResSwap will not ask again for this mode on this monitor.`,
      ),
    ).toBeVisible();

    await current.page.getByRole('button', { name: 'Restore native' }).click();
    await expect(visibleLine(current.page, `${MONITOR} is back on ${NATIVE}.`)).toBeVisible();

    await applyStretched(current.page);
    await expect(
      visibleLine(
        current.page,
        `${MONITOR} is now on ${STRETCHED}. Restore native puts back ${NATIVE}.`,
      ),
    ).toBeVisible();
    // The core opens the keep-mode window before it answers a pending change, so by the time
    // the applied line shows, no window was opened for this one.
    expect(otherPages(current)).toEqual([]);
  });

  test('REQ-DISP-21 Restore native returns the fake monitor to its native mode', async () => {
    const current = running();
    await expect(visibleLine(current.page, `${MONITOR} is on ${STRETCHED}.`)).toBeVisible();
    await current.page.getByRole('button', { name: 'Restore native' }).click();
    await expect(visibleLine(current.page, `${MONITOR} is back on ${NATIVE}.`)).toBeVisible();
    await expect(visibleLine(current.page, `${MONITOR} is on ${NATIVE}.`)).toBeVisible();
    await expect(current.page.getByRole('button', { name: 'Restore native' })).toHaveAttribute(
      'aria-disabled',
      'true',
    );
    await current.page.screenshot({
      path: resultPath(`app__main-restored__${await themeName(current.page)}.png`),
    });
  });
});

test.describe('a scripted fault from the real core', () => {
  let app: RunningApp | undefined;

  test.beforeAll(async () => {
    const info = test.info();
    app = await startApp(join(info.project.outputDir, 'app', `fault-${String(Date.now())}`), {
      RESSWAP_FAKE_FAIL: 'apply_mode:MODE_REJECTED',
    });
  });

  test.afterAll(async () => {
    if (app !== undefined) {
      await stopApp(app);
    }
  });

  test('REQ-UI-08 apply_mode answering MODE_REJECTED shows that message', async () => {
    if (app === undefined) {
      throw new Error('The debug build did not start.');
    }
    await applyStretched(app.page);
    // The placeholders depend on the context the scripted fault carries, so only the fixed start
    // of the message is matched.
    await expect(
      visibleLine(app.page, /^Windows did not accept .+\. The display is on .+\.$/),
    ).toBeVisible();
    expect(otherPages(app)).toEqual([]);
  });
});
