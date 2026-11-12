import { execFile } from 'node:child_process';
import { readdirSync, writeFileSync } from 'node:fs';
import { release } from 'node:os';
import { join } from 'node:path';
import { promisify } from 'node:util';
import { expect, type Page, test } from '@playwright/test';
import { findAppStartedEpochMs, startApp, stopApp } from '../support/app';
import { localIsoTime, resultsDir } from '../support/results';

const PHASES = ['P0', 'P1', 'P2', 'P3', 'P4', 'P5', 'P6', 'P7', 'P8'];
// The first starts of a freshly built exe are slow for reasons outside ResSwap, so two starts
// are discarded before the measured ones.
const WARM_UP_STARTS = 2;
const MEASURED_STARTS = 5;

type PaintReading = {
  /** First contentful paint, in milliseconds since the Unix epoch. */
  paintEpochMs: number;
  shellEpochMs: number | null;
  readyEpochMs: number | null;
};

type Sample = {
  paintMs: number;
  shellMs: number | null;
  readyMs: number | null;
  origin: 'app.started' | 'spawn';
};

async function readPaint(page: Page): Promise<PaintReading> {
  return page.evaluate(async () => {
    const paintEntry = (): PerformanceEntry | undefined =>
      performance.getEntriesByName('first-contentful-paint')[0];
    const entry =
      paintEntry() ??
      (await new Promise<PerformanceEntry>((resolveEntry) => {
        new PerformanceObserver((list, observer) => {
          const found = list.getEntriesByName('first-contentful-paint')[0];
          if (found !== undefined) {
            observer.disconnect();
            resolveEntry(found);
          }
        }).observe({ type: 'paint', buffered: true });
      }));
    const mark = (name: string): number | null => {
      const found = performance.getEntriesByName(name, 'mark')[0];
      return found === undefined ? null : performance.timeOrigin + found.startTime;
    };
    return {
      paintEpochMs: performance.timeOrigin + entry.startTime,
      shellEpochMs: mark('ResSwap:shell'),
      readyEpochMs: mark('ResSwap:ready'),
    };
  });
}

async function headCommit(): Promise<string | null> {
  try {
    const { stdout } = await promisify(execFile)('git', ['rev-parse', 'HEAD'], {
      windowsHide: true,
    });
    return stdout.trim();
  } catch {
    // A repository with no commit yet has no HEAD to name.
    return null;
  }
}

function nextFileNumber(dir: string, phase: string): number {
  const pattern = new RegExp(`^measure-${phase}-firstpaint-(\\d+)\\.json$`);
  const taken = readdirSync(dir)
    .map((name) => pattern.exec(name)?.[1])
    .filter((value): value is string => value !== undefined)
    .map(Number);
  return taken.length === 0 ? 1 : Math.max(...taken) + 1;
}

function median(values: readonly number[]): number {
  const sorted = [...values].sort((a, b) => a - b);
  const middle = sorted[Math.floor(sorted.length / 2)];
  if (middle === undefined) {
    throw new Error('No value to take the median of.');
  }
  return sorted.length % 2 === 1
    ? middle
    : (middle + (sorted[sorted.length / 2 - 1] ?? middle)) / 2;
}

test('REQ-PERF-05 debug-build first paint is measured and written to docs/results', async () => {
  test.setTimeout(300_000);
  // The phase is named by the person who runs the gate, because the result file is matched on it.
  const phase = process.env.RESSWAP_E2E_PHASE ?? '';
  expect(PHASES, 'Set RESSWAP_E2E_PHASE to the phase, P0 to P8').toContain(phase);

  const info = test.info();
  const runDir = join(info.project.outputDir, 'app', `first-paint-${String(Date.now())}`);
  const samples: Sample[] = [];
  for (let start = 0; start < WARM_UP_STARTS + MEASURED_STARTS; start += 1) {
    const app = await startApp(runDir);
    try {
      const reading = await readPaint(app.page);
      const started = findAppStartedEpochMs(app.dataDir, app.spawnedAtMs);
      const originMs = started ?? app.spawnedAtMs;
      const since = (epochMs: number | null): number | null =>
        epochMs === null ? null : Math.round(epochMs - originMs);
      if (start >= WARM_UP_STARTS) {
        samples.push({
          paintMs: Math.round(reading.paintEpochMs - originMs),
          shellMs: since(reading.shellEpochMs),
          readyMs: since(reading.readyEpochMs),
          origin: started === undefined ? 'spawn' : 'app.started',
        });
      }
      expect(app.network.outside()).toEqual([]);
    } finally {
      await stopApp(app);
    }
  }

  // Every start measures from the same point, or the runs cannot be compared.
  const origins = new Set(samples.map((sample) => sample.origin));
  expect(origins.size, 'some starts logged PERF app.started and some did not').toBe(1);
  const runs = samples.map((sample) => sample.paintMs);
  const dir = resultsDir();
  const result = {
    schemaVersion: 1,
    phase,
    build: 'debug',
    source: 'first-paint.spec.ts',
    measuredAt: localIsoTime(),
    commit: await headCommit(),
    osBuild: Number(release().split('.')[2]),
    timings: {
      firstPaintMs: {
        runs,
        median: median(runs),
        max: Math.max(...runs),
        from: samples[0]?.origin ?? 'spawn',
        shellRuns: samples.map((sample) => sample.shellMs),
        readyRuns: samples.map((sample) => sample.readyMs),
      },
    },
  };
  const file = join(dir, `measure-${phase}-firstpaint-${String(nextFileNumber(dir, phase))}.json`);
  writeFileSync(file, `${JSON.stringify(result, null, 2)}\n`, 'utf8');
});
