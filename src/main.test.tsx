import { afterEach, beforeEach, describe, expect, test, vi } from 'vitest';

// The boot is tested with the mocked core and the app replaced by recorders, so each test sees
// only what main.tsx decides: whether to install the mock, and what it tells the app.
const seen = vi.hoisted(() => ({
  installs: [] as string[],
  renders: [] as boolean[],
}));

vi.mock('./core/mock', () => ({
  installMockCore: (params: URLSearchParams) => {
    seen.installs.push(params.toString());
  },
  uninstallMockCore: () => undefined,
}));

vi.mock('./App', () => ({
  App: ({ hasCore }: { hasCore: boolean }) => {
    seen.renders.push(hasCore);
    return null;
  },
}));

async function boot(): Promise<void> {
  vi.resetModules();
  await import('./main');
  await vi.waitFor(() => {
    expect(seen.renders.length).toBeGreaterThan(0);
  });
}

beforeEach(() => {
  seen.installs.length = 0;
  seen.renders.length = 0;
  document.body.innerHTML = '<div id="root"></div>';
  window.history.replaceState(null, '', '/?scenario=first-run');
});

afterEach(() => {
  vi.unstubAllEnvs();
  Reflect.deleteProperty(globalThis, 'isTauri');
});

describe('the boot', () => {
  test('REQ-UI-17 a mock build in a normal browser installs the mocked core before the app', async () => {
    vi.stubEnv('VITE_MOCK_CORE', '1');
    await boot();
    expect(seen.installs).toEqual(['scenario=first-run']);
    expect(seen.renders[0]).toBe(true);
  });

  test('REQ-UI-17 a normal build never installs the mocked core', async () => {
    vi.stubEnv('VITE_MOCK_CORE', '');
    await boot();
    expect(seen.installs).toEqual([]);
    // In a normal browser nothing answers, so the app says where it runs.
    expect(seen.renders[0]).toBe(false);
  });

  test('REQ-UI-17 inside ResSwap the mocked core stays out even if the flag leaked', async () => {
    vi.stubEnv('VITE_MOCK_CORE', '1');
    Object.defineProperty(globalThis, 'isTauri', { value: true, configurable: true });
    await boot();
    expect(seen.installs).toEqual([]);
    expect(seen.renders[0]).toBe(true);
  });
});
