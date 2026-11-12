/** The port `vite preview --mode mock` serves dist-mock on during e2e:mock. */
export const MOCK_PORT = 4173;

/** The origin e2e:mock opens; the loopback address only. */
export const MOCK_ORIGIN = `http://127.0.0.1:${String(MOCK_PORT)}`;

/**
 * The WebView2 debugging port. Only the test runner opens it, through the environment of the
 * debug build it starts (REQ-SEC-06).
 */
export const CDP_PORT = 9223;
