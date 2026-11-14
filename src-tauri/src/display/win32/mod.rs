//! The Win32 display backend for the player's PC. State is always read through CCD
//! (QueryDisplayConfig and DisplayConfigGetDeviceInfo) plus the GDI mode list; a mode is
//! tested and applied as a temporary change on the route of `hw::hw_choice`: GDI with
//! CDS_FULLSCREEN by default, overridden in a debug build by RESSWAP_HW (REQ-DISP-14). No
//! call here ever saves a mode as the Windows default.

pub mod ccd;
pub mod device;
pub mod gdi;
pub mod hw;
mod snapshot;

use std::collections::HashMap;

use windows::Win32::Devices::Display::{
    DISPLAYCONFIG_MODE_INFO_TYPE_DESKTOP_IMAGE, DISPLAYCONFIG_MODE_INFO_TYPE_SOURCE,
    DISPLAYCONFIG_MODE_INFO_TYPE_TARGET, DISPLAYCONFIG_SCALING, DISPLAYCONFIG_SOURCE_MODE,
    QDC_ONLY_ACTIVE_PATHS, QDC_VIRTUAL_MODE_AWARE, QDC_VIRTUAL_REFRESH_RATE_AWARE,
    QUERY_DISPLAY_CONFIG_FLAGS,
};
use windows::Win32::Foundation::LUID;
use windows::Win32::Graphics::Gdi::DEVMODEW;

use crate::apply::Ticket;
use crate::apply::journal::LayoutRecord;
use crate::display::modes::{GdiMode, modes_from_gdi, rational_eq, within_one_hz};
use crate::display::monitors::match_monitor;
use crate::display::types::{MonitorId, ScalingChoice};
use crate::display::win32::ccd::{CcdConfig, PreferredMode, TargetName};
use crate::display::win32::gdi::{GdiAdapter, GdiEntry};
use crate::display::win32::hw::{ScalingControl, hw_choice};
use crate::display::{
    DisplayBackend, DisplayConfig, Rational, RawMode, RawMonitor, Route, RouteChoice,
    SnapshotRestore, TargetConfig,
};
use crate::error::{AppError, ErrorCode, Failure};

/// Every read and every change: active paths, 32-bit mode indexes, no virtual refresh
/// awareness. The same flags for the query a change edits, so the arrays match.
pub(crate) const READ_FLAGS: QUERY_DISPLAY_CONFIG_FLAGS = QDC_ONLY_ACTIVE_PATHS;

/// The display backend on the player's PC.
#[derive(Debug, Default)]
pub struct Win32Display;

impl DisplayBackend for Win32Display {
    fn enumerate(&self) -> Result<Vec<RawMonitor>, AppError> {
        read_monitors()
    }

    fn route(&self) -> RouteChoice {
        hw_choice().route
    }

    fn test_config(&self, config: &DisplayConfig) -> Result<(), AppError> {
        let choice = hw_choice();
        match choice.route.route {
            Route::Gdi => {
                for target in non_empty(config)? {
                    let plan = plan_gdi(target, choice.scaling)?;
                    gdi::test(&plan.device, &plan.mode)?;
                }
                Ok(())
            }
            Route::Ccd => ccd::validate(&plan_ccd(non_empty(config)?)?),
        }
    }

    fn apply_config(&self, _ticket: &Ticket, config: &DisplayConfig) -> Result<(), AppError> {
        let choice = hw_choice();
        match choice.route.route {
            Route::Gdi => {
                let flag = choice
                    .route
                    .gdi_flag
                    .ok_or_else(|| AppError::internal("the GDI route has no temporary flag"))?;
                for target in non_empty(config)? {
                    let plan = plan_gdi(target, choice.scaling)?;
                    gdi::apply(&plan.device, &plan.mode, flag)?;
                }
                Ok(())
            }
            Route::Ccd => ccd::apply(&plan_ccd(non_empty(config)?)?),
        }
    }

    fn apply_database_current(&self, _ticket: &Ticket) -> Result<(), AppError> {
        ccd::apply_database_current()
    }

