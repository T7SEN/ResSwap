// Compares measurements with the gate values in scripts/budgets.json and prints one line per
// budget that applies at the phase given as the first argument (pnpm budgets P0).
// It only reads files. Exit codes: 0 when every applicable row passes; 1 when a row is over its
// budget or a file cannot be used; 2 when a row has no measurement or no gate value yet, which
// counts as not run or waiting, not as a failed gate.
import { existsSync, readFileSync, readdirSync, statSync } from 'node:fs';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';

const PHASES = ['P0', 'P1', 'P2', 'P3', 'P4', 'P5', 'P6', 'P7', 'P8'];
const EVERY = ['gate', 'hw-round', 'record'];
const BUILDS = ['release', 'debug'];
const DAY_FOLDER = /^\d{4}-\d{2}-\d{2}$/;
const RESULT_FILE = /^measure-.+\.json$/;
const INSTALLER_METRIC = 'installer.bytes';
const BYTE_ORDER_MARK = String.fromCharCode(0xfeff);

// Paths are resolved from this file, so the result does not depend on the working folder.
const root = fileURLToPath(new URL('..', import.meta.url));
const budgetsFile = join(root, 'scripts', 'budgets.json');
const resultsDir = join(root, 'docs', 'results');
const nsisDir = join(root, 'src-tauri', 'target', 'release', 'bundle', 'nsis');

const phase = (process.argv[2] ?? '').toUpperCase();
if (!PHASES.includes(phase)) {
  console.error('Usage: pnpm budgets <phase>, where the phase is P0 to P8.');
  process.exit(2);
}

const at = (p) => PHASES.indexOf(p);
const problems = [];

function pick(object, path) {
  let value = object;
  for (const key of path.split('.')) {
    if (value === null || typeof value !== 'object') return undefined;
    value = value[key];
  }
  return value;
}

function readJson(file) {
  // A byte order mark would make JSON.parse fail, and some editors add one.
  const text = readFileSync(file, 'utf8');
  return JSON.parse(text.startsWith(BYTE_ORDER_MARK) ? text.slice(1) : text);
}

function rowProblem(row, index) {
  const name = typeof row?.id === 'string' && row.id !== '' ? row.id : `row ${String(index + 1)}`;
  if (row === null || typeof row !== 'object') return `${name}: not an object`;
  if (typeof row.id !== 'string' || row.id === '') return `${name}: id must be a text`;
  if (typeof row.req !== 'string' || !/^REQ-[A-Z0-9]+-\d{2}$/.test(row.req)) {
    return `${name}: req must be a requirement ID such as REQ-PERF-01`;
  }
  if (!PHASES.includes(row.from)) return `${name}: from must be P0 to P8`;
  if (row.until !== undefined && (!PHASES.includes(row.until) || at(row.until) < at(row.from))) {
    return `${name}: until must be a phase at or after from`;
  }
  if (typeof row.metric !== 'string' || row.metric === '') return `${name}: metric must be a text`;
  if (!EVERY.includes(row.every)) return `${name}: every must be gate, hw-round or record`;
  if (row.build !== undefined && !BUILDS.includes(row.build)) {
    return `${name}: build must be release or debug`;
  }
  if (row.max !== null && (typeof row.max !== 'number' || !Number.isFinite(row.max))) {
    return `${name}: max must be a number or null`;
  }
  if (row.every === 'record' && row.max !== null) {
    return `${name}: a record row has no gate value, so max must be null`;
  }
  if (row.strict !== undefined && typeof row.strict !== 'boolean') {
    return `${name}: strict must be true or false`;
  }
  if (row.where !== undefined && (row.where === null || typeof row.where !== 'object')) {
    return `${name}: where must be an object`;
  }
  return undefined;
}

function loadBudgets() {
  let data;
  try {
    data = readJson(budgetsFile);
  } catch (error) {
    return {
      error: `scripts/budgets.json: ${error instanceof Error ? error.message : String(error)}`,
    };
  }
  if (data?.schemaVersion !== 1 || !Array.isArray(data.budgets)) {
    return { error: 'scripts/budgets.json: needs schemaVersion 1 and a budgets list' };
  }
  const errors = data.budgets.map(rowProblem).filter((p) => p !== undefined);
  const ids = data.budgets.map((row) => row?.id);
  const repeated = ids.filter((id, index) => ids.indexOf(id) !== index);
  if (repeated.length > 0) errors.push(`ids used twice: ${[...new Set(repeated)].join(', ')}`);
  if (errors.length > 0) return { error: `scripts/budgets.json: ${errors.join('; ')}` };
  return { budgets: data.budgets };
}

