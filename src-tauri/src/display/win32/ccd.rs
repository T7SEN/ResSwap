//! The CCD route: QueryDisplayConfig, SetDisplayConfig and DisplayConfigGetDeviceInfo for
//! identity, the preferred mode, exact refresh rationals, scaling and layout (REQ-DISP-04,
//! REQ-DISP-17). Paths and modes are always used as one snapshot from one QueryDisplayConfig
//! call. ResSwap queries without QDC_VIRTUAL_MODE_AWARE and sets without
//! SDC_VIRTUAL_MODE_AWARE, so every path holds 32-bit mode indexes.

use windows::Win32::Devices::Display::{
    DISPLAYCONFIG_ADAPTER_NAME, DISPLAYCONFIG_DEVICE_INFO_GET_ADAPTER_NAME,
    DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME, DISPLAYCONFIG_DEVICE_INFO_GET_TARGET_NAME,
    DISPLAYCONFIG_DEVICE_INFO_GET_TARGET_PREFERRED_MODE, DISPLAYCONFIG_DEVICE_INFO_HEADER,
    DISPLAYCONFIG_DEVICE_INFO_TYPE, DISPLAYCONFIG_MODE_INFO, DISPLAYCONFIG_MODE_INFO_0,
    DISPLAYCONFIG_MODE_INFO_TYPE_SOURCE, DISPLAYCONFIG_MODE_INFO_TYPE_TARGET,
    DISPLAYCONFIG_PATH_INFO, DISPLAYCONFIG_RATIONAL, DISPLAYCONFIG_SCALING,
    DISPLAYCONFIG_SCALING_ASPECTRATIOCENTEREDMAX, DISPLAYCONFIG_SCALING_CENTERED,
    DISPLAYCONFIG_SCALING_CUSTOM, DISPLAYCONFIG_SCALING_IDENTITY, DISPLAYCONFIG_SCALING_PREFERRED,
    DISPLAYCONFIG_SCALING_STRETCHED, DISPLAYCONFIG_SOURCE_DEVICE_NAME, DISPLAYCONFIG_SOURCE_MODE,
    DISPLAYCONFIG_TARGET_DEVICE_NAME, DISPLAYCONFIG_TARGET_MODE,
    DISPLAYCONFIG_TARGET_PREFERRED_MODE, DisplayConfigGetDeviceInfo, GetDisplayConfigBufferSizes,
    QUERY_DISPLAY_CONFIG_FLAGS, QueryDisplayConfig, SDC_APPLY, SDC_USE_DATABASE_CURRENT,
    SDC_USE_SUPPLIED_DISPLAY_CONFIG, SDC_VALIDATE, SET_DISPLAY_CONFIG_FLAGS, SetDisplayConfig,
};
use windows::Win32::Foundation::{ERROR_INSUFFICIENT_BUFFER, ERROR_INVALID_PARAMETER, LUID};

use crate::display::Rational;
use crate::display::types::{Scaling, ScalingChoice};
use crate::error::{AppError, Failure};

/// How often a read starts again when the sizes went stale between
/// GetDisplayConfigBufferSizes and QueryDisplayConfig (ERROR_INSUFFICIENT_BUFFER).
const QUERY_ATTEMPTS: usize = 5;

/// DISPLAYCONFIG_TARGET_DEVICE_NAME_FLAGS: the friendly name was built from the EDID.
pub const FRIENDLY_NAME_FROM_EDID: u32 = 0x1;
/// DISPLAYCONFIG_TARGET_DEVICE_NAME_FLAGS: the target is forced with no monitor attached.
pub const FRIENDLY_NAME_FORCED: u32 = 0x2;
/// DISPLAYCONFIG_TARGET_DEVICE_NAME_FLAGS: edidManufactureId and edidProductCodeId are set.
pub const EDID_IDS_VALID: u32 = 0x4;

/// One path and mode snapshot from one QueryDisplayConfig call.
#[derive(Clone)]
pub(crate) struct CcdConfig {
    pub(crate) paths: Vec<DISPLAYCONFIG_PATH_INFO>,
    pub(crate) modes: Vec<DISPLAYCONFIG_MODE_INFO>,
}

