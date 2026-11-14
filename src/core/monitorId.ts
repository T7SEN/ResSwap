import type { MonitorId } from './types/MonitorId';

function hasEdidIds(id: MonitorId): boolean {
  return id.edidManufacturerId !== null && id.edidProductCode !== null;
}

/** Whether a request can name this monitor: a device path, or both EDID ids (REQ-DISP-26). */
export function hasIdentity(id: MonitorId): boolean {
  return id.devicePath !== '' || hasEdidIds(id);
}

/**
 * A list key from every identity field, so two monitors that both report no device path
 * (REQ-DISP-26) still get different keys when their EDID ids or connector instances differ.
 */
export function monitorKey(id: MonitorId): string {
  return [
    id.devicePath,
    String(id.edidManufacturerId),
    String(id.edidProductCode),
    String(id.connectorInstance),
  ].join('|');
}

/**
 * Whether two identities name the same monitor, by the core's rule (REQ-DISP-20): the same
 * non-empty device path, or the same EDID manufacturer id, product code and connector
 * instance when both carry both EDID ids. Windows reports no device path for a monitor whose
 * device is disabled in Device Manager (REQ-DISP-26), so an empty path never decides alone.
 */
export function sameMonitor(a: MonitorId, b: MonitorId): boolean {
  if (a.devicePath !== '' && a.devicePath === b.devicePath) {
    return true;
  }
  return (
    hasEdidIds(a) &&
    hasEdidIds(b) &&
    a.edidManufacturerId === b.edidManufacturerId &&
    a.edidProductCode === b.edidProductCode &&
    a.connectorInstance === b.connectorInstance
  );
}
