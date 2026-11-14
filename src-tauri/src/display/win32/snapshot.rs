//! The CCD snapshot the journal keeps (REQ-SAFE-02): every active path and mode from one
//! QueryDisplayConfig call, as plain records, with each adapter's device path and each
//! path's target identity; and its restore (REQ-DISP-22, D-024): adapter LUIDs mapped to
//! this boot's by device path, targets that are gone or report targetAvailable false left
//! out, then SetDisplayConfig with SDC_APPLY and SDC_USE_SUPPLIED_DISPLAY_CONFIG. Paths and
//! modes are never mixed from two queries, and every mode index is rebuilt for the arrays
//! actually sent.

use std::collections::HashMap;

use windows::Win32::Devices::Display::{
    DISPLAYCONFIG_2DREGION, DISPLAYCONFIG_DESKTOP_IMAGE_INFO, DISPLAYCONFIG_MODE_INFO,
    DISPLAYCONFIG_MODE_INFO_0, DISPLAYCONFIG_MODE_INFO_TYPE,
    DISPLAYCONFIG_MODE_INFO_TYPE_DESKTOP_IMAGE, DISPLAYCONFIG_MODE_INFO_TYPE_SOURCE,
    DISPLAYCONFIG_MODE_INFO_TYPE_TARGET, DISPLAYCONFIG_PATH_INFO, DISPLAYCONFIG_PATH_SOURCE_INFO,
    DISPLAYCONFIG_PATH_SOURCE_INFO_0, DISPLAYCONFIG_PATH_TARGET_INFO,
    DISPLAYCONFIG_PATH_TARGET_INFO_0, DISPLAYCONFIG_PIXELFORMAT, DISPLAYCONFIG_RATIONAL,
    DISPLAYCONFIG_ROTATION, DISPLAYCONFIG_SCALING, DISPLAYCONFIG_SCANLINE_ORDERING,
    DISPLAYCONFIG_SOURCE_MODE, DISPLAYCONFIG_TARGET_MODE, DISPLAYCONFIG_VIDEO_OUTPUT_TECHNOLOGY,
    DISPLAYCONFIG_VIDEO_SIGNAL_INFO, DISPLAYCONFIG_VIDEO_SIGNAL_INFO_0,
};
use windows::Win32::Foundation::{LUID, POINTL, RECTL};
use windows::Win32::Graphics::Gdi::DISPLAYCONFIG_PATH_MODE_IDX_INVALID;
use windows::core::BOOL;

use crate::apply::journal::{
    AdapterRecord, DesktopImageRecord, LayoutRecord, LuidRecord, ModeInfoRecord, ModeRecord,
    PathRecord, PathSourceRecord, PathTargetRecord, PointRecord, RectRecord, RegionRecord,
    SourceModeRecord, TargetModeRecord,
};
use crate::display::monitors::match_monitor;
use crate::display::types::MonitorId;
use crate::display::win32::ccd::{self, CcdConfig};
use crate::display::win32::{READ_FLAGS, monitor_id};
use crate::display::{Rational, SnapshotRestore};
use crate::error::AppError;

fn luid_record(luid: LUID) -> LuidRecord {
    LuidRecord {
        low_part: luid.LowPart,
        high_part: luid.HighPart,
    }
}

fn luid(record: LuidRecord) -> LUID {
    LUID {
        LowPart: record.low_part,
        HighPart: record.high_part,
    }
}

/// The rational exactly as Windows wrote it, a zero denominator included.
fn raw_rational(value: DISPLAYCONFIG_RATIONAL) -> Rational {
    Rational {
        numerator: value.Numerator,
        denominator: value.Denominator,
    }
}

fn win_rational(value: Rational) -> DISPLAYCONFIG_RATIONAL {
    DISPLAYCONFIG_RATIONAL {
        Numerator: value.numerator,
        Denominator: value.denominator,
    }
}

/// One path as a record, field by field; the index unions as their raw 32-bit words.
pub(crate) fn path_record(path: &DISPLAYCONFIG_PATH_INFO) -> PathRecord {
    let source = path.sourceInfo;
    let target = path.targetInfo;
    PathRecord {
        source_info: PathSourceRecord {
            adapter_id: luid_record(source.adapterId),
            id: source.id,
            mode_info_idx: ccd::source_index_word(path),
            status_flags: source.statusFlags,
        },
        target_info: PathTargetRecord {
            adapter_id: luid_record(target.adapterId),
            id: target.id,
            mode_info_idx: ccd::target_index_word(path),
            output_technology: target.outputTechnology.0,
            rotation: target.rotation.0,
            scaling: target.scaling.0,
            refresh_rate: raw_rational(target.refreshRate),
            scan_line_ordering: target.scanLineOrdering.0,
            target_available: target.targetAvailable.0,
            status_flags: target.statusFlags,
        },
        flags: path.flags,
    }
}