/// Reads the paths and modes with `flags`, asking for the sizes again whenever they went
/// stale, and keeps only the entries QueryDisplayConfig wrote. Changes nothing.
pub(crate) fn query(flags: QUERY_DISPLAY_CONFIG_FLAGS) -> Result<CcdConfig, AppError> {
    for _ in 0..QUERY_ATTEMPTS {
        let mut path_count = 0u32;
        let mut mode_count = 0u32;
        // SAFETY: both out pointers point at u32 locals on this frame that outlive the call.
        let status =
            unsafe { GetDisplayConfigBufferSizes(flags, &mut path_count, &mut mode_count) };
        if status.is_err() {
            return Err(read_failed("GetDisplayConfigBufferSizes", status.0));
        }
        let mut paths = vec![DISPLAYCONFIG_PATH_INFO::default(); path_count as usize];
        let mut modes = vec![DISPLAYCONFIG_MODE_INFO::default(); mode_count as usize];
        // SAFETY: `paths` holds `path_count` and `modes` holds `mode_count` entries, and the
        // counts say so, so the call writes inside both vectors; the counts are valid in and
        // out pointers on this frame; no topology is asked for (None), which the docs require
        // without QDC_DATABASE_CURRENT.
        let status = unsafe {
            QueryDisplayConfig(
                flags,
                &mut path_count,
                paths.as_mut_ptr(),
                &mut mode_count,
                modes.as_mut_ptr(),
                None,
            )
        };
        if status == ERROR_INSUFFICIENT_BUFFER {
            continue;
        }
        if status.is_err() {
            return Err(read_failed("QueryDisplayConfig", status.0));
        }
        paths.truncate(path_count as usize);
        modes.truncate(mode_count as usize);
        return Ok(CcdConfig { paths, modes });
    }
    Err(AppError::internal(format!(
        "QueryDisplayConfig returned ERROR_INSUFFICIENT_BUFFER {QUERY_ATTEMPTS} times in a row"
    )))
}

/// The 32-bit source mode index of a path read without virtual-mode awareness.
pub(crate) fn source_index_word(path: &DISPLAYCONFIG_PATH_INFO) -> u32 {
    // SAFETY: both members of the union are 32-bit views of the same bits, and every bit
    // pattern is a valid u32, so reading modeInfoIdx reads initialised plain data.
    unsafe { path.sourceInfo.Anonymous.modeInfoIdx }
}

/// The 32-bit target mode index of a path read without virtual-mode awareness.
pub(crate) fn target_index_word(path: &DISPLAYCONFIG_PATH_INFO) -> u32 {
    // SAFETY: both members of the union are 32-bit views of the same bits, and every bit
    // pattern is a valid u32, so reading modeInfoIdx reads initialised plain data.
    unsafe { path.targetInfo.Anonymous.modeInfoIdx }
}

/// The index of the source mode of `path` in `modes`, when it names a source entry.
pub(crate) fn source_mode_index(
    modes: &[DISPLAYCONFIG_MODE_INFO],
    path: &DISPLAYCONFIG_PATH_INFO,
) -> Option<usize> {
    let index = source_index_word(path) as usize;
    (modes.get(index)?.infoType == DISPLAYCONFIG_MODE_INFO_TYPE_SOURCE).then_some(index)
}

/// The index of the target mode of `path` in `modes`, when it names a target entry.
pub(crate) fn target_mode_index(
    modes: &[DISPLAYCONFIG_MODE_INFO],
    path: &DISPLAYCONFIG_PATH_INFO,
) -> Option<usize> {
    let index = target_index_word(path) as usize;
    (modes.get(index)?.infoType == DISPLAYCONFIG_MODE_INFO_TYPE_TARGET).then_some(index)
}

/// The source mode of `path`: the desktop size and position.
pub(crate) fn source_mode(
    modes: &[DISPLAYCONFIG_MODE_INFO],
    path: &DISPLAYCONFIG_PATH_INFO,
) -> Option<DISPLAYCONFIG_SOURCE_MODE> {
    let entry = modes.get(source_mode_index(modes, path)?)?;
    // SAFETY: infoType is DISPLAYCONFIG_MODE_INFO_TYPE_SOURCE (checked by
    // source_mode_index), which says the union holds sourceMode; the entry is plain data
    // QueryDisplayConfig wrote.
    Some(unsafe { entry.Anonymous.sourceMode })
}