    fn snapshot(&self) -> Result<LayoutRecord, AppError> {
        snapshot::take()
    }

    fn restore_snapshot(
        &self,
        _ticket: &Ticket,
        snapshot: &LayoutRecord,
    ) -> Result<SnapshotRestore, AppError> {
        snapshot::restore(snapshot)
    }
}

fn non_empty(config: &DisplayConfig) -> Result<&[TargetConfig], AppError> {
    if config.targets.is_empty() {
        return Err(AppError::internal("a display configuration with no target"));
    }
    Ok(&config.targets)
}

/// One active path as read, with every field the backend and the diagnostics tool use.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PathRead {
    /// The adapter LUID as plain numbers (LowPart, HighPart); valid for this boot only and
    /// never stored or sent.
    pub adapter_luid: (u32, i32),
    pub source_id: u32,
    pub target_id: u32,
    pub path_flags: u32,
    pub target_available: bool,
    pub output_technology: i32,
    /// DISPLAYCONFIG_PATH_TARGET_INFO.scaling as its number.
    pub scaling: i32,
    /// DISPLAYCONFIG_PATH_TARGET_INFO.refreshRate, which can be a virtual rate.
    pub path_refresh: Option<Rational>,
    pub source_index_word: u32,
    pub target_index_word: u32,
    pub source: Option<SourceRead>,
    pub target: Option<TargetRead>,
    pub target_name: Result<TargetName, ReadError>,
    pub gdi_name: Result<String, ReadError>,
    pub adapter_device_path: Result<String, ReadError>,
    pub preferred: Result<PreferredMode, ReadError>,
}

/// A device read that failed, kept as data so one failed field does not hide the rest of
/// the path: the code the app would raise with the error's log text, which the diagnostics
/// tool prints together (REQ-DISP-25).
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("{code:?}: {detail}")]
pub struct ReadError {
    pub code: ErrorCode,
    pub detail: String,
}

impl ReadError {
    fn of(error: &AppError) -> Self {
        Self {
            code: error.code(),
            detail: error.to_string(),
        }
    }
}

/// The source mode: the desktop size and position.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SourceRead {
    pub width: u32,
    pub height: u32,
    pub position_x: i32,
    pub position_y: i32,
}

/// The target mode: the timing the monitor receives.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TargetRead {
    pub active_width: u32,
    pub active_height: u32,
    pub total_width: u32,
    pub total_height: u32,
    pub vsync: Option<Rational>,
    pub hsync: Option<Rational>,
    pub pixel_rate: u64,
    pub scan_line_ordering: i32,
}

fn luid_numbers(luid: LUID) -> (u32, i32) {
    (luid.LowPart, luid.HighPart)
}

fn source_read(source: DISPLAYCONFIG_SOURCE_MODE) -> SourceRead {
    SourceRead {
        width: source.width,
        height: source.height,
        position_x: source.position.x,
        position_y: source.position.y,
    }
}