/// One mode as a record: the union member its infoType names. Any other type is refused,
/// because a snapshot that left an entry out would shift every index after it.
pub(crate) fn mode_record(mode: &DISPLAYCONFIG_MODE_INFO) -> Result<ModeRecord, AppError> {
    let info = match mode.infoType {
        DISPLAYCONFIG_MODE_INFO_TYPE_TARGET => {
            // SAFETY: infoType is DISPLAYCONFIG_MODE_INFO_TYPE_TARGET, which says the union
            // holds targetMode; the entry is plain data QueryDisplayConfig wrote.
            let target = unsafe { mode.Anonymous.targetMode };
            let signal = target.targetVideoSignalInfo;
            // SAFETY: both members of this union are 32-bit views of the same bits, and every
            // bit pattern is a valid u32.
            let video_standard = unsafe { signal.Anonymous.videoStandard };
            ModeInfoRecord::Target(TargetModeRecord {
                pixel_rate: signal.pixelRate,
                h_sync_freq: raw_rational(signal.hSyncFreq),
                v_sync_freq: raw_rational(signal.vSyncFreq),
                active_size: RegionRecord {
                    cx: signal.activeSize.cx,
                    cy: signal.activeSize.cy,
                },
                total_size: RegionRecord {
                    cx: signal.totalSize.cx,
                    cy: signal.totalSize.cy,
                },
                video_standard,
                scan_line_ordering: signal.scanLineOrdering.0,
            })
        }
        DISPLAYCONFIG_MODE_INFO_TYPE_SOURCE => {
            // SAFETY: infoType is DISPLAYCONFIG_MODE_INFO_TYPE_SOURCE, which says the union
            // holds sourceMode; the entry is plain data QueryDisplayConfig wrote.
            let source = unsafe { mode.Anonymous.sourceMode };
            ModeInfoRecord::Source(SourceModeRecord {
                width: source.width,
                height: source.height,
                pixel_format: source.pixelFormat.0,
                position: PointRecord {
                    x: source.position.x,
                    y: source.position.y,
                },
            })
        }
        DISPLAYCONFIG_MODE_INFO_TYPE_DESKTOP_IMAGE => {
            // SAFETY: infoType is DISPLAYCONFIG_MODE_INFO_TYPE_DESKTOP_IMAGE, which says the
            // union holds desktopImageInfo; the entry is plain data QueryDisplayConfig wrote.
            let image = unsafe { mode.Anonymous.desktopImageInfo };
            ModeInfoRecord::DesktopImage(DesktopImageRecord {
                path_source_size: PointRecord {
                    x: image.PathSourceSize.x,
                    y: image.PathSourceSize.y,
                },
                desktop_image_region: rect_record(image.DesktopImageRegion),
                desktop_image_clip: rect_record(image.DesktopImageClip),
            })
        }
        other => {
            return Err(AppError::internal(format!(
                "QueryDisplayConfig returned a mode entry of unknown type {}",
                other.0
            )));
        }
    };
    Ok(ModeRecord {
        info_type: mode.infoType.0,
        id: mode.id,
        adapter_id: luid_record(mode.adapterId),
        info,
    })
}

fn rect_record(rect: RECTL) -> RectRecord {
    RectRecord {
        left: rect.left,
        top: rect.top,
        right: rect.right,
        bottom: rect.bottom,
    }
}

fn rect(record: RectRecord) -> RECTL {
    RECTL {
        left: record.left,
        top: record.top,
        right: record.right,
        bottom: record.bottom,
    }
}