/// The target mode of `path`: the timing the monitor receives, with the exact vSyncFreq.
pub(crate) fn target_mode(
    modes: &[DISPLAYCONFIG_MODE_INFO],
    path: &DISPLAYCONFIG_PATH_INFO,
) -> Option<DISPLAYCONFIG_TARGET_MODE> {
    let entry = modes.get(target_mode_index(modes, path)?)?;
    // SAFETY: infoType is DISPLAYCONFIG_MODE_INFO_TYPE_TARGET (checked by
    // target_mode_index), which says the union holds targetMode; the entry is plain data
    // QueryDisplayConfig wrote.
    Some(unsafe { entry.Anonymous.targetMode })
}

/// Replaces the source mode at `index` with `source`. The caller has checked that the entry
/// is a source entry.
pub(crate) fn set_source_mode(
    modes: &mut [DISPLAYCONFIG_MODE_INFO],
    index: usize,
    source: DISPLAYCONFIG_SOURCE_MODE,
) {
    if let Some(entry) = modes.get_mut(index) {
        entry.Anonymous = DISPLAYCONFIG_MODE_INFO_0 { sourceMode: source };
    }
}

/// What DISPLAYCONFIG_DEVICE_INFO_GET_TARGET_NAME returns, as plain data.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TargetName {
    /// DISPLAYCONFIG_TARGET_DEVICE_NAME_FLAGS.value.
    pub flags: u32,
    pub output_technology: i32,
    pub edid_manufacture_id: u16,
    pub edid_product_code_id: u16,
    pub connector_instance: u32,
    /// monitorFriendlyDeviceName as read, even when the EDID flag is clear.
    pub friendly_name: String,
    pub device_path: String,
}

impl TargetName {
    /// The EDID friendly name, only when friendlyNameFromEdid is set and the name is not
    /// empty (REQ-DISP-03).
    pub fn edid_name(&self) -> Option<String> {
        (self.flags & FRIENDLY_NAME_FROM_EDID != 0 && !self.friendly_name.is_empty())
            .then(|| self.friendly_name.clone())
    }

    /// The two EDID ids, only when edidIdsValid is set.
    pub fn edid_ids(&self) -> Option<(u16, u16)> {
        (self.flags & EDID_IDS_VALID != 0)
            .then_some((self.edid_manufacture_id, self.edid_product_code_id))
    }
}

/// What DISPLAYCONFIG_DEVICE_INFO_GET_TARGET_PREFERRED_MODE returns, as plain data.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PreferredMode {
    pub width: u32,
    pub height: u32,
    /// The target mode's vSyncFreq; `None` when Windows reports 0 as the denominator.
    pub refresh: Option<Rational>,
    pub active_width: u32,
    pub active_height: u32,
}

fn header(
    kind: DISPLAYCONFIG_DEVICE_INFO_TYPE,
    size: usize,
    adapter: LUID,
    id: u32,
) -> DISPLAYCONFIG_DEVICE_INFO_HEADER {
    DISPLAYCONFIG_DEVICE_INFO_HEADER {
        r#type: kind,
        size: size as u32,
        adapterId: adapter,
        id,
    }
}

/// DISPLAYCONFIG_DEVICE_INFO_GET_TARGET_NAME for one target. Changes nothing.
pub(crate) fn target_name(adapter: LUID, target: u32) -> Result<TargetName, AppError> {
    let mut request = DISPLAYCONFIG_TARGET_DEVICE_NAME {
        header: header(
            DISPLAYCONFIG_DEVICE_INFO_GET_TARGET_NAME,
            size_of::<DISPLAYCONFIG_TARGET_DEVICE_NAME>(),
            adapter,
            target,
        ),
        ..Default::default()
    };
    // SAFETY: `request` is a DISPLAYCONFIG_TARGET_DEVICE_NAME on this frame whose first field
    // is the header, and header.size is the size of the whole struct, so the call writes only
    // inside it; the pointer is taken from the whole struct, so it may reach every field.
    let status = unsafe {
        DisplayConfigGetDeviceInfo(
            std::ptr::addr_of_mut!(request).cast::<DISPLAYCONFIG_DEVICE_INFO_HEADER>(),
        )
    };
    if status != 0 {
        return Err(read_failed_i32(
            "DisplayConfigGetDeviceInfo(GET_TARGET_NAME)",
            status,
        ));
    }
    // SAFETY: both members of the flags union are 32-bit views of the same bits, and every
    // bit pattern is a valid u32.
    let flags = unsafe { request.flags.Anonymous.value };
    Ok(TargetName {
        flags,
        output_technology: request.outputTechnology.0,
        edid_manufacture_id: request.edidManufactureId,
        edid_product_code_id: request.edidProductCodeId,
        connector_instance: request.connectorInstance,
        friendly_name: wide_to_string(&request.monitorFriendlyDeviceName),
        device_path: wide_to_string(&request.monitorDevicePath),
    })
}

/// DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME: the GDI device name of one source, for the
/// GDI calls and the log only. Changes nothing.
pub(crate) fn source_name(adapter: LUID, source: u32) -> Result<String, AppError> {
    let mut request = DISPLAYCONFIG_SOURCE_DEVICE_NAME {
        header: header(
            DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME,
            size_of::<DISPLAYCONFIG_SOURCE_DEVICE_NAME>(),
            adapter,
            source,
        ),
        ..Default::default()
    };
    // SAFETY: `request` is a DISPLAYCONFIG_SOURCE_DEVICE_NAME on this frame whose first field
    // is the header, and header.size is the size of the whole struct, so the call writes only
    // inside it; the pointer is taken from the whole struct.
    let status = unsafe {
        DisplayConfigGetDeviceInfo(
            std::ptr::addr_of_mut!(request).cast::<DISPLAYCONFIG_DEVICE_INFO_HEADER>(),
        )
    };
    if status != 0 {
        return Err(read_failed_i32(
            "DisplayConfigGetDeviceInfo(GET_SOURCE_NAME)",
            status,
        ));
    }
    Ok(wide_to_string(&request.viewGdiDeviceName))
}

/// DISPLAYCONFIG_DEVICE_INFO_GET_ADAPTER_NAME: the adapter's device interface path, which
/// lasts across restarts where the LUID does not. Changes nothing.
pub(crate) fn adapter_name(adapter: LUID) -> Result<String, AppError> {
    let mut request = DISPLAYCONFIG_ADAPTER_NAME {
        header: header(
            DISPLAYCONFIG_DEVICE_INFO_GET_ADAPTER_NAME,
            size_of::<DISPLAYCONFIG_ADAPTER_NAME>(),
            adapter,
            0,
        ),
        ..Default::default()
    };
    // SAFETY: `request` is a DISPLAYCONFIG_ADAPTER_NAME on this frame whose first field is
    // the header, and header.size is the size of the whole struct, so the call writes only
    // inside it; the pointer is taken from the whole struct.
    let status = unsafe {
        DisplayConfigGetDeviceInfo(
            std::ptr::addr_of_mut!(request).cast::<DISPLAYCONFIG_DEVICE_INFO_HEADER>(),
        )
    };
    if status != 0 {
        return Err(read_failed_i32(
            "DisplayConfigGetDeviceInfo(GET_ADAPTER_NAME)",
            status,
        ));
    }
    Ok(wide_to_string(&request.adapterDevicePath))
}

/// DISPLAYCONFIG_DEVICE_INFO_GET_TARGET_PREFERRED_MODE: "the best mode for the monitor",
/// the only documented native mode (REQ-DISP-04). Changes nothing.
pub(crate) fn preferred_mode(adapter: LUID, target: u32) -> Result<PreferredMode, AppError> {
    let mut request = DISPLAYCONFIG_TARGET_PREFERRED_MODE {
        header: header(
            DISPLAYCONFIG_DEVICE_INFO_GET_TARGET_PREFERRED_MODE,
            size_of::<DISPLAYCONFIG_TARGET_PREFERRED_MODE>(),
            adapter,
            target,
        ),
        ..Default::default()
    };
    // SAFETY: `request` is a DISPLAYCONFIG_TARGET_PREFERRED_MODE on this frame whose first
    // field is the header, and header.size is the size of the whole struct, so the call
    // writes only inside it; the pointer is taken from the whole struct.
    let status = unsafe {
        DisplayConfigGetDeviceInfo(
            std::ptr::addr_of_mut!(request).cast::<DISPLAYCONFIG_DEVICE_INFO_HEADER>(),
        )
    };
    if status != 0 {
        return Err(read_failed_i32(
            "DisplayConfigGetDeviceInfo(GET_TARGET_PREFERRED_MODE)",
            status,
        ));
    }
    let signal = request.targetMode.targetVideoSignalInfo;
    Ok(PreferredMode {
        width: request.width,
        height: request.height,
        refresh: rational(signal.vSyncFreq),
        active_width: signal.activeSize.cx,
        active_height: signal.activeSize.cy,
    })
}

