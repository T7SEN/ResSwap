import { mkdirSync } from 'node:fs';
import { join, resolve } from 'node:path';

const REPO = resolve(import.meta.dirname, '..', '..');

function pad(value: number, width = 2): string {
  return String(value).padStart(width, '0');
}

/** The local date, YYYY-MM-DD, which names the day's results folder (REQ-DOC-03). */
export function localDate(at: Date = new Date()): string {
  return `${String(at.getFullYear())}-${pad(at.getMonth() + 1)}-${pad(at.getDate())}`;
}

/** ISO 8601 local time with its UTC offset, the form result files use. */
export function localIsoTime(at: Date = new Date()): string {
  const offset = -at.getTimezoneOffset();
  const sign = offset >= 0 ? '+' : '-';
  const hours = pad(Math.floor(Math.abs(offset) / 60));
  const minutes = pad(Math.abs(offset) % 60);
  const time = `${pad(at.getHours())}:${pad(at.getMinutes())}:${pad(at.getSeconds())}`;
  return `${localDate(at)}T${time}.${pad(at.getMilliseconds(), 3)}${sign}${hours}:${minutes}`;
}

/** docs/results/<YYYY-MM-DD>/ for today, created on first use. */
export function resultsDir(): string {
  const dir = join(REPO, 'docs', 'results', localDate());
  mkdirSync(dir, { recursive: true });
  return dir;
}

/** The path of one result file in the current day's results folder. */
export function resultPath(fileName: string): string {
  return join(resultsDir(), fileName);
}