/// Every active path with its device information. Changes nothing.
pub fn read_paths() -> Result<Vec<PathRead>, AppError> {
    let config = ccd::query(READ_FLAGS)?;
    let mut adapter_paths: HashMap<(u32, i32), Result<String, ReadError>> = HashMap::new();
    let mut reads = Vec::with_capacity(config.paths.len());
    for path in &config.paths {
        let target_info = path.targetInfo;
        let source_info = path.sourceInfo;
        let luid = luid_numbers(target_info.adapterId);
        let adapter_device_path = adapter_paths
            .entry(luid)
            .or_insert_with(|| {
                ccd::adapter_name(target_info.adapterId).map_err(|error| ReadError::of(&error))
            })
            .clone();
        let target = ccd::target_mode(&config.modes, path).map(|mode| {
            let signal = mode.targetVideoSignalInfo;
            TargetRead {
                active_width: signal.activeSize.cx,
                active_height: signal.activeSize.cy,
                total_width: signal.totalSize.cx,
                total_height: signal.totalSize.cy,
                vsync: ccd::rational(signal.vSyncFreq),
                hsync: ccd::rational(signal.hSyncFreq),
                pixel_rate: signal.pixelRate,
                scan_line_ordering: signal.scanLineOrdering.0,
            }
        });
        reads.push(PathRead {
            adapter_luid: luid,
            source_id: source_info.id,
            target_id: target_info.id,
            path_flags: path.flags,
            target_available: target_info.targetAvailable.as_bool(),
            output_technology: target_info.outputTechnology.0,
            scaling: target_info.scaling.0,
            path_refresh: ccd::rational(target_info.refreshRate),
            source_index_word: ccd::source_index_word(path),
            target_index_word: ccd::target_index_word(path),
            source: ccd::source_mode(&config.modes, path).map(source_read),
            target,
            target_name: ccd::target_name(target_info.adapterId, target_info.id)
                .map_err(|error| ReadError::of(&error)),
            gdi_name: ccd::source_name(source_info.adapterId, source_info.id)
                .map_err(|error| ReadError::of(&error)),
            adapter_device_path,
            preferred: ccd::preferred_mode(target_info.adapterId, target_info.id)
                .map_err(|error| ReadError::of(&error)),
        });
    }
    Ok(reads)
}

/// The adapter's name and device instance ID, read once per adapter device path.
#[derive(Clone, Debug, Default)]
struct AdapterFacts {
    name: Option<String>,
    instance_id: String,
}

/// Every active monitor (REQ-DISP-03 to REQ-DISP-07). Changes nothing.
fn read_monitors() -> Result<Vec<RawMonitor>, AppError> {
    let paths = read_paths()?;
    let gdi_adapters = gdi::display_devices();
    let mut adapters: HashMap<String, AdapterFacts> = HashMap::new();
    let mut monitors = Vec::with_capacity(paths.len());
    for path in &paths {
        // A path whose monitor was removed can stay active with targetAvailable false for a
        // moment; it is left out as an unplugged monitor is (REQ-SAFE-09).
        if !path.target_available {
            log::info!("a display path with targetAvailable false was left out");
            continue;
        }
        let name = match &path.target_name {
            Ok(name) => name,
            Err(error) => {
                log::warn!("a display path with no target name was left out: {error}");
                continue;
            }
        };
        let gdi_name = path.gdi_name.clone().unwrap_or_default();
        let gdi_adapter = gdi_adapters
            .iter()
            .find(|adapter| !gdi_name.is_empty() && adapter.device_name == gdi_name);
        if gdi_adapter.is_some_and(GdiAdapter::is_mirroring) {
            log::info!("a mirroring-driver device was left out (REQ-DISP-07)");
            continue;
        }
        let adapter_device_path = path.adapter_device_path.clone().unwrap_or_default();
        let facts = adapters
            .entry(adapter_device_path.clone())
            .or_insert_with(|| adapter_facts(&adapter_device_path))
            .clone();
        let adapter_name = gdi_adapter
            .map(|adapter| adapter.device_string.clone())
            .filter(|text| !text.is_empty())
            .or(facts.name)
            .unwrap_or_default();
        let current = current_mode(path);
        let native = path.preferred.as_ref().ok().and_then(|preferred| {
            preferred.refresh.map(|refresh| RawMode {
                width: preferred.width,
                height: preferred.height,
                refresh,
            })
        });
        let entries = if gdi_name.is_empty() {
            Vec::new()
        } else {
            gdi::mode_entries(&gdi_name)
        };
        let modes = modes_from_gdi(
            &gdi_modes(&entries),
            current.map(|mode| mode.refresh),
            native.map(|mode| mode.refresh),
        );
        let (position_x, position_y) = path
            .source
            .map_or((0, 0), |source| (source.position_x, source.position_y));
        monitors.push(RawMonitor {
            id: monitor_id(name),
            name: name.edid_name(),
            adapter_name,
            adapter_instance_id: facts.instance_id,
            adapter_device_path,
            gdi_name,
            position_x,
            position_y,
            output_technology: i64::from(path.output_technology),
            primary: gdi_adapter.map_or(position_x == 0 && position_y == 0, |adapter| {
                adapter.is_primary()
            }),
            active: true,
            native,
            current,
            scaling: ccd::scaling_of(DISPLAYCONFIG_SCALING(path.scaling)),
            modes,
        });
    }
    Ok(monitors)
}

