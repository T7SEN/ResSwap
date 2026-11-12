import { test as base, expect } from '@playwright/test';
import { blockOutside, MOCK_HOSTS, type NetworkLog, watchNetwork } from './network';

type MockFixtures = {
  /** Requests of the current test; any request outside the preview server fails the test. */
  network: NetworkLog;
};

/** The e2e:mock test: every test blocks and records requests outside the local origin. */
export const test = base.extend<MockFixtures>({
  network: [
    async ({ context }, use) => {
      await blockOutside(context, MOCK_HOSTS);
      const log = watchNetwork(context, MOCK_HOSTS);
      await use(log);
      expect(log.outside(), 'requests outside the local origin (REQ-PRIV-03)').toEqual([]);
    },
    { auto: true },
  ],
});

export { expect };
