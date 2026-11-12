import { defineConfig } from '@playwright/test';
import { MOCK_ORIGIN, MOCK_PORT } from './e2e/support/ports';

// The projects that start the real debug build. A run that names no project leaves both out.
const APP_PROJECTS: ReadonlySet<string> = new Set(['app-checks', 'app']);

// Playwright starts every webServer before any project runs, so a run of the app projects alone
// leaves the mock preview server out: it needs no dist-mock and opens no extra port.
function runsAppProjectsOnly(argv: readonly string[]): boolean {
  const names: string[] = [];
  argv.forEach((arg, index) => {
    const next = argv[index + 1];
    if (arg === '--project' && next !== undefined) {
      names.push(next);
    } else if (arg.startsWith('--project=')) {
      names.push(arg.slice('--project='.length));
    }
  });
  return names.length > 0 && names.every((name) => APP_PROJECTS.has(name));
}

// The real debug build over the WebView2 debugging port: one port and one single-instance mutex,
// so one worker and one test at a time.
const appRun = {
  testDir: './e2e/app',
  default: false,
  workers: 1,
  fullyParallel: false,
  timeout: 120_000,
};

const mockServer = {
  command: `pnpm exec vite preview --mode mock --port ${String(MOCK_PORT)}`,
  url: MOCK_ORIGIN,
  // Never attach to a server this run did not start.
  reuseExistingServer: false,
  timeout: 60_000,
};

export default defineConfig({
  testDir: './e2e',
  outputDir: './test-results',
  forbidOnly: true,
  // A test that needs a second try is a failure to fix, not to hide.
  retries: 0,
  reporter: [['list']],
  use: {
    trace: 'off',
    screenshot: 'off',
    video: 'off',
  },
  projects: [
    {
      name: 'mock',
      testDir: './e2e/mock',
      use: { browserName: 'chromium', baseURL: MOCK_ORIGIN },
    },
    {
      // Every spec of the real window except the first-paint measurement.
      name: 'app-checks',
      ...appRun,
      testIgnore: 'first-paint.spec.ts',
    },
    {
      // `--project app` runs app-checks first and the measurement last, so the measured starts
      // follow the other starts of the new build instead of being its first ones.
      name: 'app',
      ...appRun,
      testMatch: 'first-paint.spec.ts',
      dependencies: ['app-checks'],
    },
  ],
  ...(runsAppProjectsOnly(process.argv) ? {} : { webServer: mockServer }),
});