fn adapter_facts(adapter_device_path: &str) -> AdapterFacts {
    if adapter_device_path.is_empty() {
        return AdapterFacts::default();
    }
    match device::adapter_device(adapter_device_path) {
        Ok(found) => AdapterFacts {
            name: found.description,
            instance_id: found.instance_id,
        },
        Err(error) => {
            // An unknown instance ID does not start with PCI\, so the monitor is hidden
            // behind the show-hidden toggle rather than shown as a physical GPU's.
            log::warn!("the adapter's device instance ID could not be read: {error}");
            AdapterFacts::default()
        }
    }
}

/// The current mode: the source mode's size at the target mode's exact vSyncFreq
/// (REQ-DISP-05).
fn current_mode(path: &PathRead) -> Option<RawMode> {
    let source = path.source?;
    let refresh = path.target?.vsync?;
    Some(RawMode {
        width: source.width,
        height: source.height,
        refresh,
    })
}

/// The stored identity of the monitor behind a target name (REQ-DISP-20).
pub(crate) fn monitor_id(name: &TargetName) -> MonitorId {
    let ids = name.edid_ids();
    MonitorId {
        device_path: name.device_path.clone(),
        edid_manufacturer_id: ids.map(|(manufacturer, _)| manufacturer),
        edid_product_code: ids.map(|(_, product)| product),
        connector_instance: name.connector_instance,
    }
}

/// The GDI list as display/modes.rs takes it. Interlaced entries are left out, so a
/// progressive rational is never attached to one; modes_from_gdi keeps 32 bpp only, maps each
/// integer rate to its exact rational and collapses equal entries (REQ-DISP-06, REQ-DISP-17).
fn gdi_modes(entries: &[GdiEntry]) -> Vec<GdiMode> {
    entries
        .iter()
        .filter(|entry| !entry.interlaced)
        .map(|entry| GdiMode {
            width: entry.width,
            height: entry.height,
            hz: entry.frequency,
            bits_per_pixel: entry.bits_per_pixel,
        })
        .collect()
}

/// The GDI integer to request for `rate` among the listed rates: the one closest to the
/// rational within 1 Hz, the lower one on a tie; `None` when no listed rate is that close.
pub fn pick_frequency(listed: &[u32], rate: Rational) -> Option<u32> {
    let denominator = u64::from(rate.denominator);
    let numerator = u64::from(rate.numerator);
    listed
        .iter()
        .copied()
        .filter(|hz| within_one_hz(rate, *hz))
        .min_by_key(|hz| (numerator.abs_diff(u64::from(*hz) * denominator), *hz))
}

/// The index of the active path that shows `monitor` in `config`, checked for
/// targetAvailable. MONITOR_GONE when no path matches.
fn find_path(config: &CcdConfig, monitor: &MonitorId) -> Result<usize, AppError> {
    let mut indexes = Vec::new();
    let mut live = Vec::new();
    for (index, path) in config.paths.iter().enumerate() {
        match ccd::target_name(path.targetInfo.adapterId, path.targetInfo.id) {
            Ok(name) => {
                indexes.push(index);
                live.push(monitor_id(&name));
            }
            Err(error) => log::warn!("a display path's target name was not read: {error}"),
        }
    }
    let gone = || {
        AppError::MonitorGone(Box::new(Failure::new(
            "no active display path shows this monitor",
        )))
    };
    let live_refs: Vec<&MonitorId> = live.iter().collect();
    let position = match_monitor(monitor, &live_refs).ok_or_else(gone)?;
    let index = *indexes.get(position).ok_or_else(gone)?;
    let available = config
        .paths
        .get(index)
        .is_some_and(|path| path.targetInfo.targetAvailable.as_bool());
    if !available {
        return Err(AppError::MonitorGone(Box::new(Failure::new(
            "the monitor's display path reports targetAvailable false",
        ))));
    }
    Ok(index)
}

