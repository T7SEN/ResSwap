import { join } from 'node:path';
import { expect, test } from '@playwright/test';
import { type RunningApp, startApp, stopApp } from '../support/app';
import { resultPath } from '../support/results';

// The shape the debug build's window exposes; read only inside page.evaluate.
type TauriWindow = {
  __TAURI_INTERNALS__: {
    invoke: (command: string, args: Record<string, unknown>) => Promise<unknown>;
    metadata: { currentWindow: { label: string } };
  };
};

// One app run for the whole file: one debugging port and one single-instance mutex.
test.describe.configure({ mode: 'serial' });

let app: RunningApp | undefined;

function running(): RunningApp {
  if (app === undefined) {
    throw new Error('The debug build did not start.');
  }
  return app;
}

test.beforeAll(async () => {
  const info = test.info();
  const runDir = join(info.project.outputDir, 'app', `smoke-${String(Date.now())}`);
  app = await startApp(runDir);
});

test.afterAll(async () => {
  if (app !== undefined) {
    await stopApp(app);
  }
});

test('REQ-UI-01 the debug build opens a page labelled main with the title ResSwap', async () => {
  const { page } = running();
  const label = await page.evaluate(
    () => (window as unknown as TauriWindow).__TAURI_INTERNALS__.metadata.currentWindow.label,
  );
  expect(label).toBe('main');
  await expect(page).toHaveTitle('ResSwap');
});

test('REQ-UI-17 get_state is answered by the fake backend, not the mocked core', async () => {
  // Also REQ-SEC-05
  const { page } = running();
  const marker = await page.evaluate(() => document.documentElement.dataset.core ?? null);
  expect(marker).not.toBe('RESSWAP_MOCK_CORE');
  // The same call the facade makes: @tauri-apps/api's invoke hands every command to this function,
  // so the answer passes the capability check and the real core's command on the fake backend.
  const state = await page.evaluate(() =>
    (window as unknown as TauriWindow).__TAURI_INTERNALS__.invoke('get_state', {}),
  );
  expect(state).toMatchObject({ build: { backend: 'fake', debug: true } });
});

test("REQ-PRIV-03 no request leaves the window's own origin", async () => {
  const { page, network } = running();
  // A reload puts the page's own start-up requests in the record as well.
  await page.reload({ waitUntil: 'load' });
  expect(network.seen.length).toBeGreaterThan(0);
  expect(network.outside()).toEqual([]);
});

test("REQ-DOC-03 the real window's web content is saved as a screenshot", async () => {
  const { page } = running();
  // A test never changes the Windows theme, so the name records whichever one is on.
  const isDark = await page.evaluate(() => matchMedia('(prefers-color-scheme: dark)').matches);
  await page.screenshot({ path: resultPath(`app__main__${isDark ? 'dark' : 'light'}.png`) });
});
