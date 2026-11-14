//! Turns what a backend read into the `Monitor` the window shows: vendor, the virtual
//! display rule, the name rule, identity matching and the mode lists (REQ-DISP-02,
//! REQ-DISP-03, REQ-DISP-07, REQ-DISP-08, REQ-DISP-20).

#![forbid(unsafe_code)]

use crate::display::RawMonitor;
use crate::display::modes::{mode_list, presets, to_mode};
use crate::display::types::{GpuVendor, HiddenReason, Monitor, MonitorId};

// DISPLAYCONFIG_VIDEO_OUTPUT_TECHNOLOGY values, copied from windows 0.62.2
// (Win32::Devices::Display), because display code outside win32 holds no Win32 type. The
// backend widens the i32 with i64::from, so INTERNAL (0x80000000) reads as -2147483648.

/// DISPLAYCONFIG_OUTPUT_TECHNOLOGY_OTHER.
pub const OUTPUT_OTHER: i64 = -1;
/// DISPLAYCONFIG_OUTPUT_TECHNOLOGY_HDMI.
pub const OUTPUT_HDMI: i64 = 5;
/// DISPLAYCONFIG_OUTPUT_TECHNOLOGY_DISPLAYPORT_EXTERNAL.
pub const OUTPUT_DISPLAYPORT_EXTERNAL: i64 = 10;
/// DISPLAYCONFIG_OUTPUT_TECHNOLOGY_INDIRECT_WIRED.
pub const OUTPUT_INDIRECT_WIRED: i64 = 16;
/// DISPLAYCONFIG_OUTPUT_TECHNOLOGY_INDIRECT_VIRTUAL.
pub const OUTPUT_INDIRECT_VIRTUAL: i64 = 17;
/// DISPLAYCONFIG_OUTPUT_TECHNOLOGY_DISPLAYPORT_USB_TUNNEL.
pub const OUTPUT_DISPLAYPORT_USB_TUNNEL: i64 = 18;
/// DISPLAYCONFIG_OUTPUT_TECHNOLOGY_INTERNAL, as the backend widens it.
pub const OUTPUT_INTERNAL: i64 = -2_147_483_648;

/// NVIDIA's PCI vendor ID as it starts a device instance ID.
const NVIDIA_PREFIX: &str = r"PCI\VEN_10DE";
/// Every physical GPU's device instance ID starts with this.
const PCI_PREFIX: &str = r"PCI\";

fn starts_with_ignore_case(text: &str, prefix: &str) -> bool {
    text.get(..prefix.len())
        .is_some_and(|head| head.eq_ignore_ascii_case(prefix))
}

/// NVIDIA when the adapter's device instance ID starts with `PCI\VEN_10DE`, otherwise other
/// (REQ-DISP-02). Device instance IDs are not case-sensitive, so the case is ignored.
pub fn vendor_of(adapter_instance_id: &str) -> GpuVendor {
    if starts_with_ignore_case(adapter_instance_id, NVIDIA_PREFIX) {
        GpuVendor::Nvidia
    } else {
        GpuVendor::Other
    }
}

/// Why a monitor is hidden, or `None` for a real monitor (REQ-DISP-07). An adapter that is
/// not a physical GPU comes first, such as the Parsec adapter `ROOT\DISPLAY\0000`; then the
/// connector type the driver reports: INDIRECT_VIRTUAL and INDIRECT_WIRED hide a monitor as
/// an indirect connector, OTHER as another connector. A missing EDID name never hides a
/// monitor on its own. An adapter whose ID cannot be read (an empty string) is not shown as
/// a physical GPU, so it is hidden too; the show-hidden toggle still lists it.
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

/// The name the window receives (REQ-DISP-03): the EDID friendly name with spaces and NUL
/// padding trimmed, or `None` when there is none or nothing is left, never an empty string.
/// The window shows its own fallback text for `None`.
pub fn display_name(name: Option<&str>) -> Option<String> {
    let trimmed = name?.trim_matches(|c: char| c == '\0' || c.is_whitespace());
    (!trimmed.is_empty()).then(|| trimmed.to_owned())
}

/// Whether both ids carry valid EDID ids (edidIdsValid set when they were read).
fn has_edid_ids(id: &MonitorId) -> bool {
    id.edid_manufacturer_id.is_some() && id.edid_product_code.is_some()
}

