//! The GDI route: EnumDisplayDevicesW for the mirroring-driver filter and the adapter names,
//! EnumDisplaySettingsExW for the mode list, and ChangeDisplaySettingsExW for a temporary
//! mode change, never written as the Windows default (REQ-DISP-06, REQ-DISP-14,
//! REQ-DISP-15).

use windows::Win32::Graphics::Gdi::{
    CDS_FULLSCREEN, CDS_TEST, CDS_TYPE, ChangeDisplaySettingsExW, DEVMODE_DISPLAY_FIXED_OUTPUT,
    DEVMODEW, DEVMODEW_0, DEVMODEW_0_1, DISPLAY_DEVICE_ATTACHED_TO_DESKTOP,
    DISPLAY_DEVICE_MIRRORING_DRIVER, DISPLAY_DEVICE_PRIMARY_DEVICE, DISPLAY_DEVICEW, DM_BITSPERPEL,
    DM_DISPLAYFIXEDOUTPUT, DM_DISPLAYFLAGS, DM_DISPLAYFREQUENCY, DM_INTERLACED, DM_PELSHEIGHT,
    DM_PELSWIDTH, DM_POSITION, DMDFO_CENTER, DMDFO_STRETCH, ENUM_CURRENT_SETTINGS,
    ENUM_DISPLAY_SETTINGS_FLAGS, ENUM_DISPLAY_SETTINGS_MODE, EnumDisplayDevicesW,
    EnumDisplaySettingsExW,
};
use windows::Win32::UI::WindowsAndMessaging::EDD_GET_DEVICE_INTERFACE_NAME;
use windows::core::{HSTRING, PCWSTR};

use crate::display::GdiFlag;
use crate::display::win32::ccd::{Stage, wide_to_string};
use crate::error::{AppError, Failure};

/// A stop for the device loops, far above any real count, so a driver that never reports
/// the end cannot keep a read running.
const MAX_DEVICES: u32 = 64;
/// The same stop for the mode list.
const MAX_MODES: u32 = 8192;
/// Windows 8 and later apps cannot set modes below 32 bits per pixel; ResSwap lists only
/// 32 bpp modes (EnumDisplaySettingsEx).
pub const BITS_PER_PIXEL: u32 = 32;

/// One display adapter as EnumDisplayDevicesW lists it, with its monitors.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GdiAdapter {
    /// The GDI device name the mode functions take, for example `\\.\DISPLAY1`.
    pub device_name: String,
    /// The adapter's name, for example the GPU's name.
    pub device_string: String,
    pub state_flags: u32,
    pub monitors: Vec<GdiMonitorDevice>,
}

impl GdiAdapter {
    /// DISPLAY_DEVICE_MIRRORING_DRIVER: a pseudo device for remoting, never listed
    /// (REQ-DISP-07).
    pub fn is_mirroring(&self) -> bool {
        self.state_flags & DISPLAY_DEVICE_MIRRORING_DRIVER.0 != 0
    }

    /// DISPLAY_DEVICE_PRIMARY_DEVICE: "The primary desktop is on the device."
    pub fn is_primary(&self) -> bool {
        self.state_flags & DISPLAY_DEVICE_PRIMARY_DEVICE.0 != 0
    }

    /// DISPLAY_DEVICE_ATTACHED_TO_DESKTOP, read on an adapter record.
    pub fn is_attached(&self) -> bool {
        self.state_flags & DISPLAY_DEVICE_ATTACHED_TO_DESKTOP.0 != 0
    }
}

/// One monitor record under an adapter, read with EDD_GET_DEVICE_INTERFACE_NAME.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GdiMonitorDevice {
    pub device_name: String,
    /// "The monitor name", usually generic.
    pub device_string: String,
    pub state_flags: u32,
    /// The monitor's GUID_DEVINTERFACE_MONITOR interface name.
    pub interface_name: String,
}

/// One entry of the GDI mode list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GdiEntry {
    pub width: u32,
    pub height: u32,
    pub bits_per_pixel: u32,
    /// dmDisplayFrequency: whole hertz.
    pub frequency: u32,
    pub interlaced: bool,
}

