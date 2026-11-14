import { describe, expect, test } from 'vitest';
import { hasIdentity, monitorKey, sameMonitor } from './monitorId';
import type { MonitorId } from './types/MonitorId';

function id(
  devicePath: string,
  edid: [number, number] | null,
  connectorInstance: number,
): MonitorId {
  return {
    devicePath,
    edidManufacturerId: edid === null ? null : edid[0],
    edidProductCode: edid === null ? null : edid[1],
    connectorInstance,
  };
}

describe('sameMonitor', () => {
  test('REQ-DISP-20 matches on a non-empty device path first', () => {
    expect(sameMonitor(id('path-a', null, 0), id('path-a', [1, 2], 3))).toBe(true);
    expect(sameMonitor(id('path-a', null, 0), id('path-b', null, 0))).toBe(false);
  });

  test('REQ-DISP-20 falls back to the EDID ids and connector instance', () => {
    expect(sameMonitor(id('old-path', [1, 2], 0), id('new-path', [1, 2], 0))).toBe(true);
    expect(sameMonitor(id('old-path', [1, 2], 0), id('new-path', [1, 2], 1))).toBe(false);
    expect(sameMonitor(id('old-path', [1, 2], 0), id('new-path', null, 0))).toBe(false);
  });

  test('REQ-DISP-26 tells apart two monitors that report no device path', () => {
    // Both monitors of the test PC with their devices disabled in Device Manager.
    const lg = id('', [0x6d1e, 0x5b8e], 1);
    const samsung = id('', [0x2d4c, 0x707d], 0);
    expect(sameMonitor(lg, lg)).toBe(true);
    expect(sameMonitor(lg, samsung)).toBe(false);
    expect(sameMonitor(id('', null, 0), id('', null, 0))).toBe(false);
  });
});

describe('monitorKey', () => {
  test('REQ-DISP-26 gives two monitors without a device path different keys', () => {
    const lg = id('', [0x6d1e, 0x5b8e], 1);
    const samsung = id('', [0x2d4c, 0x707d], 0);
    expect(monitorKey(lg)).not.toBe(monitorKey(samsung));
    expect(monitorKey(lg)).toBe(monitorKey(id('', [0x6d1e, 0x5b8e], 1)));
  });
});

describe('hasIdentity', () => {
  test('REQ-DISP-26 a monitor needs a device path or both EDID ids', () => {
    expect(hasIdentity(id('path-a', null, 0))).toBe(true);
    expect(hasIdentity(id('', [0x6d1e, 0x5b8e], 1))).toBe(true);
    expect(hasIdentity(id('', null, 0))).toBe(false);
  });
});