/// SetDisplayConfig with the supplied arrays and `flags`; the raw return value.
fn set_supplied(config: &CcdConfig, flags: SET_DISPLAY_CONFIG_FLAGS) -> i32 {
    // SAFETY: both slices are owned by `config` and outlive the call; the crate passes each
    // slice's length as its count, and the mode indexes in the paths come from the same
    // QueryDisplayConfig call as the modes.
    unsafe { SetDisplayConfig(Some(&config.paths), Some(&config.modes), flags) }
}

/// SDC_VALIDATE with SDC_USE_SUPPLIED_DISPLAY_CONFIG. Changes nothing.
pub(crate) fn validate(config: &CcdConfig) -> Result<(), AppError> {
    let code = set_supplied(config, VALIDATE_FLAGS);
    log::info!("SetDisplayConfig(SDC_VALIDATE | SDC_USE_SUPPLIED_DISPLAY_CONFIG) returned {code}");
    map_set_config(
        code,
        Stage::Test,
        "SDC_VALIDATE | SDC_USE_SUPPLIED_DISPLAY_CONFIG",
    )
}

/// SDC_APPLY with SDC_USE_SUPPLIED_DISPLAY_CONFIG: a temporary configuration that is not
/// saved. Changes the PC; reached only from apply_config, which holds the Ticket.
pub(crate) fn apply(config: &CcdConfig) -> Result<(), AppError> {
    let code = set_supplied(config, APPLY_FLAGS);
    log::info!("SetDisplayConfig(SDC_APPLY | SDC_USE_SUPPLIED_DISPLAY_CONFIG) returned {code}");
    map_set_config(
        code,
        Stage::Apply,
        "SDC_APPLY | SDC_USE_SUPPLIED_DISPLAY_CONFIG",
    )
}

/// SDC_APPLY with SDC_USE_DATABASE_CURRENT and no arrays: "Return from a temporary mode to
/// the last saved display configuration" (SetDisplayConfig). Changes the PC; reached only
/// from apply_database_current, which holds the Ticket.
pub(crate) fn apply_database_current() -> Result<(), AppError> {
    // SAFETY: no arrays are passed (None gives null pointers and zero counts), which is what
    // SDC_USE_DATABASE_CURRENT requires; the call reads no caller memory.
    let code = unsafe { SetDisplayConfig(None, None, DATABASE_FLAGS) };
    log::info!("SetDisplayConfig(SDC_APPLY | SDC_USE_DATABASE_CURRENT) returned {code}");
    map_set_config(code, Stage::Apply, "SDC_APPLY | SDC_USE_DATABASE_CURRENT")
}

/// The CCD test flags. Never SDC_SAVE_TO_DATABASE or SDC_ALLOW_CHANGES (REQ-DISP-14).
pub(crate) const VALIDATE_FLAGS: SET_DISPLAY_CONFIG_FLAGS =
    SET_DISPLAY_CONFIG_FLAGS(SDC_VALIDATE.0 | SDC_USE_SUPPLIED_DISPLAY_CONFIG.0);
/// The CCD apply flags: "Set a temporary display configuration" (SetDisplayConfig).
pub(crate) const APPLY_FLAGS: SET_DISPLAY_CONFIG_FLAGS =
    SET_DISPLAY_CONFIG_FLAGS(SDC_APPLY.0 | SDC_USE_SUPPLIED_DISPLAY_CONFIG.0);
/// The restore fallback flags.
pub(crate) const DATABASE_FLAGS: SET_DISPLAY_CONFIG_FLAGS =
    SET_DISPLAY_CONFIG_FLAGS(SDC_APPLY.0 | SDC_USE_DATABASE_CURRENT.0);

/// Whether a call tests or changes, which picks the error code of a refusal.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Stage {
    Test,
    Apply,
}

/// SetDisplayConfig's return value as ResSwap reports it: 0 is success;
/// ERROR_INVALID_PARAMETER means ResSwap built bad arrays (INTERNAL); anything else is
/// MODE_TEST_FAILED at SDC_VALIDATE and MODE_REJECTED at SDC_APPLY, the code in the detail.
pub(crate) fn map_set_config(code: i32, stage: Stage, flags: &str) -> Result<(), AppError> {
    if code == 0 {
        return Ok(());
    }
    let detail = format!("SetDisplayConfig({flags}) returned {code}");
    if i64::from(code) == i64::from(ERROR_INVALID_PARAMETER.0) {
        return Err(AppError::internal(detail));
    }
    let failure = Box::new(Failure::new(detail));
    Err(match stage {
        Stage::Test => AppError::ModeTestFailed(failure),
        Stage::Apply => AppError::ModeRejected(failure),
    })
}