/// What one GDI test or apply sends.
struct GdiPlan {
    device: String,
    mode: DEVMODEW,
}

/// The GDI device of the target's live path and the DEVMODEW for the listed entry of that
/// size whose integer rate matches the requested rational (REQ-DISP-17).
fn plan_gdi(target: &TargetConfig, scaling: ScalingControl) -> Result<GdiPlan, AppError> {
    let config = ccd::query(READ_FLAGS)?;
    let index = find_path(&config, &target.monitor)?;
    let path = config
        .paths
        .get(index)
        .ok_or_else(|| AppError::internal("a display path vanished between two reads"))?;
    let device = ccd::source_name(path.sourceInfo.adapterId, path.sourceInfo.id)?;
    let requested = target.mode;
    let sized: Vec<GdiEntry> = gdi::mode_entries(&device)
        .into_iter()
        .filter(|entry| {
            entry.usable() && entry.width == requested.width && entry.height == requested.height
        })
        .collect();
    if sized.is_empty() {
        return Err(AppError::ModeNotOffered(Box::new(Failure::new(format!(
            "{device} does not list {} x {} at 32 bpp",
            requested.width, requested.height
        )))));
    }
    let listed: Vec<u32> = sized.iter().map(|entry| entry.frequency).collect();
    let Some(frequency) = pick_frequency(&listed, requested.refresh) else {
        return Err(AppError::RefreshNotAvailable(Box::new(Failure::new(
            format!(
                "{device} lists {} x {} at {listed:?} Hz, none within 1 Hz of {}/{}",
                requested.width,
                requested.height,
                requested.refresh.numerator,
                requested.refresh.denominator
            ),
        ))));
    };
    let entry = GdiEntry {
        frequency,
        ..sized[0]
    };
    let stretch = target.scaling == ScalingChoice::FullScreen && scaling == ScalingControl::Gdi;
    Ok(GdiPlan {
        device,
        mode: gdi::build_devmode(&entry, stretch),
    })
}

/// The arrays for one CCD test or apply: a fresh snapshot with each target's source mode set
/// to the requested size, its target mode (the timing) kept, and its path scaling set for
/// the request ("Scaling the desktop image"). The requested rate must
/// be the kept timing's rate, because this route changes only the source.
fn plan_ccd(targets: &[TargetConfig]) -> Result<CcdConfig, AppError> {
    let mut config = ccd::query(READ_FLAGS)?;
    for target in targets {
        let index = find_path(&config, &target.monitor)?;
        let path = *config
            .paths
            .get(index)
            .ok_or_else(|| AppError::internal("a display path vanished between two reads"))?;
        let source_index = ccd::source_mode_index(&config.modes, &path).ok_or_else(|| {
            AppError::internal("the monitor's display path has no source mode entry")
        })?;
        let source = ccd::source_mode(&config.modes, &path).ok_or_else(|| {
            AppError::internal("the monitor's display path has no source mode entry")
        })?;
        let timing = ccd::target_mode(&config.modes, &path)
            .ok_or_else(|| {
                AppError::internal("the monitor's display path has no target mode entry")
            })?
            .targetVideoSignalInfo;
        let kept = ccd::rational(timing.vSyncFreq).ok_or_else(|| {
            AppError::internal("the monitor's target mode reports no refresh rate")
        })?;
        let requested = target.mode.refresh;
        let same_rate = rational_eq(kept, requested)
            || (requested.denominator == 1 && within_one_hz(kept, requested.numerator));
        if !same_rate {
            return Err(AppError::RefreshNotAvailable(Box::new(Failure::new(
                format!(
                    "the CCD route keeps the target timing at {}/{}, not {}/{}",
                    kept.numerator, kept.denominator, requested.numerator, requested.denominator
                ),
            ))));
        }
        let sizes_match =
            target.mode.width == timing.activeSize.cx && target.mode.height == timing.activeSize.cy;
        ccd::set_source_mode(
            &mut config.modes,
            source_index,
            DISPLAYCONFIG_SOURCE_MODE {
                width: target.mode.width,
                height: target.mode.height,
                ..source
            },
        );
        if let Some(path) = config.paths.get_mut(index) {
            path.targetInfo.scaling =
                ccd::scaling_for(target.scaling, path.targetInfo.scaling, sizes_match);
        }
    }
    Ok(config)
}

