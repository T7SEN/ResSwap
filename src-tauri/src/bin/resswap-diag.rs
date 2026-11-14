//! The read-only diagnostics console tool (REQ-DISP-25). Built only with the diag feature, so
//! the app build and its installer leave it out. It reads through the same Win32 display
//! backend and registry reader the app uses and calls nothing that changes the PC: no
//! ChangeDisplaySettingsExW, no SetDisplayConfig, no registry write.

#![forbid(unsafe_code)]

use std::collections::BTreeSet;
use std::process::ExitCode;

use resswap_lib::display::monitors::to_monitor;
use resswap_lib::display::win32::ccd::{
    EDID_IDS_VALID, FRIENDLY_NAME_FORCED, FRIENDLY_NAME_FROM_EDID, PreferredMode, TargetName,
};
use resswap_lib::display::win32::hw::{NoteLevel, hw_parse};
use resswap_lib::display::win32::{
    HalfReading, PathRead, QueryForm, ReadError, Win32Display, device, gdi, query_forms, read_paths,
};
use resswap_lib::display::{DisplayBackend, Rational, RawMode, RawMonitor};
use resswap_lib::system::registry::{display_class_keys, read_class_value};
use resswap_lib::vendor::nvidia::nv_modes::RawKind;

/// Every line of the report goes out here, the only place this tool prints.
#[allow(
    clippy::print_stdout,
    reason = "resswap-diag is a console tool; its report on stdout is its whole output"
)]
fn out(line: &str) {
    println!("{line}");
}

fn rate(value: Option<Rational>) -> String {
    value.map_or_else(
        || String::from("not reported"),
        |rate| format!("{}/{}", rate.numerator, rate.denominator),
    )
}

fn mode(value: Option<RawMode>) -> String {
    value.map_or_else(
        || String::from("not reported"),
        |mode| {
            format!(
                "{} x {} at {}",
                mode.width,
                mode.height,
                rate(Some(mode.refresh))
            )
        },
    )
}

