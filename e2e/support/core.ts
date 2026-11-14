import type { Locator, Page } from '@playwright/test';

// The shape of the page's IPC entry point, read only inside page.evaluate. In e2e:mock it is the
// mocked core; in e2e:app it is the real core on the fake backend.
type TauriWindow = {
  __TAURI_INTERNALS__: {
    invoke: (command: string, args: Record<string, unknown>) => Promise<unknown>;
  };
};

/** The parts of AppState these tests read. */
export type CoreStateView = {
  revision: number;
  pending: { changeId: string; secondsLeft: number } | null;
  active: { changeId: string; now: { label: string } }[];
  monitors: { name: string | null; current: { label: string } | null }[];
  settings: { showHiddenMonitors: boolean; confirmedModes: { mode: { label: string } }[] };
};

/** The core's state, read with the same call the facade makes. */
export async function readCoreState(page: Page): Promise<CoreStateView> {
  const state = await page.evaluate(() =>
    (window as unknown as TauriWindow).__TAURI_INTERNALS__.invoke('get_state', {}),
  );
  return state as CoreStateView;
}

/**
 * A line as it shows on screen. The live regions repeat lines for screen readers, so they are
 * left out here.
 */
export function visibleLine(page: Page, text: string | RegExp): Locator {
  return page.getByText(text, { exact: true }).and(page.locator(':not([aria-live])'));
}