/// Which of the `live` identities `stored` names (REQ-DISP-20):
/// 1. the one with the same non-empty device path;
/// 2. otherwise the one with the same EDID manufacturer id, product code and connector
///    instance, only when both sides carry valid EDID ids and exactly one live monitor
///    matches; two identical monitors on one connector instance give no match.
///
/// Never the adapter LUID, the GDI device name, the target id or the friendly name: none of
/// them is documented as stable across restarts. `None` is MONITOR_GONE for the caller.
pub fn match_monitor(stored: &MonitorId, live: &[&MonitorId]) -> Option<usize> {
    if !stored.device_path.is_empty()
        && let Some(index) = live
            .iter()
            .position(|candidate| candidate.device_path == stored.device_path)
    {
        return Some(index);
    }
    if !has_edid_ids(stored) {
        return None;
    }
    let mut found = None;
    for (index, candidate) in live.iter().enumerate() {
        let same = has_edid_ids(candidate)
            && candidate.edid_manufacturer_id == stored.edid_manufacturer_id
            && candidate.edid_product_code == stored.edid_product_code
            && candidate.connector_instance == stored.connector_instance;
        if same {
            if found.is_some() {
                return None;
            }
            found = Some(index);
        }
    }
    found
}

/// Whether two stored identities name the same monitor, the pairwise form of
/// `match_monitor` for records that are compared with each other rather than with a fresh
/// read (the journal and the confirmed modes): the same non-empty device path, or the same
/// EDID manufacturer id, product code and connector instance when both carry valid EDID
/// ids (REQ-DISP-20).
pub fn same_monitor(a: &MonitorId, b: &MonitorId) -> bool {
    if !a.device_path.is_empty() && a.device_path == b.device_path {
        return true;
    }
    has_edid_ids(a)
        && has_edid_ids(b)
        && a.edid_manufacturer_id == b.edid_manufacturer_id
        && a.edid_product_code == b.edid_product_code
        && a.connector_instance == b.connector_instance
}

/// The monitor of a fresh read that `stored` names, by the rule of `match_monitor`.
pub fn find_monitor<'a>(stored: &MonitorId, live: &'a [RawMonitor]) -> Option<&'a RawMonitor> {
    let ids: Vec<&MonitorId> = live.iter().map(|monitor| &monitor.id).collect();
    match_monitor(stored, &ids).and_then(|index| live.get(index))
}

