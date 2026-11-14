import { type ReactNode, useEffect } from 'react';
import { currentWindowLabel } from '@/core/client';
import { errorMessage } from '@/core/errors';
import { useCore } from '@/core/useCore';
import { Home } from '@/screens/Home';
import { KeepMode } from '@/screens/KeepMode';

export type AppProps = {
  /** False in a normal browser with no mocked core: nothing answers the window's calls. */
  hasCore: boolean;
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
      className="mx-auto flex max-w-2xl flex-col gap-6 p-4 text-sm md:p-6"
    >
      <h1 className="text-xl font-semibold">ResSwap</h1>
      {children}
    </main>
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
        <Frame isBusy={core.state.busy !== null}>
          <Home state={core.state} />
        </Frame>
      );
  }
}

// The first-paint measurement reads this mark beside first contentful paint (REQ-PERF-05).
const SHELL_MARK = 'ResSwap:shell';

/**
 * The window at this build: Home in the main window, the keep-this-mode prompt in the window
 * labelled keep-mode. Both read the core's state and never keep their own copy of it.
 */
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
  // The label is read only once a core exists: outside ResSwap and the mock there is none.
  return currentWindowLabel() === 'keep-mode' ? <KeepMode /> : <CoreWindow />;
}