function readResults() {
  if (!existsSync(resultsDir)) return [];
  const results = [];
  for (const day of readdirSync(resultsDir).sort()) {
    const dayDir = join(resultsDir, day);
    if (!DAY_FOLDER.test(day) || !statSync(dayDir).isDirectory()) continue;
    for (const name of readdirSync(dayDir).sort()) {
      if (!RESULT_FILE.test(name)) continue;
      const shown = `docs/results/${day}/${name}`;
      let data;
      try {
        data = readJson(join(dayDir, name));
      } catch (error) {
        problems.push(`${shown}: ${error instanceof Error ? error.message : String(error)}`);
        continue;
      }
      const time = Date.parse(data?.measuredAt);
      if (
        data?.schemaVersion !== 1 ||
        !PHASES.includes(data.phase) ||
        !BUILDS.includes(data.build) ||
        Number.isNaN(time)
      ) {
        problems.push(
          `${shown}: needs schemaVersion 1, a phase P0 to P8, build release or debug and an ISO 8601 measuredAt`,
        );
        continue;
      }
      results.push({ shown, data, time });
    }
  }
  return results.sort((a, b) => a.time - b.time);
}

// The installer is read from the bundle folder at every gate, not from a result file.
function installerValues() {
  if (!existsSync(nsisDir)) return { values: [], note: 'no installer built' };
  const files = readdirSync(nsisDir).filter((f) => f.endsWith('-setup.exe'));
  if (files.length === 0) return { values: [], note: 'no installer built' };
  if (files.length > 1) {
    problems.push(
      `src-tauri/target/release/bundle/nsis holds ${String(files.length)} installers (${files.join(', ')}); expected one`,
    );
    return { values: [], note: 'more than one installer', error: true };
  }
  return { values: [statSync(join(nsisDir, files[0])).size], note: files[0] };
}

function resultValues(row, results) {
  const build = row.build ?? 'release';
  const where = Object.entries(row.where ?? {});
  const matching = results.filter(
    (r) =>
      r.data.build === build &&
      where.every(([key, value]) => pick(r.data, key) === value) &&
      pick(r.data, row.metric) !== undefined,
  );
  // A gate row counts the newest result of the current phase, the one the latest gate run wrote
  // (D-017); every earlier file stays in docs/results and in the test log. The other rows count
  // the newest result from their first phase on.
  const chosen =
    row.every === 'gate'
      ? matching.filter((r) => r.data.phase === phase).slice(-1)
      : matching
          .filter((r) => at(r.data.phase) >= at(row.from) && at(r.data.phase) <= at(phase))
          .slice(-1);
  const values = [];
  for (const r of chosen) {
    const value = pick(r.data, row.metric);
    if (typeof value === 'number' && Number.isFinite(value)) values.push(value);
    else problems.push(`${r.shown}: ${row.metric} is not a number`);
  }
  return { values, note: chosen.map((r) => r.shown).join(', ') };
}

function gateText(row, unit) {
  if (row.every === 'record') return 'recorded only, no gate';
  if (row.max === null) return 'no gate value';
  return `${row.strict === true ? 'under' : 'at most'} ${String(row.max)}${unit}`;
}

const loaded = loadBudgets();
if (loaded.error !== undefined) {
  console.error(loaded.error);
  process.exit(1);
}

const results = readResults();
let over = false;
let waiting = false;
let rows = 0;

console.log(`Budgets at ${phase}, from scripts/budgets.json:`);
for (const row of loaded.budgets) {
  if (at(row.from) > at(phase)) continue;
  if (row.until !== undefined && at(phase) > at(row.until)) continue;
  rows += 1;
  const unit = typeof row.unit === 'string' ? ` ${row.unit}` : '';
  const measured = row.metric === INSTALLER_METRIC ? installerValues() : resultValues(row, results);
  let result;
  if (measured.error === true) result = 'error';
  else if (row.every === 'record') result = measured.values.length > 0 ? 'recorded' : 'missing';
  else if (row.max === null) result = 'no gate value';
  else if (measured.values.length === 0) result = 'missing';
  else if (measured.values.some((v) => (row.strict === true ? v >= row.max : v > row.max))) {
    result = 'over';
  } else result = 'pass';

  if (result === 'over' || result === 'error') over = true;
  if (result === 'missing' || result === 'no gate value') waiting = true;
  const values =
    measured.values.length > 0
      ? measured.values.map((v) => `${String(v)}${unit}`).join(', ')
      : 'none';
  const by = typeof row.by === 'string' ? row.by : 'not named';
  const source = measured.note === '' ? '' : ` (${measured.note})`;
  console.log(
    [row.id, row.req, gateText(row, unit), `${values}${source}`, result, `measured by ${by}`].join(
      ' | ',
    ),
  );
}
if (rows === 0) console.log('No budget applies at this phase.');

for (const problem of problems) console.error(`Problem: ${problem}`);
if (problems.length > 0) over = true;

if (over) {
  console.log('Exit code 1: a row is over its budget, or a file could not be used.');
  process.exitCode = 1;
} else if (waiting) {
  console.log('Exit code 2: a row has no measurement or no gate value yet (not run or waiting).');
  process.exitCode = 2;
} else {
  console.log('Exit code 0: every row passes.');
  process.exitCode = 0;
}