/// One of the four query forms the diagnostics tool compares.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QueryForm {
    pub label: &'static str,
    pub flags: u32,
    pub result: Result<Vec<IndexForm>, ReadError>,
}

/// One path of a query form: its raw index words, both 16-bit readings and whether each
/// reading names a mode entry of the right type.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IndexForm {
    pub path_flags: u32,
    pub mode_count: usize,
    pub source_word: u32,
    pub target_word: u32,
    /// The path's refreshRate, which can be a virtual rate.
    pub path_refresh: Option<Rational>,
    /// The vSyncFreq of the entry the 32-bit target index names, when it is a target entry.
    pub target_vsync_32: Option<Rational>,
    /// Low half as cloneGroupId and desktopModeInfoIdx, high half as sourceModeInfoIdx and
    /// targetModeInfoIdx (bit fields allocated from the low bit).
    pub low_first: HalfReading,
    /// The other order.
    pub high_first: HalfReading,
}

/// What one 16-bit reading of the index words names.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HalfReading {
    pub source_index: u16,
    pub target_index: u16,
    pub desktop_index: u16,
    /// The source index names a source entry.
    pub source_valid: bool,
    /// The target index names a target entry.
    pub target_valid: bool,
    /// The desktop index names a desktop image entry (0xFFFF means not specified).
    pub desktop_valid: bool,
}

/// The path and mode arrays read four ways: with and without QDC_VIRTUAL_MODE_AWARE and
/// QDC_VIRTUAL_REFRESH_RATE_AWARE, so the hardware round can pick one awareness for the
/// whole app. Reads only; for the diagnostics tool.
pub fn query_forms() -> Vec<QueryForm> {
    let forms = [
        ("QDC_ONLY_ACTIVE_PATHS", QDC_ONLY_ACTIVE_PATHS),
        (
            "QDC_ONLY_ACTIVE_PATHS | QDC_VIRTUAL_MODE_AWARE",
            QDC_ONLY_ACTIVE_PATHS | QDC_VIRTUAL_MODE_AWARE,
        ),
        (
            "QDC_ONLY_ACTIVE_PATHS | QDC_VIRTUAL_REFRESH_RATE_AWARE",
            QDC_ONLY_ACTIVE_PATHS | QDC_VIRTUAL_REFRESH_RATE_AWARE,
        ),
        (
            "QDC_ONLY_ACTIVE_PATHS | QDC_VIRTUAL_MODE_AWARE | QDC_VIRTUAL_REFRESH_RATE_AWARE",
            QDC_ONLY_ACTIVE_PATHS | QDC_VIRTUAL_MODE_AWARE | QDC_VIRTUAL_REFRESH_RATE_AWARE,
        ),
    ];
    forms
        .into_iter()
        .map(|(label, flags)| QueryForm {
            label,
            flags: flags.0,
            result: ccd::query(flags)
                .map(|config| index_forms(&config))
                .map_err(|error| ReadError::of(&error)),
        })
        .collect()
}