/// A record back as the Win32 path.
pub(crate) fn path_info(record: &PathRecord) -> DISPLAYCONFIG_PATH_INFO {
    let source = record.source_info;
    let target = record.target_info;
    DISPLAYCONFIG_PATH_INFO {
        sourceInfo: DISPLAYCONFIG_PATH_SOURCE_INFO {
            adapterId: luid(source.adapter_id),
            id: source.id,
            Anonymous: DISPLAYCONFIG_PATH_SOURCE_INFO_0 {
                modeInfoIdx: source.mode_info_idx,
            },
            statusFlags: source.status_flags,
        },
        targetInfo: DISPLAYCONFIG_PATH_TARGET_INFO {
            adapterId: luid(target.adapter_id),
            id: target.id,
            Anonymous: DISPLAYCONFIG_PATH_TARGET_INFO_0 {
                modeInfoIdx: target.mode_info_idx,
            },
            outputTechnology: DISPLAYCONFIG_VIDEO_OUTPUT_TECHNOLOGY(target.output_technology),
            rotation: DISPLAYCONFIG_ROTATION(target.rotation),
            scaling: DISPLAYCONFIG_SCALING(target.scaling),
            refreshRate: win_rational(target.refresh_rate),
            scanLineOrdering: DISPLAYCONFIG_SCANLINE_ORDERING(target.scan_line_ordering),
            targetAvailable: BOOL(target.target_available),
            statusFlags: target.status_flags,
        },
        flags: record.flags,
    }
}

/// A record back as the Win32 mode entry.
pub(crate) fn mode_info(record: &ModeRecord) -> DISPLAYCONFIG_MODE_INFO {
    let anonymous = match record.info {
        ModeInfoRecord::Target(target) => DISPLAYCONFIG_MODE_INFO_0 {
            targetMode: DISPLAYCONFIG_TARGET_MODE {
                targetVideoSignalInfo: DISPLAYCONFIG_VIDEO_SIGNAL_INFO {
                    pixelRate: target.pixel_rate,
                    hSyncFreq: win_rational(target.h_sync_freq),
                    vSyncFreq: win_rational(target.v_sync_freq),
                    activeSize: DISPLAYCONFIG_2DREGION {
                        cx: target.active_size.cx,
                        cy: target.active_size.cy,
                    },
                    totalSize: DISPLAYCONFIG_2DREGION {
                        cx: target.total_size.cx,
                        cy: target.total_size.cy,
                    },
                    Anonymous: DISPLAYCONFIG_VIDEO_SIGNAL_INFO_0 {
                        videoStandard: target.video_standard,
                    },
                    scanLineOrdering: DISPLAYCONFIG_SCANLINE_ORDERING(target.scan_line_ordering),
                },
            },
        },
        ModeInfoRecord::Source(source) => DISPLAYCONFIG_MODE_INFO_0 {
            sourceMode: DISPLAYCONFIG_SOURCE_MODE {
                width: source.width,
                height: source.height,
                pixelFormat: DISPLAYCONFIG_PIXELFORMAT(source.pixel_format),
                position: POINTL {
                    x: source.position.x,
                    y: source.position.y,
                },
            },
        },
        ModeInfoRecord::DesktopImage(image) => DISPLAYCONFIG_MODE_INFO_0 {
            desktopImageInfo: DISPLAYCONFIG_DESKTOP_IMAGE_INFO {
                PathSourceSize: POINTL {
                    x: image.path_source_size.x,
                    y: image.path_source_size.y,
                },
                DesktopImageRegion: rect(image.desktop_image_region),
                DesktopImageClip: rect(image.desktop_image_clip),
            },
        },
    };
    DISPLAYCONFIG_MODE_INFO {
        infoType: DISPLAYCONFIG_MODE_INFO_TYPE(record.info_type),
        id: record.id,
        adapterId: luid(record.adapter_id),
        Anonymous: anonymous,
    }
}

/// The current configuration as a snapshot record. Changes nothing.
pub(crate) fn take() -> Result<LayoutRecord, AppError> {
    let config = ccd::query(READ_FLAGS)?;
    let mut adapters: Vec<AdapterRecord> = Vec::new();
    let adapter_ids = config
        .paths
        .iter()
        .flat_map(|path| [path.sourceInfo.adapterId, path.targetInfo.adapterId])
        .chain(config.modes.iter().map(|mode| mode.adapterId));
    for adapter in adapter_ids {
        let record = luid_record(adapter);
        if adapters.iter().any(|known| known.luid == record) {
            continue;
        }
        let adapter_device_path = ccd::adapter_name(adapter).unwrap_or_else(|error| {
            log::warn!("an adapter's device path was not read for the snapshot: {error}");
            String::new()
        });
        adapters.push(AdapterRecord {
            luid: record,
            adapter_device_path,
        });
    }
    let modes = config
        .modes
        .iter()
        .map(mode_record)
        .collect::<Result<Vec<_>, _>>()?;
    let targets = config
        .paths
        .iter()
        .map(|path| {
            ccd::target_name(path.targetInfo.adapterId, path.targetInfo.id)
                .map(|name| monitor_id(&name))
                .unwrap_or_else(|error| {
                    // An identity that matches nothing: the restore leaves this path out.
                    log::warn!("a path's target was not named for the snapshot: {error}");
                    MonitorId {
                        device_path: String::new(),
                        edid_manufacturer_id: None,
                        edid_product_code: None,
                        connector_instance: 0,
                    }
                })
        })
        .collect();
    Ok(LayoutRecord {
        virtual_mode_aware: false,
        adapters,
        paths: config.paths.iter().map(path_record).collect(),
        modes,
        targets,
        taken_ms: 0,
    })
}