/// The path scaling as the window shows it (DISPLAYCONFIG_SCALING).
pub fn scaling_of(value: DISPLAYCONFIG_SCALING) -> Scaling {
    match value {
        DISPLAYCONFIG_SCALING_IDENTITY => Scaling::Identity,
        DISPLAYCONFIG_SCALING_CENTERED => Scaling::Centered,
        DISPLAYCONFIG_SCALING_STRETCHED => Scaling::Stretched,
        DISPLAYCONFIG_SCALING_ASPECTRATIOCENTEREDMAX => Scaling::AspectRatio,
        DISPLAYCONFIG_SCALING_CUSTOM => Scaling::Custom,
        DISPLAYCONFIG_SCALING_PREFERRED => Scaling::Preferred,
        _ => Scaling::Unknown,
    }
}

/// The scaling a CCD change writes into the path. Full screen asks for STRETCHED, which is
/// an intent the driver may override, so the pipeline reads it back. Keep leaves the path's
/// value, except that IDENTITY, valid only when source and target sizes match, becomes
/// PREFERRED ("no preference") when the new source size differs from the target.
pub(crate) fn scaling_for(
    choice: ScalingChoice,
    current: DISPLAYCONFIG_SCALING,
    sizes_match: bool,
) -> DISPLAYCONFIG_SCALING {
    match choice {
        ScalingChoice::FullScreen => DISPLAYCONFIG_SCALING_STRETCHED,
        ScalingChoice::Keep if current == DISPLAYCONFIG_SCALING_IDENTITY && !sizes_match => {
            DISPLAYCONFIG_SCALING_PREFERRED
        }
        ScalingChoice::Keep => current,
    }
}

/// The exact rate as Windows reports it; `None` for a zero denominator, which means "not
/// specified".
pub fn rational(value: DISPLAYCONFIG_RATIONAL) -> Option<Rational> {
    (value.Denominator != 0).then_some(Rational {
        numerator: value.Numerator,
        denominator: value.Denominator,
    })
}

/// The text of a fixed-size wide-character field, up to its first NUL and never past its
/// end.
pub(crate) fn wide_to_string(buffer: &[u16]) -> String {
    let end = buffer
        .iter()
        .position(|&unit| unit == 0)
        .unwrap_or(buffer.len());
    String::from_utf16_lossy(buffer.get(..end).unwrap_or(buffer))
}

/// The two 16-bit halves of a virtual-mode-aware index word, low half first. MSVC allocates
/// bit fields from the low bit, which would put cloneGroupId and desktopModeInfoIdx in the
/// low half; the diagnostics tool prints both readings until the hardware round settles it.
pub fn split_index_word(word: u32) -> (u16, u16) {
    ((word & 0xFFFF) as u16, (word >> 16) as u16)
}

fn read_failed(call: &str, code: u32) -> AppError {
    AppError::internal(format!("{call} returned {code}"))
}

