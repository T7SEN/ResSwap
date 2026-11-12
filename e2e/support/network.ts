import type { BrowserContext, Page } from '@playwright/test';

/** Hosts e2e:mock may reach: the preview server on the loopback address (REQ-PRIV-03). */
export const MOCK_HOSTS = ['127.0.0.1', 'localhost'] as const;

/**
 * Hosts the real window may reach: its own origin and the IPC origin. On Windows, tauri 2.12.5
 * serves the bundled files from http://tauri.localhost and answers commands on
 * http://ipc.localhost (src/protocol/mod.rs, `origin`).
 */
export const APP_HOSTS = ['tauri.localhost', 'ipc.localhost'] as const;

const LOCAL_SCHEMES = new Set(['about:', 'blob:', 'data:']);

/** True when the URL stays on the page itself or goes to one of the given hosts. */
export function isAllowedUrl(url: string, hosts: readonly string[]): boolean {
  let parsed: URL;
  try {
    parsed = new URL(url);
  } catch {
    return false;
  }
  return LOCAL_SCHEMES.has(parsed.protocol) || hosts.includes(parsed.hostname);
}

/** Every URL the pages of one context asked for, in order. */
export type NetworkLog = {
  readonly seen: readonly string[];
  /** The URLs that went to a host outside the allowed list. */
  outside: () => string[];
};

/** Records every request and WebSocket of the context's pages, present and future. */
export function watchNetwork(context: BrowserContext, hosts: readonly string[]): NetworkLog {
  const seen: string[] = [];
  const watchPage = (page: Page): void => {
    page.on('websocket', (socket) => {
      seen.push(socket.url());
    });
  };
  context.on('request', (request) => {
    seen.push(request.url());
  });
  context.pages().forEach(watchPage);
  context.on('page', watchPage);
  return {
    seen,
    outside: () => seen.filter((url) => !isAllowedUrl(url, hosts)),
  };
}

/** Aborts every request to a host outside the list before it leaves the browser. */
export async function blockOutside(
  context: BrowserContext,
  hosts: readonly string[],
): Promise<void> {
  await context.route(
    (url) => !isAllowedUrl(url.href, hosts),
    (route) => route.abort('blockedbyclient'),
  );
}
