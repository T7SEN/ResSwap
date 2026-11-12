import { isTauri } from '@tauri-apps/api/core';
import { clearMocks, mockIPC, mockWindows } from '@tauri-apps/api/mocks';
import { MockCore } from './core';
import { readParams } from './scenarios';

// A string value rather than a name, so minifying cannot remove it from a bundle that holds the
// mock: e2e:mock asserts it on the page, and every other build fails when its output holds it.
const MOCK_MARKER = 'RESSWAP_MOCK_CORE';
const BANNER_ID = 'mock-core-problems';

let installed: MockCore | null = null;

/** Lists what in the URL could not be used, above the window, so a test that relies on it fails. */
function showProblems(problems: readonly string[]): void {
  if (problems.length === 0) {
    return;
  }
  const banner = document.createElement('div');
  banner.id = BANNER_ID;
  banner.setAttribute('role', 'alert');
  banner.className = 'border-b border-destructive bg-background p-4 text-sm text-foreground';
  const title = document.createElement('p');
  title.className = 'font-semibold';
  title.textContent = 'The mocked core could not use this URL.';
  banner.append(title);
  for (const problem of problems) {
    const line = document.createElement('p');
    line.textContent = problem;
    banner.append(line);
  }
  document.body.prepend(banner);
}

/**
 * Installs the in-memory core for a normal browser: the window label, the IPC and the events.
 * Throws inside ResSwap's own window, so a flag that leaked into a real build changes nothing.
 */
export function installMockCore(params: URLSearchParams): void {
  if (isTauri()) {
    throw new Error('The mocked core runs only in a normal browser, never inside ResSwap.');
  }
  uninstallMockCore();
  const { label, scenario, faults, problems } = readParams(params);
  // Before anything reads the window label.
  mockWindows(label);
  const core = new MockCore(scenario, faults);
  mockIPC((cmd, payload) => core.dispatch(cmd, payload), { shouldMockEvents: true });
  installed = core;
  document.documentElement.dataset.core = MOCK_MARKER;
  showProblems(problems);
}

/**
 * Removes the mocked core and every Tauri mock. Tests outside src/core/mock call this, because
 * only this folder may import the mocks module.
 */
export function uninstallMockCore(): void {
  installed?.dispose();
  installed = null;
  clearMocks();
  document.getElementById(BANNER_ID)?.remove();
  delete document.documentElement.dataset.core;
}
