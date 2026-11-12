import { type ReactNode, useEffect } from 'react';
import { errorMessage } from '@/core/errors';
import type { AppState } from '@/core/types/AppState';
import type { ErrorSource } from '@/core/types/ErrorSource';
import type { Monitor } from '@/core/types/Monitor';
import { useCore } from '@/core/useCore';

export type AppProps = {
  /** False in a normal browser with no mocked core: nothing answers the window's calls. */
  hasCore: boolean;
};

const SOURCE_LABELS: Record<ErrorSource, string> = {
  watcher: 'Auto-switch',
  hotkey: 'Hotkey',
  tray: 'Tray menu',
  playShortcut: 'Play shortcut',
  startup: 'Start-up',
  resume: 'After sleep',
  displayChange: 'Display change',
  updateCheck: 'Update check',
};

type FrameProps = {
  isBusy: boolean;
  children: ReactNode;
};

function Frame({ isBusy, children }: FrameProps) {
  return (
    <main
      id="content"
      tabIndex={-1}
      aria-busy={isBusy}
      className="mx-auto flex max-w-2xl flex-col gap-3 p-4 text-sm md:p-6"
    >
      <h1 className="text-xl font-semibold">ResSwap</h1>
      {children}
    </main>
  );
}

/** The monitor Home will show: the chosen one, else the primary one, else the first. */
function shownMonitor(state: AppState): Monitor | undefined {
  const visible = state.monitors.filter(
    (monitor) => !monitor.hidden || state.settings.showHiddenMonitors,
  );
  const selected = state.settings.selectedMonitor;
  const chosen =
    selected === null
      ? undefined
      : visible.find((monitor) => monitor.id.devicePath === selected.devicePath);
  return chosen ?? visible.find((monitor) => monitor.primary) ?? visible[0];
}

function monitorLine(monitor: Monitor | undefined): string {
  if (monitor === undefined) {
    return 'ResSwap found no monitor it can change.';
  }
  const name = monitor.name ?? 'Unnamed monitor';
  // The label comes from the core; the window never builds one.
  return monitor.current === null
    ? `${name} is connected but turned off.`
    : `${name} is on ${monitor.current.label}.`;
}

function Readout({ state }: { state: AppState }) {
  return (
    <>
      <p className="tabular-nums">{monitorLine(shownMonitor(state))}</p>
      {state.errors.length === 0 ? null : (
        <ul className="flex flex-col gap-2">
          {state.errors.map((raised) => (
            <li key={raised.id}>
              {SOURCE_LABELS[raised.source]}: {errorMessage(raised.error)}
              {raised.count > 1 ? ` (${String(raised.count)} times)` : null}
            </li>
          ))}
        </ul>
      )}
      {state.build.backend === 'fake' ? (
        <p className="text-muted-foreground">
          Test build: ResSwap is using its fake display backend, so nothing on this PC changes.
        </p>
      ) : null}
    </>
  );
}

function CoreWindow() {
  const core = useCore();
  switch (core.status) {
    case 'loading':
      return (
        <Frame isBusy>
          <p>ResSwap is reading the display settings.</p>
        </Frame>
      );
    case 'failed':
      return (
        <Frame isBusy={false}>
          <p>{errorMessage(core.error)}</p>
        </Frame>
      );
    case 'ready':
      return (
        <Frame isBusy={false}>
          <Readout state={core.state} />
        </Frame>
      );
  }
}

// The first-paint measurement reads this mark beside first contentful paint (REQ-PERF-05).
const SHELL_MARK = 'ResSwap:shell';

/** The window at this build: the core's state in short, read from the core and never kept. */
export function App({ hasCore }: AppProps) {
  useEffect(() => {
    // After the first commit only. StrictMode runs an effect twice in development, so the page
    // keeps the first mark and never adds a second.
    if (performance.getEntriesByName(SHELL_MARK, 'mark').length === 0) {
      performance.mark(SHELL_MARK);
    }
  }, []);

  if (!hasCore) {
    return (
      <Frame isBusy={false}>
        <p>This window runs inside ResSwap.</p>
      </Frame>
    );
  }
  return <CoreWindow />;
}