/// The arrays a restore sends.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Remapped {
    pub(crate) paths: Vec<PathRecord>,
    pub(crate) modes: Vec<ModeRecord>,
    pub(crate) left_out: Vec<MonitorId>,
}

/// Builds the arrays that put `saved` back on the configuration `live` reads now: each
/// saved path whose adapters map to a live LUID by device path and whose target is connected
/// with targetAvailable true, in saved order, then every available live path whose target
/// the snapshot does not hold, as it is, so a monitor connected since stays on. Each mode a path names is
/// sent once, with its adapter LUID mapped and every index rebuilt for the new arrays.
pub(crate) fn remap(saved: &LayoutRecord, live: &LayoutRecord) -> Remapped {
    let live_luid = |device_path: &str| {
        live.adapters
            .iter()
            .find(|adapter| !device_path.is_empty() && adapter.adapter_device_path == device_path)
            .map(|adapter| adapter.luid)
    };
    let map_saved = |old: LuidRecord| {
        saved
            .adapters
            .iter()
            .find(|adapter| adapter.luid == old)
            .and_then(|adapter| live_luid(&adapter.adapter_device_path))
    };
    let live_ids: Vec<&MonitorId> = live.targets.iter().collect();
    let mut out = Remapped::default();
    let mut indexes: HashMap<(bool, u32), u32> = HashMap::new();
    let mut covered: Vec<usize> = Vec::new();

    // Copies one mode entry into the new arrays once, and gives its new index.
    let mut copy =
        |out: &mut Remapped, from_saved: bool, index: u32, adapter: LuidRecord| -> Option<u32> {
            if index == DISPLAYCONFIG_PATH_MODE_IDX_INVALID {
                return Some(DISPLAYCONFIG_PATH_MODE_IDX_INVALID);
            }
            if let Some(known) = indexes.get(&(from_saved, index)) {
                return Some(*known);
            }
            let source = if from_saved { saved } else { live };
            let mut mode = *source.modes.get(usize::try_from(index).ok()?)?;
            mode.adapter_id = adapter;
            let new_index = u32::try_from(out.modes.len()).ok()?;
            out.modes.push(mode);
            indexes.insert((from_saved, index), new_index);
            Some(new_index)
        };

    for (index, path) in saved.paths.iter().enumerate() {
        let identity = saved.targets.get(index).cloned().unwrap_or(MonitorId {
            device_path: String::new(),
            edid_manufacturer_id: None,
            edid_product_code: None,
            connector_instance: 0,
        });
        let live_index = match_monitor(&identity, &live_ids);
        let available = live_index
            .and_then(|found| live.paths.get(found))
            .is_some_and(|live_path| live_path.target_info.target_available != 0);
        let luids = (
            map_saved(path.source_info.adapter_id),
            map_saved(path.target_info.adapter_id),
        );
        let (true, Some(source_luid), Some(target_luid)) = (available, luids.0, luids.1) else {
            out.left_out.push(identity);
            continue;
        };
        let mut kept = *path;
        kept.source_info.adapter_id = source_luid;
        kept.target_info.adapter_id = target_luid;
        let source_index = copy(&mut out, true, path.source_info.mode_info_idx, source_luid);
        let target_index = copy(&mut out, true, path.target_info.mode_info_idx, target_luid);
        let (Some(source_index), Some(target_index)) = (source_index, target_index) else {
            out.left_out.push(identity);
            continue;
        };
        kept.source_info.mode_info_idx = source_index;
        kept.target_info.mode_info_idx = target_index;
        out.paths.push(kept);
        if let Some(found) = live_index {
            covered.push(found);
        }
    }
    for (index, path) in live.paths.iter().enumerate() {
        if covered.contains(&index) || path.target_info.target_available == 0 {
            continue;
        }
        let mut kept = *path;
        let source_index = copy(
            &mut out,
            false,
            path.source_info.mode_info_idx,
            path.source_info.adapter_id,
        );
        let target_index = copy(
            &mut out,
            false,
            path.target_info.mode_info_idx,
            path.target_info.adapter_id,
        );
        let (Some(source_index), Some(target_index)) = (source_index, target_index) else {
            continue;
        };
        kept.source_info.mode_info_idx = source_index;
        kept.target_info.mode_info_idx = target_index;
        out.paths.push(kept);
    }
    out
}