impl GdiEntry {
    /// 32 bpp and progressive: the entries ResSwap offers and applies.
    pub fn usable(&self) -> bool {
        self.bits_per_pixel == BITS_PER_PIXEL && !self.interlaced
    }
}

/// ENUM_CURRENT_SETTINGS for one device, for cross-checks and the diagnostics tool.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GdiCurrent {
    pub entry: GdiEntry,
    pub position_x: i32,
    pub position_y: i32,
    /// dmDisplayFixedOutput when DM_DISPLAYFIXEDOUTPUT is set. GDI reads back STRETCH or
    /// CENTER only, so the path scaling always comes from CCD.
    pub fixed_output: Option<u32>,
}

fn blank_device() -> DISPLAY_DEVICEW {
    DISPLAY_DEVICEW {
        cb: size_of::<DISPLAY_DEVICEW>() as u32,
        ..Default::default()
    }
}

/// Every display adapter and its monitors. Changes nothing.
pub fn display_devices() -> Vec<GdiAdapter> {
    let mut adapters = Vec::new();
    for index in 0..MAX_DEVICES {
        let mut adapter = blank_device();
        // SAFETY: a null device name asks for adapters; `adapter` is a DISPLAY_DEVICEW on
        // this frame with cb set to its size, so the call writes only inside it.
        let found = unsafe { EnumDisplayDevicesW(PCWSTR::null(), index, &mut adapter, 0) };
        if !found.as_bool() {
            break;
        }
        let device_name = wide_to_string(&adapter.DeviceName);
        let monitors = monitor_devices(&device_name);
        adapters.push(GdiAdapter {
            device_name,
            device_string: wide_to_string(&adapter.DeviceString),
            state_flags: adapter.StateFlags.0,
            monitors,
        });
    }
    adapters
}

fn monitor_devices(adapter: &str) -> Vec<GdiMonitorDevice> {
    let name = HSTRING::from(adapter);
    let mut monitors = Vec::new();
    for index in 0..MAX_DEVICES {
        let mut monitor = blank_device();
        // SAFETY: `name` is the adapter's NUL-terminated device name and lives until the call
        // returns; `monitor` is a DISPLAY_DEVICEW on this frame with cb set to its size.
        let found = unsafe {
            EnumDisplayDevicesW(&name, index, &mut monitor, EDD_GET_DEVICE_INTERFACE_NAME)
        };
        if !found.as_bool() {
            break;
        }
        monitors.push(GdiMonitorDevice {
            device_name: wide_to_string(&monitor.DeviceName),
            device_string: wide_to_string(&monitor.DeviceString),
            state_flags: monitor.StateFlags.0,
            interface_name: wide_to_string(&monitor.DeviceID),
        });
    }
    monitors
}

fn blank_devmode() -> DEVMODEW {
    DEVMODEW {
        dmSize: size_of::<DEVMODEW>() as u16,
        dmDriverExtra: 0,
        ..Default::default()
    }
}

/// One mode as a GdiEntry, when the call filled every field ResSwap reads.
fn entry_of(mode: &DEVMODEW) -> Option<GdiEntry> {
    let needed = DM_PELSWIDTH.0 | DM_PELSHEIGHT.0 | DM_BITSPERPEL.0 | DM_DISPLAYFREQUENCY.0;
    if mode.dmFields.0 & needed != needed {
        return None;
    }
    let interlaced = if mode.dmFields.contains(DM_DISPLAYFLAGS) {
        // SAFETY: DM_DISPLAYFLAGS is set, so the call filled dmDisplayFlags; both members of
        // the union are u32 views of the same bits.
        let flags = unsafe { mode.Anonymous2.dmDisplayFlags };
        flags & DM_INTERLACED.0 != 0
    } else {
        false
    };
    Some(GdiEntry {
        width: mode.dmPelsWidth,
        height: mode.dmPelsHeight,
        bits_per_pixel: mode.dmBitsPerPel,
        frequency: mode.dmDisplayFrequency,
        interlaced,
    })
}