fn read_failed_i32(call: &str, code: i32) -> AppError {
    AppError::internal(format!("{call} returned {code}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::ErrorCode;
    use windows::Win32::Devices::Display::{
        DISPLAYCONFIG_PATH_SOURCE_INFO, DISPLAYCONFIG_PATH_SOURCE_INFO_0,
        DISPLAYCONFIG_PATH_TARGET_INFO, DISPLAYCONFIG_PATH_TARGET_INFO_0,
    };

    /// SDC_SAVE_TO_DATABASE (0x200), SDC_ALLOW_CHANGES (0x400), SDC_TOPOLOGY_SUPPLIED (0x10)
    /// and SDC_FORCE_MODE_ENUMERATION (0x1000), as SetDisplayConfig and the 0.62.2 crate
    /// define them. ResSwap's P1 calls never set any of them.
    const NEVER: u32 = 0x200 | 0x400 | 0x10 | 0x1000;

    #[test]
    fn req_disp_14_ccd_flags_never_save_or_allow_changes() {
        for flags in [VALIDATE_FLAGS, APPLY_FLAGS, DATABASE_FLAGS] {
            assert_eq!(flags.0 & NEVER, 0, "{:#x}", flags.0);
        }
        // Exactly one of SDC_APPLY (0x80) and SDC_VALIDATE (0x40) in each.
        assert_eq!(VALIDATE_FLAGS.0 & 0xC0, 0x40);
        assert_eq!(APPLY_FLAGS.0 & 0xC0, 0x80);
        assert_eq!(DATABASE_FLAGS.0 & 0xC0, 0x80);
        // The supplied arrays on test and apply; the database on the fallback.
        assert_eq!(VALIDATE_FLAGS.0 & 0x20, 0x20);
        assert_eq!(APPLY_FLAGS.0 & 0x20, 0x20);
        assert_eq!(DATABASE_FLAGS.0 & 0x2F, 0x0F);
    }

    #[test]
    fn req_disp_12_set_display_config_codes_map_to_their_errors() {
        assert!(map_set_config(0, Stage::Test, "x").is_ok());
        assert!(map_set_config(0, Stage::Apply, "x").is_ok());
        // ERROR_BAD_CONFIGURATION is 1610.
        let test = map_set_config(1610, Stage::Test, "SDC_VALIDATE").unwrap_err();
        assert_eq!(test.code(), ErrorCode::ModeTestFailed);
        assert!(test.to_ipc().detail.contains("1610"));
        let apply = map_set_config(1610, Stage::Apply, "SDC_APPLY").unwrap_err();
        assert_eq!(apply.code(), ErrorCode::ModeRejected);
        // ERROR_INVALID_PARAMETER (87) from ResSwap's own arrays is a ResSwap bug.
        let own = map_set_config(87, Stage::Apply, "SDC_APPLY").unwrap_err();
        assert_eq!(own.code(), ErrorCode::Internal);
        assert!(own.to_ipc().detail.contains("87"));
    }

    #[test]
    fn req_disp_05_rational_from_vsync() {
        let exact = DISPLAYCONFIG_RATIONAL {
            Numerator: 60000,
            Denominator: 1001,
        };
        assert_eq!(
            rational(exact),
            Some(Rational {
                numerator: 60000,
                denominator: 1001
            })
        );
        // Kept as Windows reports it, not reduced.
        let whole = DISPLAYCONFIG_RATIONAL {
            Numerator: 240_000,
            Denominator: 1000,
        };
        assert_eq!(
            rational(whole),
            Some(Rational {
                numerator: 240_000,
                denominator: 1000
            })
        );
        let unspecified = DISPLAYCONFIG_RATIONAL {
            Numerator: 0,
            Denominator: 0,
        };
        assert_eq!(rational(unspecified), None);
    }

    #[test]
    fn req_nv_25_scaling_values_map_both_ways() {
        assert_eq!(scaling_of(DISPLAYCONFIG_SCALING(1)), Scaling::Identity);
        assert_eq!(scaling_of(DISPLAYCONFIG_SCALING(2)), Scaling::Centered);
        assert_eq!(scaling_of(DISPLAYCONFIG_SCALING(3)), Scaling::Stretched);
        assert_eq!(scaling_of(DISPLAYCONFIG_SCALING(4)), Scaling::AspectRatio);
        assert_eq!(scaling_of(DISPLAYCONFIG_SCALING(5)), Scaling::Custom);
        assert_eq!(scaling_of(DISPLAYCONFIG_SCALING(128)), Scaling::Preferred);
        assert_eq!(scaling_of(DISPLAYCONFIG_SCALING(0)), Scaling::Unknown);
        assert_eq!(scaling_of(DISPLAYCONFIG_SCALING(6)), Scaling::Unknown);

        let identity = DISPLAYCONFIG_SCALING_IDENTITY;
        let centered = DISPLAYCONFIG_SCALING_CENTERED;
        assert_eq!(
            scaling_for(ScalingChoice::FullScreen, identity, false),
            DISPLAYCONFIG_SCALING_STRETCHED
        );
        assert_eq!(
            scaling_for(ScalingChoice::Keep, identity, false),
            DISPLAYCONFIG_SCALING_PREFERRED
        );
        assert_eq!(scaling_for(ScalingChoice::Keep, identity, true), identity);
        assert_eq!(scaling_for(ScalingChoice::Keep, centered, false), centered);
    }

    #[test]
    fn req_disp_03_wide_strings_stop_at_the_nul_and_the_end() {
        let mut name = [0u16; 8];
        for (slot, unit) in name.iter_mut().zip("Test".encode_utf16()) {
            *slot = unit;
        }
        assert_eq!(wide_to_string(&name), "Test");
        let full: Vec<u16> = "ABCD".encode_utf16().collect();
        assert_eq!(wide_to_string(&full), "ABCD");
        assert_eq!(wide_to_string(&[0, 65]), "");
    }

    #[test]
    fn req_disp_03_edid_name_and_ids_follow_their_flags() {
        let name = |flags| TargetName {
            flags,
            output_technology: 10,
            edid_manufacture_id: 0x1234,
            edid_product_code_id: 0x5678,
            connector_instance: 0,
            friendly_name: String::from("Test monitor"),
            device_path: String::from(r"\\?\DISPLAY#TEST0001#0"),
        };
        assert_eq!(
            name(FRIENDLY_NAME_FROM_EDID).edid_name().as_deref(),
            Some("Test monitor")
        );
        assert_eq!(name(0).edid_name(), None);
        assert_eq!(name(FRIENDLY_NAME_FORCED).edid_name(), None);
        assert_eq!(name(EDID_IDS_VALID).edid_ids(), Some((0x1234, 0x5678)));
        assert_eq!(name(FRIENDLY_NAME_FROM_EDID).edid_ids(), None);
        let mut empty = name(FRIENDLY_NAME_FROM_EDID);
        empty.friendly_name.clear();
        assert_eq!(empty.edid_name(), None);
    }

    #[test]
    fn req_safe_02_mode_indexes_are_checked_against_the_mode_types() {
        let mut source = DISPLAYCONFIG_MODE_INFO {
            infoType: DISPLAYCONFIG_MODE_INFO_TYPE_SOURCE,
            ..Default::default()
        };
        source.Anonymous = DISPLAYCONFIG_MODE_INFO_0 {
            sourceMode: DISPLAYCONFIG_SOURCE_MODE {
                width: 1920,
                height: 1080,
                ..Default::default()
            },
        };
        let target = DISPLAYCONFIG_MODE_INFO {
            infoType: DISPLAYCONFIG_MODE_INFO_TYPE_TARGET,
            ..Default::default()
        };
        let mut modes = vec![target, source];
        let path = |source_index: u32, target_index: u32| DISPLAYCONFIG_PATH_INFO {
            sourceInfo: DISPLAYCONFIG_PATH_SOURCE_INFO {
                Anonymous: DISPLAYCONFIG_PATH_SOURCE_INFO_0 {
                    modeInfoIdx: source_index,
                },
                ..Default::default()
            },
            targetInfo: DISPLAYCONFIG_PATH_TARGET_INFO {
                Anonymous: DISPLAYCONFIG_PATH_TARGET_INFO_0 {
                    modeInfoIdx: target_index,
                },
                ..Default::default()
            },
            flags: 0,
        };
        let good = path(1, 0);
        assert_eq!(source_mode_index(&modes, &good), Some(1));
        assert_eq!(target_mode_index(&modes, &good), Some(0));
        assert_eq!(
            source_mode(&modes, &good).map(|mode| mode.width),
            Some(1920)
        );
        // Swapped indexes name entries of the wrong type.
        let swapped = path(0, 1);
        assert_eq!(source_mode_index(&modes, &swapped), None);
        assert_eq!(target_mode_index(&modes, &swapped), None);
        // DISPLAYCONFIG_PATH_MODE_IDX_INVALID and any index past the end name nothing.
        let invalid = path(u32::MAX, 2);
        assert_eq!(source_mode(&modes, &invalid), None);
        assert!(target_mode(&modes, &invalid).is_none());

        set_source_mode(
            &mut modes,
            1,
            DISPLAYCONFIG_SOURCE_MODE {
                width: 1440,
                height: 1080,
                ..Default::default()
            },
        );
        assert_eq!(
            source_mode(&modes, &good).map(|mode| mode.width),
            Some(1440)
        );
    }

    #[test]
    fn req_safe_02_index_words_split_both_ways() {
        assert_eq!(split_index_word(0x0003_0001), (1, 3));
        assert_eq!(split_index_word(0xFFFF_0000), (0, 0xFFFF));
        assert_eq!(split_index_word(u32::MAX), (0xFFFF, 0xFFFF));
    }
}
