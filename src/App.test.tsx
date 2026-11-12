import { render, screen, within } from '@testing-library/react';
import { StrictMode } from 'react';
import { beforeEach, describe, expect, test } from 'vitest';
import { App } from './App';
import { installMockCore } from './core/mock';

function withMockedCore(query: string): void {
  installMockCore(new URLSearchParams(query));
}

function marks(name: string): PerformanceEntryList {
  return performance.getEntriesByName(name, 'mark');
}

beforeEach(() => {
  performance.clearMarks();
});

describe('the window at this build', () => {
  test('REQ-UI-15 a main landmark holds the heading ResSwap and the state read from the core', async () => {
    withMockedCore('scenario=home-native');
    render(<App hasCore />);
    const main = screen.getByRole('main');
    expect(within(main).getByRole('heading', { level: 1, name: 'ResSwap' })).toBeInTheDocument();
    expect(
      await within(main).findByText('Primary monitor is on 1920 x 1080 at 240 Hz.'),
    ).toBeInTheDocument();
    expect(main).toHaveAttribute('aria-busy', 'false');
    expect(screen.queryByText(/fake display backend/)).not.toBeInTheDocument();
  });

  test('REQ-UI-16 the readout shows the mode the core reports, never one the window builds', async () => {
    withMockedCore('scenario=home-stretched');
    render(<App hasCore />);
    expect(
      await screen.findByText('Primary monitor is on 1280 x 960 at 240 Hz.'),
    ).toBeInTheDocument();
  });

  test('REQ-UI-08 a failed get_state shows the message for its code', async () => {
    withMockedCore('fail=get_state:INTERNAL');
    render(<App hasCore />);
    expect(
      await screen.findByText('Something went wrong inside ResSwap. The log has the details.'),
    ).toBeInTheDocument();
  });

  test('REQ-UI-08 an error raised before the window opened is listed with its source', async () => {
    withMockedCore('scenario=recovery-incomplete');
    render(<App hasCore />);
    expect(
      await screen.findByText(
        'Start-up: ResSwap could not put everything back. The list below shows what is still changed. Try Restore everything again.',
      ),
    ).toBeInTheDocument();
  });

  test('REQ-UI-01 a build on the fake backend says so, so its monitors are not taken as real', async () => {
    withMockedCore('scenario=fake-backend');
    render(<App hasCore />);
    expect(
      await screen.findByText(
        'Test build: ResSwap is using its fake display backend, so nothing on this PC changes.',
      ),
    ).toBeInTheDocument();
  });

  test('REQ-UI-17 with no core and no mock the window only says where it runs', () => {
    render(<App hasCore={false} />);
    expect(screen.getByRole('heading', { level: 1, name: 'ResSwap' })).toBeInTheDocument();
    expect(screen.getByText('This window runs inside ResSwap.')).toBeInTheDocument();
  });
});

describe('the first-paint marks', () => {
  test('REQ-PERF-05 App marks ResSwap:shell once after its first commit, StrictMode included', () => {
    const { rerender } = render(
      <StrictMode>
        <App hasCore={false} />
      </StrictMode>,
    );
    expect(marks('ResSwap:shell')).toHaveLength(1);

    rerender(
      <StrictMode>
        <App hasCore={false} />
      </StrictMode>,
    );
    expect(marks('ResSwap:shell')).toHaveLength(1);
  });

  test('REQ-PERF-05 the shell mark comes first, then the ready mark once the state arrives', async () => {
    withMockedCore('scenario=home-native');
    render(<App hasCore />);
    const [shell] = marks('ResSwap:shell');
    expect(shell).toBeDefined();
    expect(marks('ResSwap:ready')).toHaveLength(0);

    expect(
      await screen.findByText('Primary monitor is on 1920 x 1080 at 240 Hz.'),
    ).toBeInTheDocument();
    const [ready] = marks('ResSwap:ready');
    expect(ready).toBeDefined();
    expect(ready!.startTime).toBeGreaterThanOrEqual(shell!.startTime);
  });
});