/// Every mode the device lists, from index 0 every time so the list is built afresh and a
/// newly added mode appears (REQ-DISP-06); flags 0, never EDS_RAWMODE. Changes nothing.
pub fn mode_entries(device: &str) -> Vec<GdiEntry> {
    let name = HSTRING::from(device);
    let mut entries = Vec::new();
    for index in 0..MAX_MODES {
        let mut mode = blank_devmode();
        // SAFETY: `name` is a NUL-terminated device name that lives until the call returns;
        // `mode` is a DEVMODEW on this frame with dmSize set to its size and dmDriverExtra 0,
        // so the call writes no driver data past it.
        let found = unsafe {
            EnumDisplaySettingsExW(
                &name,
                ENUM_DISPLAY_SETTINGS_MODE(index),
                &mut mode,
                ENUM_DISPLAY_SETTINGS_FLAGS(0),
            )
        };
        if !found.as_bool() {
            break;
        }
        if let Some(entry) = entry_of(&mode) {
            entries.push(entry);
        }
    }
    entries
}

/// ENUM_CURRENT_SETTINGS for one device. Changes nothing.
pub fn current_settings(device: &str) -> Option<GdiCurrent> {
    let name = HSTRING::from(device);
    let mut mode = blank_devmode();
    // SAFETY: as in mode_entries; ENUM_CURRENT_SETTINGS asks for the current mode.
    let found = unsafe {
        EnumDisplaySettingsExW(
            &name,
            ENUM_CURRENT_SETTINGS,
            &mut mode,
            ENUM_DISPLAY_SETTINGS_FLAGS(0),
        )
    };
    if !found.as_bool() {
        return None;
    }
    let entry = entry_of(&mode)?;
    // SAFETY: for a display device the union holds the display view (DEVMODEW), and every
    // bit pattern is valid for its plain integer fields; each is used only when its DM_ bit
    // says the call filled it.
    let display = unsafe { mode.Anonymous1.Anonymous2 };
    let (position_x, position_y) = if mode.dmFields.contains(DM_POSITION) {
        (display.dmPosition.x, display.dmPosition.y)
    } else {
        (0, 0)
    };
    let fixed_output = mode
        .dmFields
        .contains(DM_DISPLAYFIXEDOUTPUT)
        .then_some(display.dmDisplayFixedOutput.0);
    Some(GdiCurrent {
        entry,
        position_x,
        position_y,
        fixed_output,
    })
}

/// What a GDI call says about scaling: nothing (the bit clear, read as DMDFO_DEFAULT), or
/// DM_DISPLAYFIXEDOUTPUT with DMDFO_STRETCH or DMDFO_CENTER.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FixedOutput {
    Default,
    Stretch,
    Center,
}

/// The DEVMODEW for a test or an apply: only the size and the frequency of a listed entry,
/// plus DM_DISPLAYFIXEDOUTPUT with DMDFO_STRETCH or DMDFO_CENTER when `output` asks for one.
/// Without it the bit stays clear and the member zero, as DEVMODEW requires. No position, so
/// the desktop layout is not touched.
pub(crate) fn build_devmode(entry: &GdiEntry, output: FixedOutput) -> DEVMODEW {
    let mut fields = DM_PELSWIDTH | DM_PELSHEIGHT | DM_DISPLAYFREQUENCY;
    let mut fixed_output = DEVMODE_DISPLAY_FIXED_OUTPUT(0);
    match output {
        FixedOutput::Default => {}
        FixedOutput::Stretch => {
            fields |= DM_DISPLAYFIXEDOUTPUT;
            fixed_output = DMDFO_STRETCH;
        }
        FixedOutput::Center => {
            fields |= DM_DISPLAYFIXEDOUTPUT;
            fixed_output = DMDFO_CENTER;
        }
    }
    DEVMODEW {
        dmFields: fields,
        dmPelsWidth: entry.width,
        dmPelsHeight: entry.height,
        dmDisplayFrequency: entry.frequency,
        Anonymous1: DEVMODEW_0 {
            Anonymous2: DEVMODEW_0_1 {
                dmDisplayFixedOutput: fixed_output,
                ..Default::default()
            },
        },
        ..blank_devmode()
    }
}

