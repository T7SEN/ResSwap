import { execFile, spawn } from 'node:child_process';
import { existsSync, mkdirSync, readdirSync, readFileSync } from 'node:fs';
import { createConnection } from 'node:net';
import { join, resolve } from 'node:path';
import { setTimeout as delay } from 'node:timers/promises';
import { promisify } from 'node:util';
import { type Browser, chromium, type Page } from '@playwright/test';
import { APP_HOSTS, type NetworkLog, watchNetwork } from './network';
import { CDP_PORT } from './ports';

const REPO = resolve(import.meta.dirname, '..', '..');

/**
 * The debug build only. The runner sets the debugging port in the environment of this exe and of
 * nothing else, so no release build ever opens it (REQ-SEC-06). Cargo names the file after the
 * package; Windows file names ignore case.
 */
export const DEBUG_EXE = join(REPO, 'src-tauri', 'target', 'debug', 'ResSwap.exe');

// listProcesses lowers every image name, so the names compared with them are lowered too.
const APP_IMAGE = 'ResSwap.exe'.toLowerCase();
const WEBVIEW_IMAGE = 'msedgewebview2.exe';
const CDP_URL = `http://127.0.0.1:${String(CDP_PORT)}`;
const CONNECT_LIMIT_MS = 30_000;
const EXIT_LIMIT_MS = 10_000;
const POWERSHELL = join(
  process.env.SystemRoot ?? 'C:\\Windows',
  'System32',
  'WindowsPowerShell',
  'v1.0',
  'powershell.exe',
);

const execFileAsync = promisify(execFile);

/** One debug ResSwap.exe started by the test runner, with its main page over CDP. */
export type RunningApp = {
  readonly pid: number;
  readonly browser: Browser;
  readonly page: Page;
  readonly dataDir: string;
  /** Wall-clock time just before the spawn, in milliseconds since the Unix epoch. */
  readonly spawnedAtMs: number;
  /** Requests the window made after the runner connected. */
  readonly network: NetworkLog;
};

type ProcessRow = { pid: number; parentPid: number; image: string };

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null;
}

/** Reads the process list once: process IDs, parent IDs and image names, nothing else. */
async function listProcesses(): Promise<ProcessRow[]> {
  const query =
    'ConvertTo-Json -Compress -InputObject @(Get-CimInstance Win32_Process | ' +
    'Select-Object ProcessId, ParentProcessId, Name)';
  const { stdout } = await execFileAsync(
    POWERSHELL,
    ['-NoProfile', '-NonInteractive', '-Command', query],
    { windowsHide: true, maxBuffer: 16 * 1024 * 1024 },
  );
  const parsed: unknown = JSON.parse(stdout);
  if (!Array.isArray(parsed)) {
    throw new Error('The process list did not come back as a JSON array.');
  }
  return parsed.filter(isRecord).map((row) => ({
    pid: Number(row.ProcessId),
    parentPid: Number(row.ParentProcessId),
    image: typeof row.Name === 'string' ? row.Name.toLowerCase() : '',
  }));
}

/** Every process whose parent chain leads to the given process ID. */
function descendantsOf(rows: readonly ProcessRow[], rootPid: number): ProcessRow[] {
  const found: ProcessRow[] = [];
  const queue = [rootPid];
  let parent = queue.shift();
  while (parent !== undefined) {
    for (const row of rows) {
      if (row.parentPid === parent && row.pid !== rootPid && !found.includes(row)) {
        found.push(row);
        queue.push(row.pid);
      }
    }
    parent = queue.shift();
  }
  return found;
}

/** True when something already listens on the port on the loopback address. */
function isPortTaken(port: number): Promise<boolean> {
  return new Promise((resolvePort) => {
    const socket = createConnection({ host: '127.0.0.1', port });
    const finish = (taken: boolean): void => {
      socket.destroy();
      resolvePort(taken);
    };
    socket.setTimeout(1_000, () => {
      finish(false);
    });
    socket.once('connect', () => {
      finish(true);
    });
    socket.once('error', () => {
      finish(false);
    });
  });
}

/** The environment of the runner without any variable the app or WebView2 would read. */
function cleanEnvironment(): Record<string, string> {
  const env: Record<string, string> = {};
  for (const [key, value] of Object.entries(process.env)) {
    const upper = key.toUpperCase();
    if (value !== undefined && !upper.startsWith('RESSWAP_') && !upper.startsWith('WEBVIEW2_')) {
      env[key] = value;
    }
  }
  return env;
}

async function connect(isRunning: () => boolean): Promise<Browser> {
  const deadline = Date.now() + CONNECT_LIMIT_MS;
  let lastError: unknown = undefined;
  while (Date.now() < deadline) {
    if (!isRunning()) {
      throw new Error('ResSwap.exe ended before its debugging port opened.');
    }
    try {
      return await chromium.connectOverCDP(CDP_URL, { noDefaults: true, timeout: 5_000 });
    } catch (error) {
      lastError = error;
      await delay(250);
    }
  }
  throw new Error(`No connection to ${CDP_URL} within ${String(CONNECT_LIMIT_MS)} ms.`, {
    cause: lastError,
  });
}