/// Applies `saved` as `remap` builds it. Changes the PC; reached only from
/// restore_snapshot, which holds the Ticket.
pub(crate) fn restore(saved: &LayoutRecord) -> Result<SnapshotRestore, AppError> {
    if saved.virtual_mode_aware {
        return Err(AppError::internal(
            "the snapshot used virtual-mode-aware indexes, which this build never writes",
        ));
    }
    let live = take()?;
    let plan = remap(saved, &live);
    if plan.paths.is_empty() {
        return Ok(SnapshotRestore {
            left_out: plan.left_out,
        });
    }
    let config = CcdConfig {
        paths: plan.paths.iter().map(path_info).collect(),
        modes: plan.modes.iter().map(mode_info).collect(),
    };
    ccd::apply(&config)?;
    Ok(SnapshotRestore {
        left_out: plan.left_out,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(path: &str) -> MonitorId {
        MonitorId {
            device_path: path.to_owned(),
            edid_manufacturer_id: None,
            edid_product_code: None,
            connector_instance: 0,
        }
    }

    fn luid_of(high: i32, low: u32) -> LuidRecord {
        LuidRecord {
            low_part: low,
            high_part: high,
        }
    }

    fn source(width: u32, adapter: LuidRecord) -> ModeRecord {
        ModeRecord {
            info_type: DISPLAYCONFIG_MODE_INFO_TYPE_SOURCE.0,
            id: 0,
            adapter_id: adapter,
            info: ModeInfoRecord::Source(SourceModeRecord {
                width,
                height: 1080,
                pixel_format: 4,
                position: PointRecord { x: 0, y: 0 },
            }),
        }
    }

    fn target(adapter: LuidRecord) -> ModeRecord {
        ModeRecord {
            info_type: DISPLAYCONFIG_MODE_INFO_TYPE_TARGET.0,
            id: 0,
            adapter_id: adapter,
            info: ModeInfoRecord::Target(TargetModeRecord {
                pixel_rate: 1,
                h_sync_freq: Rational::hz(1),
                v_sync_freq: Rational::hz(240),
                active_size: RegionRecord { cx: 1920, cy: 1080 },
                total_size: RegionRecord { cx: 2000, cy: 1100 },
                video_standard: 0,
                scan_line_ordering: 1,
            }),
        }
    }

    fn path(
        adapter: LuidRecord,
        source_index: u32,
        target_index: u32,
        available: i32,
    ) -> PathRecord {
        PathRecord {
            source_info: PathSourceRecord {
                adapter_id: adapter,
                id: 0,
                mode_info_idx: source_index,
                status_flags: 0,
            },
            target_info: PathTargetRecord {
                adapter_id: adapter,
                id: 0,
                mode_info_idx: target_index,
                output_technology: 10,
                rotation: 1,
                scaling: 3,
                refresh_rate: Rational::hz(240),
                scan_line_ordering: 1,
                target_available: available,
                status_flags: 0,
            },
            flags: 1,
        }
    }

    fn layout(boot: i32, width: u32, monitors: &[&str]) -> LayoutRecord {
        let adapter = luid_of(boot, 1);
        let mut record = LayoutRecord {
            virtual_mode_aware: false,
            adapters: vec![AdapterRecord {
                luid: adapter,
                adapter_device_path: String::from("adapter-path"),
            }],
            paths: Vec::new(),
            modes: Vec::new(),
            targets: Vec::new(),
            taken_ms: 0,
        };
        for (index, monitor) in monitors.iter().enumerate() {
            let base = u32::try_from(index * 2).unwrap();
            record.modes.push(source(width, adapter));
            record.modes.push(target(adapter));
            record.paths.push(path(adapter, base, base + 1, 1));
            record.targets.push(id(monitor));
        }
        record
    }

    #[test]
    fn req_disp_22_inj_d21_snapshot_luids_map_by_device_path() {
        // Saved on boot 0 at 1920 wide; the live configuration has new LUIDs (boot 7).
        let saved = layout(0, 1920, &["lg"]);
        let live = layout(7, 1280, &["lg"]);
        let plan = remap(&saved, &live);
        assert!(plan.left_out.is_empty());
        assert_eq!(plan.paths.len(), 1);
        assert_eq!(plan.paths[0].source_info.adapter_id, luid_of(7, 1));
        assert_eq!(plan.paths[0].target_info.adapter_id, luid_of(7, 1));
        assert!(
            plan.modes
                .iter()
                .all(|mode| mode.adapter_id == luid_of(7, 1))
        );
        // The saved width goes back, not the live one.
        assert_eq!(plan.modes, {
            let mut modes = saved.modes.clone();
            for mode in &mut modes {
                mode.adapter_id = luid_of(7, 1);
            }
            modes
        });
    }

    #[test]
    fn req_safe_09_snapshot_leaves_out_gone_and_unavailable_targets() {
        let saved = layout(0, 1920, &["lg", "samsung"]);
        // Only the LG is connected now, and its path reports targetAvailable false.
        let mut live = layout(0, 1280, &["lg"]);
        live.paths[0].target_info.target_available = 0;
        let plan = remap(&saved, &live);
        assert_eq!(plan.left_out, vec![id("lg"), id("samsung")]);
        // The live path that reports targetAvailable false is not sent either.
        assert!(plan.paths.is_empty());

        // An adapter whose device path is not live is left out too.
        let mut other = layout(0, 1280, &["lg"]);
        other.adapters[0].adapter_device_path = String::from("another-adapter");
        let plan = remap(&saved, &other);
        assert_eq!(plan.left_out.len(), 2);
    }

    #[test]
    fn req_disp_22_snapshot_keeps_monitors_connected_since() {
        let saved = layout(0, 1920, &["lg"]);
        let mut live = layout(0, 1280, &["lg", "new-monitor"]);
        live.modes[2] = source(1024, luid_of(0, 1));
        let plan = remap(&saved, &live);
        assert!(plan.left_out.is_empty());
        assert_eq!(plan.paths.len(), 2);
        // The new monitor's path keeps its live mode, with indexes rebuilt after the saved ones.
        assert_eq!(plan.paths[1].source_info.mode_info_idx, 2);
        assert_eq!(plan.paths[1].target_info.mode_info_idx, 3);
        assert_eq!(plan.modes[2], source(1024, luid_of(0, 1)));
        assert_eq!(plan.modes.len(), 4);
    }

    #[test]
    fn req_safe_02_records_round_trip_through_the_win32_structs() {
        let saved = layout(3, 1440, &["lg"]);
        for record in &saved.paths {
            assert_eq!(path_record(&path_info(record)), *record);
        }
        for record in &saved.modes {
            assert_eq!(mode_record(&mode_info(record)).unwrap(), *record);
        }
        let image = ModeRecord {
            info_type: DISPLAYCONFIG_MODE_INFO_TYPE_DESKTOP_IMAGE.0,
            id: 2,
            adapter_id: luid_of(3, 1),
            info: ModeInfoRecord::DesktopImage(DesktopImageRecord {
                path_source_size: PointRecord { x: 1920, y: 1080 },
                desktop_image_region: RectRecord {
                    left: 0,
                    top: 0,
                    right: 1920,
                    bottom: 1080,
                },
                desktop_image_clip: RectRecord {
                    left: 0,
                    top: 0,
                    right: 1920,
                    bottom: 1080,
                },
            }),
        };
        assert_eq!(mode_record(&mode_info(&image)).unwrap(), image);
        // An entry of a type ResSwap does not know is refused.
        let mut unknown = mode_info(&image);
        unknown.infoType = DISPLAYCONFIG_MODE_INFO_TYPE(9);
        assert!(mode_record(&unknown).is_err());
        // An invalid index stays invalid.
        let mut no_target = saved.clone();
        no_target.paths[0].target_info.mode_info_idx = DISPLAYCONFIG_PATH_MODE_IDX_INVALID;
        let plan = remap(&no_target, &saved);
        assert_eq!(
            plan.paths[0].target_info.mode_info_idx,
            DISPLAYCONFIG_PATH_MODE_IDX_INVALID
        );
        assert_eq!(plan.modes.len(), 1);
    }
}