/// The GDI test flag.
pub(crate) const TEST_FLAGS: CDS_TYPE = CDS_TEST;

/// The GDI apply flag for the route's temporary flag. Never CDS_UPDATEREGISTRY, CDS_GLOBAL
/// or CDS_NORESET (REQ-DISP-14).
pub(crate) const fn apply_flags(flag: GdiFlag) -> CDS_TYPE {
    match flag {
        GdiFlag::Fullscreen => CDS_FULLSCREEN,
        GdiFlag::Zero => CDS_TYPE(0),
    }
}

fn flags_text(flags: CDS_TYPE) -> &'static str {
    match flags.0 {
        0 => "0",
        value if value == CDS_TEST.0 => "CDS_TEST",
        value if value == CDS_FULLSCREEN.0 => "CDS_FULLSCREEN",
        _ => "other flags",
    }
}

/// ChangeDisplaySettingsExW on one GDI device; the raw DISP_CHANGE value.
fn change(device: &str, mode: &DEVMODEW, flags: CDS_TYPE) -> i32 {
    let name = HSTRING::from(device);
    // SAFETY: `name` is a NUL-terminated device name and `mode` a fully initialised DEVMODEW
    // with dmSize set and dmDriverExtra 0; both outlive the call. hwnd is None ("Reserved;
    // must be NULL") and lParam is None, as CDS_VIDEOPARAMETERS is not set.
    let result = unsafe {
        ChangeDisplaySettingsExW(&name, Some(std::ptr::from_ref(mode)), None, flags, None)
    };
    result.0
}

/// CDS_TEST: "determine which graphics modes are actually valid, without causing the system
/// to change to them" (ChangeDisplaySettingsEx). Changes nothing.
pub(crate) fn test(device: &str, mode: &DEVMODEW) -> Result<(), AppError> {
    let code = change(device, mode, TEST_FLAGS);
    log_call(device, mode, TEST_FLAGS, code);
    map_disp_change(code, Stage::Test, device, TEST_FLAGS)
}

/// The temporary apply with the route's flag. Changes the PC; reached only from
/// apply_config, which holds the Ticket.
pub(crate) fn apply(device: &str, mode: &DEVMODEW, flag: GdiFlag) -> Result<(), AppError> {
    let flags = apply_flags(flag);
    let code = change(device, mode, flags);
    log_call(device, mode, flags, code);
    map_disp_change(code, Stage::Apply, device, flags)
}

fn log_call(device: &str, mode: &DEVMODEW, flags: CDS_TYPE, code: i32) {
    let output = if mode.dmFields.contains(DM_DISPLAYFIXEDOUTPUT) {
        // SAFETY: build_devmode wrote the display view of the union, and DEVMODEW_0_1 is
        // plain data, so this copy reads only bytes it set.
        let display = unsafe { mode.Anonymous1.Anonymous2 };
        fixed_output_name(display.dmDisplayFixedOutput)
    } else {
        "DMDFO_DEFAULT"
    };
    log::info!(
        "ChangeDisplaySettingsExW({}) on {device} for {} x {} at {} Hz, {} returned {code} ({})",
        flags_text(flags),
        mode.dmPelsWidth,
        mode.dmPelsHeight,
        mode.dmDisplayFrequency,
        output,
        disp_change_name(code)
    );
}

/// The DMDFO name of a dmDisplayFixedOutput value, for the log.
fn fixed_output_name(value: DEVMODE_DISPLAY_FIXED_OUTPUT) -> &'static str {
    match value {
        DMDFO_STRETCH => "DMDFO_STRETCH",
        DMDFO_CENTER => "DMDFO_CENTER",
        _ => "DMDFO_DEFAULT",
    }
}

