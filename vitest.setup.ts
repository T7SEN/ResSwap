import '@testing-library/jest-dom/vitest';
import { cleanup } from '@testing-library/react';
import { afterEach } from 'vitest';
import { uninstallMockCore } from '@/core/mock';
import { resetCoreStoreForTests } from '@/core/useCore';

// jsdom 31.0.0 implements crypto.getRandomValues through node:crypto, which mockIPC needs, so no
// polyfill is installed here.

afterEach(() => {
  // Vitest runs without globals, so Testing Library cannot register its own cleanup.
  cleanup();
  // The store next, while the mocked IPC still answers its unsubscribe calls.
  resetCoreStoreForTests();
  // Then the mocked core's timers and every Tauri mock, mockIPC from a test included.
  uninstallMockCore();
});
