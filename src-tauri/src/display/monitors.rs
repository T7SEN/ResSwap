//! Turns what a backend read into the `Monitor` the window shows: vendor, the virtual
//! display rule and the mode lists (REQ-DISP-07, REQ-DISP-08).

#![forbid(unsafe_code)]

use crate::display::RawMonitor;
use crate::display::modes::{mode_list, presets, to_mode};
use crate::display::types::{GpuVendor, HiddenReason, Monitor};

/// DISPLAYCONFIG_OUTPUT_TECHNOLOGY_OTHER.
const OUTPUT_OTHER: i64 = -1;
/// DISPLAYCONFIG_OUTPUT_TECHNOLOGY_INDIRECT_WIRED.
const OUTPUT_INDIRECT_WIRED: i64 = 16;
/// DISPLAYCONFIG_OUTPUT_TECHNOLOGY_INDIRECT_VIRTUAL.
const OUTPUT_INDIRECT_VIRTUAL: i64 = 17;

/// NVIDIA's PCI vendor ID as it starts a device instance ID.
const NVIDIA_PREFIX: &str = r"PCI\VEN_10DE";
/// Every physical GPU's device instance ID starts with this.
const PCI_PREFIX: &str = r"PCI\";

fn starts_with_ignore_case(text: &str, prefix: &str) -> bool {
    text.get(..prefix.len())
        .is_some_and(|head| head.eq_ignore_ascii_case(prefix))
}

/// NVIDIA when the adapter's device instance ID starts with `PCI\VEN_10DE`, otherwise other.
pub fn vendor_of(adapter_instance_id: &str) -> GpuVendor {
    if starts_with_ignore_case(adapter_instance_id, NVIDIA_PREFIX) {
        GpuVendor::Nvidia
    } else {
        GpuVendor::Other
    }
}

/// Why a monitor is hidden, or `None` for a real monitor. An adapter that is not a
/// physical GPU comes first; then the connector type the driver reports. A missing EDID
/// name never hides a monitor on its own.
pub fn hidden_reason(adapter_instance_id: &str, output_technology: i64) -> Option<HiddenReason> {
    if !starts_with_ignore_case(adapter_instance_id, PCI_PREFIX) {
        return Some(HiddenReason::VirtualAdapter);
    }
    match output_technology {
        OUTPUT_INDIRECT_WIRED | OUTPUT_INDIRECT_VIRTUAL => Some(HiddenReason::IndirectConnector),
        OUTPUT_OTHER => Some(HiddenReason::OtherConnector),
        _ => None,
    }
}

/// The monitor the window shows: vendor, hidden rule, mode list and presets from one read.
pub fn to_monitor(raw: &RawMonitor) -> Monitor {
    let hidden_reason = hidden_reason(&raw.adapter_instance_id, raw.output_technology);
    Monitor {
        id: raw.id.clone(),
        name: raw.name.clone(),
        adapter: raw.adapter_name.clone(),
        vendor: vendor_of(&raw.adapter_instance_id),
        primary: raw.primary,
        active: raw.active,
        hidden: hidden_reason.is_some(),
        hidden_reason,
        native: raw.native.map(to_mode),
        current: raw.current.map(to_mode),
        scaling: raw.scaling,
        modes: mode_list(&raw.modes),
        presets: presets(raw.native, raw.current, &raw.modes),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn req_disp_07_virtual_displays_are_hidden() {
        // Parsec's adapter on the test PC is ROOT\DISPLAY\0000.
        assert_eq!(
            hidden_reason(r"ROOT\DISPLAY\0000", OUTPUT_INDIRECT_VIRTUAL),
            Some(HiddenReason::VirtualAdapter)
        );
        assert_eq!(
            hidden_reason(r"PCI\VEN_10DE&DEV_2803", OUTPUT_INDIRECT_VIRTUAL),
            Some(HiddenReason::IndirectConnector)
        );
        assert_eq!(
            hidden_reason(r"PCI\VEN_10DE&DEV_2803", OUTPUT_OTHER),
            Some(HiddenReason::OtherConnector)
        );
        // DISPLAYCONFIG_OUTPUT_TECHNOLOGY_DISPLAYPORT_EXTERNAL is 10.
        assert_eq!(hidden_reason(r"PCI\VEN_10DE&DEV_2803", 10), None);
    }

    #[test]
    fn req_disp_02_vendor_from_the_adapter_instance_id() {
        assert_eq!(vendor_of(r"PCI\VEN_10DE&DEV_2803"), GpuVendor::Nvidia);
        assert_eq!(vendor_of(r"pci\ven_10de&dev_2803"), GpuVendor::Nvidia);
        assert_eq!(vendor_of(r"PCI\VEN_8086&DEV_0000"), GpuVendor::Other);
        assert_eq!(vendor_of(r"ROOT\DISPLAY\0000"), GpuVendor::Other);
        assert_eq!(vendor_of(""), GpuVendor::Other);
    }
}