fn main() -> ExitCode {
    out("resswap-diag: read-only display diagnostics. Nothing on this PC is changed.");
    out("");
    route_section();
    let monitors = monitor_section();
    paths_section();
    gdi_section();
    adapter_section(monitors.as_deref().unwrap_or_default());
    query_section();
    registry_section();
    if monitors.is_some() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

fn route_section() {
    let parsed = hw_parse();
    out("== Route");
    out(&format!(
        "build: {}",
        if cfg!(debug_assertions) {
            "debug (RESSWAP_HW is read)"
        } else {
            "release (RESSWAP_HW is ignored)"
        }
    ));
    out(&format!(
        "route: {:?}; GDI flag: {:?}; scaling control: {:?}",
        parsed.choice.route.route, parsed.choice.route.gdi_flag, parsed.choice.scaling
    ));
    for note in &parsed.notes {
        let level = match note.level {
            NoteLevel::Info => "note",
            NoteLevel::Warn => "warning",
        };
        out(&format!("RESSWAP_HW {level}: {}", note.text));
    }
    out("");
}

fn monitor_section() -> Option<Vec<RawMonitor>> {
    out("== Monitors (as the app lists them)");
    let monitors = match Win32Display.enumerate() {
        Ok(monitors) => monitors,
        Err(error) => {
            out(&format!(
                "enumerate failed: {:?}: {}",
                error.code(),
                error.to_ipc().detail
            ));
            out("");
            return None;
        }
    };
    for (index, raw) in monitors.iter().enumerate() {
        let shown = to_monitor(raw);
        out(&format!(
            "monitor {}: {}",
            index + 1,
            raw.name.as_deref().unwrap_or("(no EDID name)")
        ));
        out(&format!("  device path: {}", raw.id.device_path));
        out(&format!(
            "  EDID ids: {}; connector instance: {}",
            raw.id
                .edid_manufacturer_id
                .zip(raw.id.edid_product_code)
                .map_or_else(
                    || String::from("not valid"),
                    |(manufacturer, product)| format!("{manufacturer:#06x} {product:#06x}")
                ),
            raw.id.connector_instance
        ));
        out(&format!("  adapter: {}", raw.adapter_name));
        out(&format!(
            "  adapter instance ID: {}",
            raw.adapter_instance_id
        ));
        out(&format!(
            "  adapter device path: {}",
            raw.adapter_device_path
        ));
        out(&format!(
            "  outputTechnology: {}; primary: {}; active: {}",
            raw.output_technology, raw.primary, raw.active
        ));
        out(&format!(
            "  virtual verdict: hidden {}, reason {:?}; vendor {:?}",
            shown.hidden, shown.hidden_reason, shown.vendor
        ));
        out(&format!(
            "  GDI device name: {}; position: {}, {}",
            raw.gdi_name, raw.position_x, raw.position_y
        ));
        out(&format!("  current mode: {}", mode(raw.current)));
        out(&format!("  preferred mode: {}", mode(raw.native)));
        out(&format!("  path scaling: {:?}", raw.scaling));
        out(&format!("  offered modes ({}):", raw.modes.len()));
        for offered in &raw.modes {
            out(&format!("    {}", mode(Some(*offered))));
        }
    }
    out("");
    Some(monitors)
}

fn name_flags(flags: u32) -> String {
    let mut names = Vec::new();
    if flags & FRIENDLY_NAME_FROM_EDID != 0 {
        names.push("friendlyNameFromEdid");
    }
    if flags & FRIENDLY_NAME_FORCED != 0 {
        names.push("friendlyNameForced");
    }
    if flags & EDID_IDS_VALID != 0 {
        names.push("edidIdsValid");
    }
    format!("{flags:#x} [{}]", names.join(", "))
}

fn target_name_lines(name: &Result<TargetName, ReadError>) {
    match name {
        Ok(name) => {
            out(&format!("  target name flags: {}", name_flags(name.flags)));
            out(&format!(
                "  friendly name as read: {:?}",
                name.friendly_name
            ));
            out(&format!(
                "  target outputTechnology: {}; EDID ids as read: {:#06x} {:#06x}; connector instance: {}",
                name.output_technology,
                name.edid_manufacture_id,
                name.edid_product_code_id,
                name.connector_instance
            ));
            out(&format!("  monitor device path: {}", name.device_path));
        }
        Err(error) => out(&format!("  target name: failed: {error}")),
    }
}

fn preferred_line(preferred: &Result<PreferredMode, ReadError>) {
    match preferred {
        Ok(preferred) => out(&format!(
            "  preferred mode: {} x {} at {}; target active size {} x {}",
            preferred.width,
            preferred.height,
            rate(preferred.refresh),
            preferred.active_width,
            preferred.active_height
        )),
        Err(error) => out(&format!("  preferred mode: failed: {error}")),
    }
}

fn path_lines(index: usize, path: &PathRead) {
    out(&format!(
        "path {}: adapter LUID {}:{}; source id {}; target id {}; flags {:#x}; targetAvailable {}",
        index + 1,
        path.adapter_luid.1,
        path.adapter_luid.0,
        path.source_id,
        path.target_id,
        path.path_flags,
        path.target_available
    ));
    out(&format!(
        "  path outputTechnology: {}; path scaling value: {}; path refreshRate: {}",
        path.output_technology,
        path.scaling,
        rate(path.path_refresh)
    ));
    out(&format!(
        "  mode index words: source {:#010x}, target {:#010x}",
        path.source_index_word, path.target_index_word
    ));
    match path.source {
        Some(source) => out(&format!(
            "  source mode: {} x {} at position {}, {}",
            source.width, source.height, source.position_x, source.position_y
        )),
        None => out("  source mode: none"),
    }
    match path.target {
        Some(target) => {
            out(&format!(
                "  target mode: active {} x {}, total {} x {}, vSyncFreq {}, hSyncFreq {}",
                target.active_width,
                target.active_height,
                target.total_width,
                target.total_height,
                rate(target.vsync),
                rate(target.hsync)
            ));
            out(&format!(
                "  target pixel rate: {}; scan line ordering: {}",
                target.pixel_rate, target.scan_line_ordering
            ));
        }
        None => out("  target mode: none"),
    }
    target_name_lines(&path.target_name);
    match &path.gdi_name {
        Ok(name) => out(&format!("  viewGdiDeviceName: {name}")),
        Err(error) => out(&format!("  viewGdiDeviceName: failed: {error}")),
    }
    match &path.adapter_device_path {
        Ok(device_path) => out(&format!("  adapterDevicePath: {device_path}")),
        Err(error) => out(&format!("  adapterDevicePath: failed: {error}")),
    }
    preferred_line(&path.preferred);
}

fn paths_section() {
    out("== Active display paths (QDC_ONLY_ACTIVE_PATHS, as the app reads them)");
    match read_paths() {
        Ok(paths) => {
            let adapters = gdi::display_devices();
            for (index, path) in paths.iter().enumerate() {
                path_lines(index, path);
                if let Ok(name) = &path.gdi_name {
                    let found = adapters.iter().any(|adapter| &adapter.device_name == name);
                    out(&format!(
                        "  equals an EnumDisplayDevicesW adapter DeviceName: {found}"
                    ));
                }
            }
        }
        Err(error) => out(&format!(
            "read failed: {:?}: {}",
            error.code(),
            error.to_ipc().detail
        )),
    }
    out("");
}

fn gdi_section() {
    out("== GDI devices (EnumDisplayDevicesW)");
    for adapter in gdi::display_devices() {
        out(&format!(
            "adapter {}: {:?}; StateFlags {:#x} (attached {}, primary {}, mirroring {})",
            adapter.device_name,
            adapter.device_string,
            adapter.state_flags,
            adapter.is_attached(),
            adapter.is_primary(),
            adapter.is_mirroring()
        ));
        for monitor in &adapter.monitors {
            out(&format!(
                "  monitor {}: DeviceString {:?}; StateFlags {:#x}",
                monitor.device_name, monitor.device_string, monitor.state_flags
            ));
            out(&format!("    interface name: {}", monitor.interface_name));
        }
        if adapter.is_attached() {
            match gdi::current_settings(&adapter.device_name) {
                Some(current) => out(&format!(
                    "  ENUM_CURRENT_SETTINGS: {} x {}, {} bpp, {} Hz, position {}, {}, fixed output {}",
                    current.entry.width,
                    current.entry.height,
                    current.entry.bits_per_pixel,
                    current.entry.frequency,
                    current.position_x,
                    current.position_y,
                    current
                        .fixed_output
                        .map_or_else(|| String::from("not reported"), |value| value.to_string())
                )),
                None => out("  ENUM_CURRENT_SETTINGS: not available"),
            }
            let entries = gdi::mode_entries(&adapter.device_name);
            out(&format!(
                "  GDI mode list from index 0, flags 0: {} entries, {} at 32 bpp progressive",
                entries.len(),
                entries.iter().filter(|entry| entry.usable()).count()
            ));
            for entry in entries.iter().filter(|entry| entry.usable()) {
                out(&format!(
                    "    {} x {} at {} Hz",
                    entry.width, entry.height, entry.frequency
                ));
            }
        }
    }
    out("");
}

fn adapter_section(monitors: &[RawMonitor]) {
    out("== Adapter devices (SetupAPI, from each adapterDevicePath)");
    let paths: BTreeSet<&str> = monitors
        .iter()
        .map(|monitor| monitor.adapter_device_path.as_str())
        .filter(|path| !path.is_empty())
        .collect();
    for path in paths {
        out(&format!("adapterDevicePath: {path}"));
        match device::adapter_device(path) {
            Ok(found) => {
                out(&format!("  device instance ID: {}", found.instance_id));
                out(&format!(
                    "  description: {}",
                    found.description.as_deref().unwrap_or("not reported")
                ));
                out(&format!(
                    "  driver key: {}",
                    found.driver_key.as_deref().unwrap_or("not reported")
                ));
            }
            Err(error) => out(&format!("  failed: {}", error.to_ipc().detail)),
        }
    }
    out("");
}

fn reading_line(label: &str, reading: &HalfReading) -> String {
    format!(
        "{label}: source index {} ({}), target index {} ({}), desktop index {} ({})",
        reading.source_index,
        if reading.source_valid {
            "names a source mode"
        } else {
            "invalid"
        },
        reading.target_index,
        if reading.target_valid {
            "names a target mode"
        } else {
            "invalid"
        },
        reading.desktop_index,
        if reading.desktop_valid {
            "names a desktop image"
        } else {
            "invalid or not specified"
        }
    )
}

fn query_lines(form: &QueryForm) {
    out(&format!("query {} ({:#x}):", form.label, form.flags));
    match &form.result {
        Ok(paths) => {
            for (index, path) in paths.iter().enumerate() {
                out(&format!(
                    "  path {}: flags {:#x}; {} mode entries; words source {:#010x}, target {:#010x}; path refreshRate {}; 32-bit target vSyncFreq {}",
                    index + 1,
                    path.path_flags,
                    path.mode_count,
                    path.source_word,
                    path.target_word,
                    rate(path.path_refresh),
                    rate(path.target_vsync_32)
                ));
                out(&format!(
                    "    {}",
                    reading_line("low half first", &path.low_first)
                ));
                out(&format!(
                    "    {}",
                    reading_line("high half first", &path.high_first)
                ));
            }
        }
        Err(error) => out(&format!("  failed: {error}")),
    }
}

fn query_section() {
    out("== Query forms (mode awareness and virtual refresh awareness)");
    for form in query_forms() {
        query_lines(&form);
    }
    out("");
}

fn registry_section() {
    out("== Display class registry keys (read only)");
    let keys = match display_class_keys() {
        Ok(keys) => keys,
        Err(error) => {
            out(&format!("listing failed: {}", error.to_ipc().detail));
            return;
        }
    };
    for key in &keys {
        out(&format!(
            "key {}: MatchingDeviceId {}; DriverDesc {}; NVIDIA {}{}",
            key.key,
            key.matching_device_id.as_deref().unwrap_or("not set"),
            key.driver_desc.as_deref().unwrap_or("not set"),
            key.is_nvidia(),
            key.open_error
                .map_or_else(String::new, |code| format!("; open failed with {code}"))
        ));
        if !key.is_nvidia() {
            continue;
        }
        match read_class_value(&key.key, "NV_Modes") {
            Ok(Some(value)) => {
                let kind = match value.kind {
                    RawKind::MultiSz => String::from("REG_MULTI_SZ"),
                    RawKind::Sz => String::from("REG_SZ"),
                    RawKind::Other(number) => format!("type {number}"),
                };
                out(&format!(
                    "  NV_Modes: {kind}, {} strings",
                    value.strings.len()
                ));
                for text in &value.strings {
                    out(&format!("    ({} characters) {text}", text.chars().count()));
                }
            }
            Ok(None) => out("  NV_Modes: not set"),
            Err(error) => out(&format!(
                "  NV_Modes: read failed: {}",
                error.to_ipc().detail
            )),
        }
    }
    out("");
}