/**
 * Starts the debug build on the fake backend with the debugging port open, and connects to its
 * main page. Refuses to start while any ResSwap.exe runs, because the single-instance mutex would
 * hand the start to that process; it never ends a process it did not start.
 */
export async function startApp(
  runDir: string,
  extraEnv: Readonly<Record<string, string>> = {},
): Promise<RunningApp> {
  if (!existsSync(DEBUG_EXE)) {
    throw new Error(`${DEBUG_EXE} is missing. Build it with: pnpm tauri build --debug --no-bundle`);
  }
  const running = (await listProcesses()).filter((row) => row.image === APP_IMAGE);
  if (running.length > 0) {
    const pids = running.map((row) => String(row.pid)).join(', ');
    throw new Error(`ResSwap.exe already runs (PID ${pids}). Quit it, then run the tests again.`);
  }
  if (await isPortTaken(CDP_PORT)) {
    throw new Error(`Port ${String(CDP_PORT)} is in use, so the runner cannot tell whose it is.`);
  }

  const webviewDir = join(runDir, 'webview2');
  const dataDir = join(runDir, 'data');
  mkdirSync(webviewDir, { recursive: true });
  mkdirSync(dataDir, { recursive: true });

  const env = {
    ...cleanEnvironment(),
    WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: `--remote-debugging-port=${String(CDP_PORT)}`,
    WEBVIEW2_USER_DATA_FOLDER: webviewDir,
    RESSWAP_BACKEND: 'fake',
    RESSWAP_DATA_DIR: dataDir,
    ...extraEnv,
  };
  const spawnedAtMs = Date.now();
  const child = spawn(DEBUG_EXE, [], { env, stdio: 'ignore', shell: false });
  const status = { hasExited: false };
  child.once('error', () => {
    status.hasExited = true;
  });
  child.once('exit', () => {
    status.hasExited = true;
  });
  const pid = child.pid;
  if (pid === undefined) {
    throw new Error('ResSwap.exe did not start.');
  }

  let browser: Browser | undefined;
  try {
    browser = await connect(() => !status.hasExited);
    const context = browser.contexts()[0];
    if (context === undefined) {
      throw new Error('The WebView2 debugging connection has no browser context.');
    }
    // Requests are recorded, not intercepted, so the runner never stands between the window and
    // the app's own protocol handlers.
    const network = watchNetwork(context, APP_HOSTS);
    const page = context.pages()[0] ?? (await context.waitForEvent('page'));
    await page.waitForURL(/^https?:\/\/tauri\.localhost\//);
    await page.waitForLoadState('load');
    return { pid, browser, page, dataDir, spawnedAtMs, network };
  } catch (error) {
    // The start failed half way: end the one process this call started, then report the failure.
    if (!status.hasExited) {
      process.kill(pid);
    }
    if (browser?.isConnected() === true) {
      await browser.close();
    }
    throw error;
  }
}

/**
 * Ends the one process the runner started, then checks that its WebView2 processes ended with
 * it. A WebView2 process that stays is reported and left running for review.
 */
export async function stopApp(app: RunningApp): Promise<void> {
  const before = await listProcesses();
  const webviews = descendantsOf(before, app.pid).filter((row) => row.image === WEBVIEW_IMAGE);
  const stillOurs = before.some((row) => row.pid === app.pid && row.image === APP_IMAGE);
  if (stillOurs) {
    process.kill(app.pid);
  }

  const watched = [{ pid: app.pid, image: APP_IMAGE }, ...webviews];
  const deadline = Date.now() + EXIT_LIMIT_MS;
  let left = watched;
  while (left.length > 0 && Date.now() < deadline) {
    await delay(250);
    const now = await listProcesses();
    left = watched.filter((item) =>
      now.some((row) => row.pid === item.pid && row.image === item.image),
    );
  }
  if (app.browser.isConnected()) {
    await app.browser.close();
  }
  if (left.length > 0) {
    const list = left.map((item) => `${item.image} ${String(item.pid)}`).join(', ');
    throw new Error(`Still running ${String(EXIT_LIMIT_MS)} ms after the stop: ${list}.`);
  }
}

/**
 * The epoch_ms of the newest `PERF app.started` line written at or after the given time, read
 * from the log files under the data folder, or undefined when no such line exists.
 */
export function findAppStartedEpochMs(dataDir: string, notBeforeMs: number): number | undefined {
  const pattern = /\bPERF app\.started\b.*\bepoch_ms=(\d+)/;
  let found: number | undefined;
  const visit = (dir: string): void => {
    for (const entry of readdirSync(dir, { withFileTypes: true })) {
      const path = join(dir, entry.name);
      if (entry.isDirectory()) {
        visit(path);
      } else if (entry.name.endsWith('.log')) {
        for (const line of readFileSync(path, 'utf8').split('\n')) {
          const match = pattern.exec(line);
          const epoch = match?.[1] === undefined ? Number.NaN : Number(match[1]);
          if (epoch >= notBeforeMs && (found === undefined || epoch > found)) {
            found = epoch;
          }
        }
      }
    }
  };
  if (existsSync(dataDir)) {
    visit(dataDir);
  }
  return found;
}