fn index_forms(config: &CcdConfig) -> Vec<IndexForm> {
    let kind_at = |index: u16| {
        config
            .modes
            .get(usize::from(index))
            .map(|mode| mode.infoType)
    };
    let reading = |source: u16, target: u16, desktop: u16| HalfReading {
        source_index: source,
        target_index: target,
        desktop_index: desktop,
        source_valid: kind_at(source) == Some(DISPLAYCONFIG_MODE_INFO_TYPE_SOURCE),
        target_valid: kind_at(target) == Some(DISPLAYCONFIG_MODE_INFO_TYPE_TARGET),
        desktop_valid: kind_at(desktop) == Some(DISPLAYCONFIG_MODE_INFO_TYPE_DESKTOP_IMAGE),
    };
    config
        .paths
        .iter()
        .map(|path| {
            let source_word = ccd::source_index_word(path);
            let target_word = ccd::target_index_word(path);
            let (source_low, source_high) = ccd::split_index_word(source_word);
            let (target_low, target_high) = ccd::split_index_word(target_word);
            IndexForm {
                path_flags: path.flags,
                mode_count: config.modes.len(),
                source_word,
                target_word,
                path_refresh: ccd::rational(path.targetInfo.refreshRate),
                target_vsync_32: ccd::target_mode(&config.modes, path)
                    .and_then(|mode| ccd::rational(mode.targetVideoSignalInfo.vSyncFreq)),
                low_first: reading(source_high, target_high, target_low),
                high_first: reading(source_low, target_low, target_high),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rate(numerator: u32, denominator: u32) -> Rational {
        Rational {
            numerator,
            denominator,
        }
    }

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
    fn req_disp_06_gdi_list_collapses_equal_rates_and_drops_unusable_entries() {
        let current = Some(rate(60000, 1001));
        let entries = [
            entry(1920, 1080, 59),
            entry(1920, 1080, 60),
            entry(1920, 1080, 50),
            entry(1280, 960, 60),
            GdiEntry {
                bits_per_pixel: 16,
                ..entry(800, 600, 60)
            },
            GdiEntry {
                interlaced: true,
                ..entry(1920, 1080, 30)
            },
        ];
        let modes = modes_from_gdi(&gdi_modes(&entries), current, None);
        assert_eq!(
            modes,
            vec![
                RawMode {
                    width: 1920,
                    height: 1080,
                    refresh: rate(60000, 1001)
                },
                RawMode {
                    width: 1920,
                    height: 1080,
                    refresh: Rational::hz(50)
                },
                RawMode {
                    width: 1280,
                    height: 960,
                    refresh: rate(60000, 1001)
                },
            ]
        );
    }

    #[test]
    fn req_disp_17_pick_frequency_for_the_rational() {
        let ntsc = rate(60000, 1001);
        // 60 is closer to 59.94 than 59 is.
        assert_eq!(pick_frequency(&[59, 60, 50], ntsc), Some(60));
        assert_eq!(pick_frequency(&[59, 50], ntsc), Some(59));
        assert_eq!(pick_frequency(&[50, 75], ntsc), None);
        assert_eq!(
            pick_frequency(&[240, 239, 144], Rational::hz(240)),
            Some(240)
        );
        assert_eq!(pick_frequency(&[239, 144], Rational::hz(240)), None);
        assert_eq!(pick_frequency(&[], Rational::hz(60)), None);
    }

    /// A failed device read keeps its code beside the log text, and the diagnostics tool's
    /// line shows both.
    #[test]
    fn req_disp_25_failed_reads_keep_their_code() {
        let gone = AppError::MonitorGone(Box::new(Failure::new("DisplayConfigGetDeviceInfo 31")));
        let read = ReadError::of(&gone);
        assert_eq!(read.code, ErrorCode::MonitorGone);
        assert_eq!(read.detail, gone.to_string());
        assert_eq!(
            read.to_string(),
            "MonitorGone: the monitor is not connected: DisplayConfigGetDeviceInfo 31"
        );
        let internal = ReadError::of(&AppError::internal("no source mode entry"));
        assert_eq!(internal.code, ErrorCode::Internal);
        assert_eq!(
            internal.to_string(),
            "Internal: internal error: no source mode entry"
        );
    }

    // Reads the active display paths of the PC this runs on; changes nothing.
    #[test]
    #[ignore = "reads the real PC; run by name"]
    fn req_disp_05_read_paths_on_this_pc() {
        let paths = read_paths().unwrap();
        assert!(!paths.is_empty());
        let monitors = Win32Display.enumerate().unwrap();
        assert!(!monitors.is_empty());
    }
}