/// The DISP_CHANGE name of a return value, for the log and the detail.
pub(crate) fn disp_change_name(code: i32) -> &'static str {
    match code {
        0 => "DISP_CHANGE_SUCCESSFUL",
        1 => "DISP_CHANGE_RESTART",
        -1 => "DISP_CHANGE_FAILED",
        -2 => "DISP_CHANGE_BADMODE",
        -3 => "DISP_CHANGE_NOTUPDATED",
        -4 => "DISP_CHANGE_BADFLAGS",
        -5 => "DISP_CHANGE_BADPARAM",
        -6 => "DISP_CHANGE_BADDUALVIEW",
        _ => "an undocumented value",
    }
}

/// A DISP_CHANGE value as ResSwap reports it (ChangeDisplaySettingsEx return values):
/// success only for DISP_CHANGE_SUCCESSFUL, and even then the pipeline decides by its
/// read-back; BADFLAGS and BADPARAM are ResSwap bugs (INTERNAL); every other value is
/// MODE_TEST_FAILED at CDS_TEST and MODE_REJECTED at the apply, the value in the detail.
pub(crate) fn map_disp_change(
    code: i32,
    stage: Stage,
    device: &str,
    flags: CDS_TYPE,
) -> Result<(), AppError> {
    if code == 0 {
        return Ok(());
    }
    let detail = format!(
        "ChangeDisplaySettingsExW({}) on {device} returned {code} ({})",
        flags_text(flags),
        disp_change_name(code)
    );
    if matches!(code, -4 | -5) {
        return Err(AppError::internal(detail));
    }
    let failure = Box::new(Failure::new(detail));
    Err(match stage {
        Stage::Test => AppError::ModeTestFailed(failure),
        Stage::Apply => AppError::ModeRejected(failure),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::ErrorCode;

    /// CDS_UPDATEREGISTRY (0x1), CDS_GLOBAL (0x8), CDS_NORESET (0x10000000), CDS_RESET_EX
    /// (0x20000000) and the unsafe-mode flags (0x100, 0x200), as the 0.62.2 crate and
    /// WinUser.h define them. No GDI call ResSwap makes ever sets one.
    const NEVER: u32 = 0x1 | 0x8 | 0x1000_0000 | 0x2000_0000 | 0x100 | 0x200;

    fn entry(width: u32, height: u32, frequency: u32) -> GdiEntry {
        GdiEntry {
            width,
            height,
            bits_per_pixel: 32,
            frequency,
            interlaced: false,
        }
    }

    #[test]
    fn req_disp_14_gdi_flags_are_never_saved() {
        assert_eq!(TEST_FLAGS.0 & NEVER, 0);
        assert_eq!(TEST_FLAGS.0, 0x2);
        for flag in [GdiFlag::Fullscreen, GdiFlag::Zero] {
            assert_eq!(apply_flags(flag).0 & NEVER, 0, "{flag:?}");
        }
        assert_eq!(apply_flags(GdiFlag::Fullscreen).0, 0x4);
        assert_eq!(apply_flags(GdiFlag::Zero).0, 0);
    }

    #[test]
    fn req_disp_17_built_mode_sets_size_and_rate_only() {
        let mode = build_devmode(&entry(1440, 1080, 240), FixedOutput::Default);
        assert_eq!(mode.dmSize as usize, size_of::<DEVMODEW>());
        assert_eq!(mode.dmDriverExtra, 0);
        assert_eq!(
            mode.dmFields.0,
            DM_PELSWIDTH.0 | DM_PELSHEIGHT.0 | DM_DISPLAYFREQUENCY.0
        );
        assert_eq!(
            (mode.dmPelsWidth, mode.dmPelsHeight, mode.dmDisplayFrequency),
            (1440, 1080, 240)
        );
        // SAFETY: build_devmode wrote the display view of the union.
        let display = unsafe { mode.Anonymous1.Anonymous2 };
        // Without DM_DISPLAYFIXEDOUTPUT the member must be zero (DEVMODEW).
        assert_eq!(display.dmDisplayFixedOutput.0, 0);
        assert!(!mode.dmFields.contains(DM_POSITION));
    }

    #[test]
    fn req_nv_25_built_mode_asks_for_stretch_with_dmdfo_stretch() {
        let mode = build_devmode(&entry(1440, 1080, 240), FixedOutput::Stretch);
        assert!(mode.dmFields.contains(DM_DISPLAYFIXEDOUTPUT));
        // SAFETY: build_devmode wrote the display view of the union.
        let display = unsafe { mode.Anonymous1.Anonymous2 };
        // DMDFO_STRETCH is 1 in wingdi.h, not 2.
        assert_eq!(display.dmDisplayFixedOutput.0, 1);
        assert_eq!(display.dmPosition.x, 0);
        assert_eq!(display.dmPosition.y, 0);
    }

    #[test]
    fn req_nv_25_put_back_writes_dmdfo_center_for_a_centred_state() {
        let mode = build_devmode(&entry(1440, 1080, 240), FixedOutput::Center);
        assert!(mode.dmFields.contains(DM_DISPLAYFIXEDOUTPUT));
        // SAFETY: build_devmode wrote the display view of the union.
        let display = unsafe { mode.Anonymous1.Anonymous2 };
        // DMDFO_CENTER is 2 in wingdi.h.
        assert_eq!(display.dmDisplayFixedOutput.0, 2);
        assert_eq!(
            fixed_output_name(display.dmDisplayFixedOutput),
            "DMDFO_CENTER"
        );
        assert_eq!(fixed_output_name(DMDFO_STRETCH), "DMDFO_STRETCH");
        assert_eq!(
            fixed_output_name(DEVMODE_DISPLAY_FIXED_OUTPUT(0)),
            "DMDFO_DEFAULT"
        );
    }

    #[test]
    fn req_disp_12_disp_change_codes_map_to_their_errors() {
        assert!(map_disp_change(0, Stage::Test, r"\\.\DISPLAY1", TEST_FLAGS).is_ok());
        for code in [1, -1, -2, -3, -6, 7] {
            let test = map_disp_change(code, Stage::Test, r"\\.\DISPLAY1", TEST_FLAGS).unwrap_err();
            assert_eq!(test.code(), ErrorCode::ModeTestFailed, "{code}");
            assert!(test.to_ipc().detail.contains(&code.to_string()));
            let apply = map_disp_change(
                code,
                Stage::Apply,
                r"\\.\DISPLAY1",
                apply_flags(GdiFlag::Fullscreen),
            )
            .unwrap_err();
            assert_eq!(apply.code(), ErrorCode::ModeRejected, "{code}");
        }
        for code in [-4, -5] {
            let bug = map_disp_change(code, Stage::Test, r"\\.\DISPLAY1", TEST_FLAGS).unwrap_err();
            assert_eq!(bug.code(), ErrorCode::Internal, "{code}");
        }
        let detail = map_disp_change(-2, Stage::Test, r"\\.\DISPLAY1", TEST_FLAGS)
            .unwrap_err()
            .to_ipc()
            .detail;
        assert!(detail.contains("DISP_CHANGE_BADMODE"), "{detail}");
        assert!(detail.contains("CDS_TEST"), "{detail}");
    }

    #[test]
    fn req_disp_06_only_32_bpp_progressive_entries_are_usable() {
        assert!(entry(1920, 1080, 240).usable());
        let low = GdiEntry {
            bits_per_pixel: 16,
            ..entry(1920, 1080, 60)
        };
        assert!(!low.usable());
        let interlaced = GdiEntry {
            interlaced: true,
            ..entry(1920, 1080, 60)
        };
        assert!(!interlaced.usable());
    }

    // Reads the GDI lists of the PC this runs on; changes nothing.
    #[test]
    #[ignore = "reads the real PC; run by name"]
    fn req_disp_06_list_gdi_modes_on_this_pc() {
        let adapters = display_devices();
        let attached = adapters
            .iter()
            .find(|adapter| adapter.is_attached())
            .unwrap();
        assert!(!mode_entries(&attached.device_name).is_empty());
    }
}