/// The monitor the window shows: vendor, hidden rule, name rule, mode list and presets from
/// one read. The native mode is the backend's preferred mode, never the largest listed mode
/// (REQ-DISP-04). A hidden monitor gets no stretched presets: ResSwap offers stretched sizes
/// for real monitors only, and the player can still pick a listed mode with the show-hidden
/// toggle on (REQ-DISP-07, REQ-DISP-08).
pub fn to_monitor(raw: &RawMonitor) -> Monitor {
    let hidden_reason = hidden_reason(&raw.adapter_instance_id, raw.output_technology);
    let presets = if hidden_reason.is_some() {
        Vec::new()
    } else {
        presets(raw.native, raw.current, &raw.modes)
    };
    Monitor {
        id: raw.id.clone(),
        name: display_name(raw.name.as_deref()),
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
        presets,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::apply::OpenEntries;
    use crate::display::fake::{FakeDisplay, FixtureProfile, known_monitors};
    use crate::display::types::Aspect;
    use crate::display::{DisplayBackend, Rational, RawMode};

    fn id(path: &str, edid: Option<(u16, u16)>, connector: u32) -> MonitorId {
        MonitorId {
            device_path: String::from(path),
            edid_manufacturer_id: edid.map(|(manufacturer, _)| manufacturer),
            edid_product_code: edid.map(|(_, product)| product),
            connector_instance: connector,
        }
    }

    fn mode(width: u32, height: u32, hz: u32) -> RawMode {
        RawMode {
            width,
            height,
            refresh: Rational::hz(hz),
        }
    }

    #[test]
    fn req_disp_07_virtual_displays_are_hidden() {
        // Parsec's adapter on the test PC is ROOT\DISPLAY\0000.
        assert_eq!(
            hidden_reason(r"ROOT\DISPLAY\0000", OUTPUT_INDIRECT_VIRTUAL),
            Some(HiddenReason::VirtualAdapter)
        );
        assert_eq!(
            hidden_reason(r"root\display\0000", OUTPUT_HDMI),
            Some(HiddenReason::VirtualAdapter)
        );
        assert_eq!(
            hidden_reason("", OUTPUT_HDMI),
            Some(HiddenReason::VirtualAdapter)
        );
        let gpu = r"PCI\VEN_10DE&DEV_2803";
        assert_eq!(
            hidden_reason(gpu, OUTPUT_INDIRECT_VIRTUAL),
            Some(HiddenReason::IndirectConnector)
        );
        assert_eq!(
            hidden_reason(gpu, OUTPUT_INDIRECT_WIRED),
            Some(HiddenReason::IndirectConnector)
        );
        assert_eq!(
            hidden_reason(gpu, OUTPUT_OTHER),
            Some(HiddenReason::OtherConnector)
        );
        for real in [
            OUTPUT_HDMI,
            OUTPUT_DISPLAYPORT_EXTERNAL,
            OUTPUT_DISPLAYPORT_USB_TUNNEL,
            OUTPUT_INTERNAL,
        ] {
            assert_eq!(hidden_reason(gpu, real), None, "{real}");
        }
        assert_eq!(hidden_reason(r"pci\ven_8086&dev_0000", OUTPUT_HDMI), None);
    }

    #[test]
    fn req_disp_07_output_technology_values_match_the_win32_constants() {
        // The values windows 0.62.2 gives the DISPLAYCONFIG_OUTPUT_TECHNOLOGY_* constants.
        assert_eq!(OUTPUT_OTHER, i64::from(-1_i32));
        assert_eq!(OUTPUT_INDIRECT_WIRED, i64::from(16_i32));
        assert_eq!(OUTPUT_INDIRECT_VIRTUAL, i64::from(17_i32));
        assert_eq!(OUTPUT_INTERNAL, i64::from(i32::MIN));
    }

    #[test]
    fn req_disp_08_hidden_monitors_are_still_listed_with_a_reason() {
        let display = FakeDisplay::new(FixtureProfile::TestPc, OpenEntries::default());
        let monitors: Vec<Monitor> = display
            .enumerate()
            .unwrap()
            .iter()
            .map(to_monitor)
            .collect();
        assert_eq!(monitors.len(), 2);
        let lg = &monitors[0];
        assert!(!lg.hidden);
        assert_eq!(lg.hidden_reason, None);
        assert_eq!(lg.presets.len(), 3);
        let parsec = &monitors[1];
        assert!(parsec.hidden);
        assert_eq!(parsec.hidden_reason, Some(HiddenReason::VirtualAdapter));
        assert_eq!(parsec.name, None);
        // Listed modes stay; stretched presets are for real monitors only.
        assert!(!parsec.modes.is_empty());
        assert!(parsec.presets.is_empty());
    }

    #[test]
    fn req_disp_02_vendor_from_the_adapter_instance_id() {
        assert_eq!(vendor_of(r"PCI\VEN_10DE&DEV_2803"), GpuVendor::Nvidia);
        assert_eq!(vendor_of(r"pci\ven_10de&dev_2803"), GpuVendor::Nvidia);
        assert_eq!(vendor_of(r"PCI\VEN_8086&DEV_0000"), GpuVendor::Other);
        assert_eq!(vendor_of(r"PCI\VEN_10D"), GpuVendor::Other);
        assert_eq!(vendor_of(r"ROOT\DISPLAY\0000"), GpuVendor::Other);
        assert_eq!(vendor_of(""), GpuVendor::Other);
    }

    #[test]
    fn req_disp_02_other_vendor_gets_the_display_core() {
        let raw = known_monitors(FixtureProfile::NotNvidia).remove(0);
        let monitor = to_monitor(&raw);
        assert_eq!(monitor.vendor, GpuVendor::Other);
        assert!(!monitor.hidden);
        assert_eq!(
            monitor.native.map(|native| native.label),
            Some(String::from("1920 x 1080 at 60 Hz"))
        );
        assert_eq!(monitor.modes.len(), 3);
        let presets: Vec<(Aspect, u32, bool)> = monitor
            .presets
            .iter()
            .map(|preset| (preset.aspect, preset.mode.width, preset.offered))
            .collect();
        assert_eq!(
            presets,
            vec![
                (Aspect::FourThree, 1440, true),
                (Aspect::FiveFour, 1350, false),
                (Aspect::SixteenTen, 1728, false),
            ]
        );
    }

    #[test]
    fn req_disp_03_name_is_null_without_an_edid_name() {
        assert_eq!(display_name(None), None);
        assert_eq!(display_name(Some("")), None);
        assert_eq!(display_name(Some("  \0\0")), None);
        assert_eq!(
            display_name(Some("LG ULTRAGEAR\0\0\0")),
            Some(String::from("LG ULTRAGEAR"))
        );
        assert_eq!(
            display_name(Some(" Samsung LF24T35 ")),
            Some(String::from("Samsung LF24T35"))
        );
        let mut raw = known_monitors(FixtureProfile::TestPc).remove(0);
        raw.name = Some(String::new());
        assert_eq!(to_monitor(&raw).name, None);
    }

    #[test]
    fn req_disp_04_native_is_the_preferred_mode_not_the_largest() {
        let mut raw = known_monitors(FixtureProfile::TestPc).remove(0);
        // A larger size in the list must not become the native mode.
        raw.modes.push(mode(2560, 1440, 60));
        let monitor = to_monitor(&raw);
        assert_eq!(
            monitor.native.map(|native| native.label),
            Some(String::from("1920 x 1080 at 240 Hz"))
        );
        assert_eq!(
            monitor.modes.first().map(|first| first.label.as_str()),
            Some("2560 x 1440 at 60 Hz")
        );
        // Presets follow the preferred height, 1080, not the largest mode's 1440.
        assert!(monitor.presets.iter().all(|p| p.mode.height == 1080));
        // No preferred mode reported: no native and no presets, never a guess.
        raw.native = None;
        let monitor = to_monitor(&raw);
        assert_eq!(monitor.native, None);
        assert!(monitor.presets.is_empty());
    }

    #[test]
    fn req_disp_20_identity_matches_device_path_first() {
        let lg = id("path-lg", Some((0x1E6D, 0x5B8E)), 0);
        let other = id("path-other", Some((0x1E6D, 0x5B8E)), 0);
        let live = [&other, &lg];
        // The device path wins even when another monitor has the same EDID ids.
        assert_eq!(match_monitor(&lg, &live), Some(1));
    }

    #[test]
    fn req_disp_20_edid_ids_plus_connector_second() {
        // The monitor moved to another port, so its device path changed.
        let stored = id("old-path", Some((0x1E6D, 0x5B8E)), 0);
        let moved = id("new-path", Some((0x1E6D, 0x5B8E)), 0);
        let unrelated = id("path-x", Some((0x4C2D, 0x0001)), 0);
        assert_eq!(match_monitor(&stored, &[&unrelated, &moved]), Some(1));
        // Another connector instance is another monitor.
        let other_connector = id("new-path", Some((0x1E6D, 0x5B8E)), 1);
        assert_eq!(match_monitor(&stored, &[&other_connector]), None);
    }

    #[test]
    fn req_disp_20_two_identical_monitors_never_match_by_edid() {
        let stored = id("old-path", Some((0x1E6D, 0x5B8E)), 0);
        let first = id("path-1", Some((0x1E6D, 0x5B8E)), 0);
        let second = id("path-2", Some((0x1E6D, 0x5B8E)), 0);
        assert_eq!(match_monitor(&stored, &[&first, &second]), None);
    }

    #[test]
    fn req_disp_20_edid_fallback_needs_valid_ids_on_both_sides() {
        let stored = id("old-path", None, 0);
        let live = id("new-path", None, 0);
        assert_eq!(match_monitor(&stored, &[&live]), None);
        let stored = id("old-path", Some((0x1E6D, 0x5B8E)), 0);
        assert_eq!(match_monitor(&stored, &[&live]), None);
        let half = MonitorId {
            edid_product_code: None,
            ..id("new-path", Some((0x1E6D, 0x5B8E)), 0)
        };
        assert_eq!(match_monitor(&stored, &[&half]), None);
        // An empty device path never matches by path.
        let blank = id("", None, 0);
        assert_eq!(match_monitor(&blank, &[&id("", None, 0)]), None);
    }

    #[test]
    fn req_disp_20_never_matches_by_gdi_name_or_adapter() {
        let monitors = known_monitors(FixtureProfile::TwoMonitors);
        let lg = monitors[0].clone();
        let mut live = monitors;
        // After a restart the LG can come back as \\.\DISPLAY3 on a new adapter path.
        live[0].gdi_name = String::from(r"\\.\DISPLAY3");
        live[0].adapter_device_path = String::from("another-adapter-path");
        live[2].gdi_name = String::from(r"\\.\DISPLAY1");
        let found = find_monitor(&lg.id, &live).map(|monitor| monitor.id.clone());
        assert_eq!(found, Some(lg.id.clone()));
        // A monitor whose path and EDID ids are unknown is gone, whatever its GDI name.
        let stranger = id("unknown-path", None, 0);
        assert!(find_monitor(&stranger, &live).is_none());
    }

    #[test]
    fn req_disp_20_pairwise_identity_for_stored_records() {
        let lg = id("fake-device-path:lg", Some((0x1E6D, 0x5B8E)), 0);
        // The same device path, whatever else differs.
        assert!(same_monitor(&lg, &id("fake-device-path:lg", None, 3)));
        // Moved to another port: the EDID ids and connector instance still match.
        assert!(same_monitor(&lg, &id("moved", Some((0x1E6D, 0x5B8E)), 0)));
        assert!(!same_monitor(&lg, &id("moved", Some((0x1E6D, 0x5B8E)), 1)));
        // No EDID ids on one side: only the device path counts, and an empty path never does.
        assert!(!same_monitor(&lg, &id("moved", None, 0)));
        assert!(!same_monitor(&id("", None, 0), &id("", None, 0)));
    }
}
