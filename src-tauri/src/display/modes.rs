//! Refresh rationals, labels, mode lists, presets, the target of an apply, the target of
//! Restore native and the confirmed-mode rule (REQ-DISP-04, REQ-DISP-06, REQ-DISP-09,
//! REQ-DISP-11, REQ-DISP-17, REQ-DISP-18, REQ-DISP-19, REQ-DISP-21).

#![forbid(unsafe_code)]

use std::cmp::Ordering;

use crate::display::monitors::{display_name, find_monitor};
use crate::display::types::{Aspect, Mode, MonitorId, Preset, RefreshRate, Scaling};
use crate::display::{Rational, RawMode, RawMonitor};
use crate::error::{AppError, ErrorContext, Failure};
use crate::settings::ConfirmedMode;

/// The only bit depth ResSwap lists: apps with a Windows 8 or later manifest cannot query
/// or set modes below 32 bits per pixel (EnumDisplaySettingsExW, REQ-DISP-06).
pub const LISTED_BITS_PER_PIXEL: u32 = 32;

/// Two rates are equal when n1 * d2 == n2 * d1, computed in u64. Never compare labels,
/// floats or GDI integers (REQ-DISP-17). A rate with a zero denominator is not a rate (CCD
/// uses 0/0 for "let Windows choose"), so it equals nothing, itself included.
pub fn rational_eq(a: Rational, b: Rational) -> bool {
    a.denominator != 0
        && b.denominator != 0
        && u64::from(a.numerator) * u64::from(b.denominator)
            == u64::from(b.numerator) * u64::from(a.denominator)
}

/// Orders rates by value, computed in u64 like `rational_eq`. Only for rates with a
/// denominator above zero.
pub fn rational_cmp(a: Rational, b: Rational) -> Ordering {
    (u64::from(a.numerator) * u64::from(b.denominator))
        .cmp(&(u64::from(b.numerator) * u64::from(a.denominator)))
}

/// Whether the rational `rate` lies within 1 Hz of the whole-hertz value `hz`, strictly,
/// in integers: n.abs_diff(hz * d) < d, in u64 (REQ-DISP-17). So 59 and 60 are both
/// within 1 Hz of 60000/1001, and 239 is not within 1 Hz of 240/1.
pub fn within_one_hz(rate: Rational, hz: u32) -> bool {
    let denominator = u64::from(rate.denominator);
    denominator != 0
        && u64::from(rate.numerator).abs_diff(u64::from(hz) * denominator) < denominator
}

/// The exact rate for a GDI list entry of `hz` whole hertz (REQ-DISP-17). GDI holds whole
/// hertz only, so an entry takes the monitor's current target rational (CCD vSyncFreq) when
/// `hz` is within 1 Hz of it, else the preferred mode's rational when within 1 Hz of that,
/// else hz / 1.
pub fn rate_for_gdi_entry(
    hz: u32,
    current: Option<Rational>,
    preferred: Option<Rational>,
) -> Rational {
    [current, preferred]
        .into_iter()
        .flatten()
        .find(|rate| within_one_hz(*rate, hz))
        .unwrap_or_else(|| Rational::hz(hz))
}

/// Hertz with at most two decimals, halves rounded up, trailing zeros removed: "240 Hz",
/// "59.94 Hz", "143.9 Hz".
pub fn refresh_label(rate: Rational) -> String {
    if rate.denominator == 0 {
        return String::from("unknown rate");
    }
    let numerator = u64::from(rate.numerator);
    let denominator = u64::from(rate.denominator);
    let hundredths = (numerator * 200 + denominator) / (denominator * 2);
    let whole = hundredths / 100;
    let fraction = hundredths % 100;
    if fraction == 0 {
        format!("{whole} Hz")
    } else if fraction % 10 == 0 {
        format!("{whole}.{} Hz", fraction / 10)
    } else {
        format!("{whole}.{fraction:02} Hz")
    }
}

/// The rate as the window receives it: the exact rational and its label.
pub fn to_refresh(rate: Rational) -> RefreshRate {
    RefreshRate {
        numerator: rate.numerator,
        denominator: rate.denominator,
        label: refresh_label(rate),
    }
}

/// The mode as the window receives it, with a label such as "1280 x 960 at 240 Hz".
pub fn to_mode(mode: RawMode) -> Mode {
    let refresh = to_refresh(mode.refresh);
    let label = format!("{} x {} at {}", mode.width, mode.height, refresh.label);
    Mode {
        width: mode.width,
        height: mode.height,
        refresh,
        label,
    }
}

/// The plain mode inside a window `Mode`. The labels are ignored: the core builds labels,
/// it never reads them back.
pub fn to_raw(mode: &Mode) -> RawMode {
    RawMode {
        width: mode.width,
        height: mode.height,
        refresh: Rational {
            numerator: mode.refresh.numerator,
            denominator: mode.refresh.denominator,
        },
    }
}

/// Whether two modes have the same size and equal rates.
pub fn same_mode(a: RawMode, b: RawMode) -> bool {
    a.width == b.width && a.height == b.height && rational_eq(a.refresh, b.refresh)
}

/// One entry of EnumDisplaySettingsExW as plain data, before it gets its exact rate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GdiMode {
    pub width: u32,
    pub height: u32,
    /// dmDisplayFrequency: whole hertz.
    pub hz: u32,
    pub bits_per_pixel: u32,
}

/// The raw mode list from one GDI enumeration (REQ-DISP-06): 32 bpp entries only, each with
/// its exact rate by `rate_for_gdi_entry`, and each size and rate once, in the order
/// Windows listed them. Entries of one size whose rates become equal collapse, so 59 and
/// 60 for a 59.94 Hz timing give one entry.
pub fn modes_from_gdi(
    entries: &[GdiMode],
    current: Option<Rational>,
    preferred: Option<Rational>,
) -> Vec<RawMode> {
    let mut modes: Vec<RawMode> = Vec::with_capacity(entries.len());
    for entry in entries {
        if entry.bits_per_pixel != LISTED_BITS_PER_PIXEL {
            continue;
        }
        let mode = RawMode {
            width: entry.width,
            height: entry.height,
            refresh: rate_for_gdi_entry(entry.hz, current, preferred),
        };
        if !modes.iter().any(|kept| same_mode(*kept, mode)) {
            modes.push(mode);
        }
    }
    modes
}

/// Every mode once, largest first: sizes by width then height, rates from high to low.
/// Entries of one size whose rationals are equal collapse into one (REQ-DISP-06).
pub fn mode_list(modes: &[RawMode]) -> Vec<Mode> {
    let mut unique: Vec<RawMode> = Vec::with_capacity(modes.len());
    for mode in modes {
        if !unique.iter().any(|kept| same_mode(*kept, *mode)) {
            unique.push(*mode);
        }
    }
    unique.sort_by(|a, b| {
        b.width
            .cmp(&a.width)
            .then(b.height.cmp(&a.height))
            .then(rational_cmp(b.refresh, a.refresh))
    });
    unique.into_iter().map(to_mode).collect()
}

/// The listed entry with this exact size at this rate, if any. The entry comes back as the
/// list holds it, so 480/2 asked for returns a listed 240/1.
pub fn find_listed(modes: &[RawMode], width: u32, height: u32, rate: Rational) -> Option<RawMode> {
    modes.iter().copied().find(|mode| {
        mode.width == width && mode.height == height && rational_eq(mode.refresh, rate)
    })
}

/// Whether the list holds this exact size at this exact rate.
pub fn offers(modes: &[RawMode], width: u32, height: u32, rate: Rational) -> bool {
    find_listed(modes, width, height, rate).is_some()
}

/// Every rate the list offers at this size, once each, from high to low.
pub fn rates_at(modes: &[RawMode], width: u32, height: u32) -> Vec<Rational> {
    let mut rates: Vec<Rational> = Vec::new();
    for mode in modes {
        if mode.width == width
            && mode.height == height
            && !rates.iter().any(|kept| rational_eq(*kept, mode.refresh))
        {
            rates.push(mode.refresh);
        }
    }
    rates.sort_by(|a, b| rational_cmp(*b, *a));
    rates
}

/// The width for `height` at `num:den` (REQ-DISP-09). When height x ratio is not whole, the
/// width is the nearest whole pixel, halves up, in integers: (2 * height * num + den) /
/// (2 * den). This is ResSwap's rule, not a Windows fact. A native height of 1080 gives
/// whole widths for all three ratios; 768 at 16:10 gives 1229 (1228.8) and 900 at 5:4 gives
/// 1125.
pub fn preset_width(height: u32, num: u32, den: u32) -> u32 {
    let wide = (2 * u64::from(height) * u64::from(num) + u64::from(den)) / (2 * u64::from(den));
    u32::try_from(wide).unwrap_or(u32::MAX)
}

/// The rate a new size is requested at: the current exact rational, or the preferred
/// mode's when the monitor shows no mode now (REQ-DISP-17).
pub fn keep_rate(monitor: &RawMonitor) -> Option<Rational> {
    monitor.current.or(monitor.native).map(|mode| mode.refresh)
}

/// 4:3, 5:4 and 16:10 from the native height, each at the current rate (the native rate
/// when the monitor is not active). `offered` is true only when the list holds that size at
/// that exact rate (REQ-DISP-09).
pub fn presets(
    native: Option<RawMode>,
    current: Option<RawMode>,
    modes: &[RawMode],
) -> Vec<Preset> {
    let Some(native) = native else {
        return Vec::new();
    };
    let rate = current.map_or(native.refresh, |mode| mode.refresh);
    [
        (Aspect::FourThree, 4, 3),
        (Aspect::FiveFour, 5, 4),
        (Aspect::SixteenTen, 16, 10),
    ]
    .into_iter()
    .map(|(aspect, num, den)| {
        let width = preset_width(native.height, num, den);
        let mode = RawMode {
            width,
            height: native.height,
            refresh: rate,
        };
        Preset {
            aspect,
            mode: to_mode(mode),
            offered: offers(modes, width, native.height, rate),
        }
    })
    .collect()
}

/// The listed mode an apply sets (REQ-DISP-11, REQ-DISP-17, REQ-DISP-18), from a fresh
/// read: the size must be in the list (else MODE_NOT_OFFERED), at the requested exact rate
/// (else REFRESH_NOT_AVAILABLE carrying every rate that size offers). Nothing is changed
/// either way. A zero denominator is a request the window never sends: INTERNAL.
pub fn resolve_target(
    monitor: &RawMonitor,
    width: u32,
    height: u32,
    rate: Rational,
) -> Result<RawMode, AppError> {
    if width == 0 || height == 0 || rate.denominator == 0 {
        return Err(AppError::internal(format!(
            "refused a mode request of {width} x {height} at {}/{}",
            rate.numerator, rate.denominator
        )));
    }
    if let Some(listed) = find_listed(&monitor.modes, width, height, rate) {
        return Ok(listed);
    }
    let requested = to_mode(RawMode {
        width,
        height,
        refresh: rate,
    });
    let offered = rates_at(&monitor.modes, width, height);
    if offered.is_empty() {
        let context = ErrorContext {
            monitor: display_name(monitor.name.as_deref()),
            mode: Some(requested),
            ..ErrorContext::default()
        };
        return Err(AppError::ModeNotOffered(Box::new(
            Failure::new(format!(
                "{width} x {height} is not in the monitor's mode list"
            ))
            .with_context(context),
        )));
    }
    let detail = format!(
        "{width} x {height} is listed, but not at {}/{}",
        rate.numerator, rate.denominator
    );
    let context = ErrorContext {
        monitor: display_name(monitor.name.as_deref()),
        mode: Some(requested),
        offered_refresh: offered.into_iter().map(to_refresh).collect(),
        ..ErrorContext::default()
    };
    Err(AppError::RefreshNotAvailable(Box::new(
        Failure::new(detail).with_context(context),
    )))
}

/// A custom width and height take the same path as a preset (REQ-DISP-11): requested at the
/// rate `keep_rate` gives, then `resolve_target`. A monitor with no current and no
/// preferred mode has no rate to keep: INTERNAL.
pub fn resolve_custom_size(
    monitor: &RawMonitor,
    width: u32,
    height: u32,
) -> Result<RawMode, AppError> {
    let Some(rate) = keep_rate(monitor) else {
        return Err(AppError::internal(
            "the monitor reports no current or preferred mode, so no rate to keep",
        ));
    };
    resolve_target(monitor, width, height, rate)
}

/// What Restore native asks for on one monitor (REQ-DISP-21): the preferred size at
/// `kept_rate` when the list offers it there, otherwise the preferred mode at its own rate.
/// `kept_rate` is the journal baseline's rate when ResSwap changed the monitor, else the
/// current rate. A monitor that reports no preferred mode has no native mode to restore:
/// INTERNAL, never a guess from the largest mode (REQ-DISP-04).
pub fn restore_native_target(
    monitor: &RawMonitor,
    baseline_rate: Option<Rational>,
) -> Result<RawMode, AppError> {
    let Some(native) = monitor.native else {
        return Err(AppError::internal(
            "the monitor reports no preferred mode, so Restore native has no target",
        ));
    };
    let kept_rate = baseline_rate.or_else(|| monitor.current.map(|mode| mode.refresh));
    Ok(kept_rate
        .and_then(|rate| find_listed(&monitor.modes, native.width, native.height, rate))
        .unwrap_or(native))
}

/// Whether a stored confirmed mode covers `mode` on the monitor `target` of the fresh read
/// `live` (REQ-DISP-19): the stored MonitorId names that monitor by the identity rule
/// (device path first, then EDID ids plus connector instance), the sizes are equal and the
/// rates are equal rationals.
pub fn stored_confirmed(
    confirmed: &[ConfirmedMode],
    live: &[RawMonitor],
    target: &MonitorId,
    mode: RawMode,
) -> bool {
    confirmed.iter().any(|entry| {
        same_mode(to_raw(&entry.mode), mode)
            && find_monitor(&entry.monitor, live).is_some_and(|found| found.id == *target)
    })
}

/// The two modes that count as confirmed without being stored: the monitor's baseline mode
/// exactly (size and rate before ResSwap's first change), and the preferred mode at the
/// preferred mode's own rate. The native size at any other rate is not confirmed until the
/// player keeps it.
pub fn confirmed_without_storing(
    monitor: &RawMonitor,
    baseline: Option<RawMode>,
    mode: RawMode,
) -> bool {
    baseline.is_some_and(|base| same_mode(base, mode))
        || monitor.native.is_some_and(|native| same_mode(native, mode))
}

/// Whether `mode` on `target` applies with no countdown (REQ-SAFE-04): stored, or one of the
/// two modes that count as confirmed without being stored. `false` when `target` is not in
/// `live`.
pub fn is_confirmed(
    confirmed: &[ConfirmedMode],
    live: &[RawMonitor],
    target: &MonitorId,
    baseline: Option<RawMode>,
    mode: RawMode,
) -> bool {
    let Some(monitor) = live.iter().find(|monitor| monitor.id == *target) else {
        return false;
    };
    confirmed_without_storing(monitor, baseline, mode)
        || stored_confirmed(confirmed, live, target, mode)
}

// DISPLAYCONFIG_SCALING values, copied from windows 0.62.2 (Win32::Devices::Display), so the
// journal records and the fake hold the path scaling as its number without a Win32 type.
const SCALING_IDENTITY: i32 = 1;
const SCALING_CENTERED: i32 = 2;
const SCALING_STRETCHED: i32 = 3;
const SCALING_ASPECT_RATIO: i32 = 4;
const SCALING_CUSTOM: i32 = 5;
const SCALING_PREFERRED: i32 = 128;

/// The DISPLAYCONFIG_SCALING number of a path scaling. Unknown becomes PREFERRED, "no
/// preference", because ResSwap never writes a value it could not read.
pub fn scaling_to_raw(scaling: Scaling) -> i32 {
    match scaling {
        Scaling::Identity => SCALING_IDENTITY,
        Scaling::Centered => SCALING_CENTERED,
        Scaling::Stretched => SCALING_STRETCHED,
        Scaling::AspectRatio => SCALING_ASPECT_RATIO,
        Scaling::Custom => SCALING_CUSTOM,
        Scaling::Preferred | Scaling::Unknown => SCALING_PREFERRED,
    }
}

/// The path scaling a DISPLAYCONFIG_SCALING number stands for.
pub fn scaling_from_raw(value: i32) -> Scaling {
    match value {
        SCALING_IDENTITY => Scaling::Identity,
        SCALING_CENTERED => Scaling::Centered,
        SCALING_STRETCHED => Scaling::Stretched,
        SCALING_ASPECT_RATIO => Scaling::AspectRatio,
        SCALING_CUSTOM => Scaling::Custom,
        SCALING_PREFERRED => Scaling::Preferred,
        _ => Scaling::Unknown,
    }
}

/// Whether a read-back rate verifies the request (REQ-DISP-17). A rate that came from CCD
/// (a denominator other than 1, or equal to one of `ccd_rates`, the current and preferred
/// rates read before the change) must read back exactly equal. A whole-hertz rate from the
/// GDI list passes when the read-back is within 1 Hz of it, because GDI cannot say which
/// exact timing its integer stands for.
pub fn rate_verifies(requested: Rational, read_back: Rational, ccd_rates: &[Rational]) -> bool {
    let from_ccd =
        requested.denominator != 1 || ccd_rates.iter().any(|rate| rational_eq(*rate, requested));
    if from_ccd {
        rational_eq(requested, read_back)
    } else {
        within_one_hz(read_back, requested.numerator)
    }
}

/// Whether a read-back mode verifies the request: the same size, and the rate by
/// `rate_verifies`. Anything else is MODE_REJECTED for the pipeline.
pub fn read_back_verifies(requested: RawMode, read_back: RawMode, ccd_rates: &[Rational]) -> bool {
    requested.width == read_back.width
        && requested.height == read_back.height
        && rate_verifies(requested.refresh, read_back.refresh, ccd_rates)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::display::fake::{FixtureProfile, known_monitors};
    use crate::error::ErrorCode;

    const NTSC_60: Rational = Rational {
        numerator: 60_000,
        denominator: 1_001,
    };

    fn mode(width: u32, height: u32, hz: u32) -> RawMode {
        RawMode {
            width,
            height,
            refresh: Rational::hz(hz),
        }
    }

    fn lg() -> RawMonitor {
        known_monitors(FixtureProfile::TestPc).remove(0)
    }

    fn gdi(width: u32, height: u32, hz: u32) -> GdiMode {
        GdiMode {
            width,
            height,
            hz,
            bits_per_pixel: 32,
        }
    }

    #[test]
    fn req_disp_17_rationals_compare_exactly() {
        assert!(rational_eq(Rational::hz(240), Rational::hz(240)));
        assert!(rational_eq(
            Rational {
                numerator: 480,
                denominator: 2
            },
            Rational::hz(240)
        ));
        // 60000/1001 is 59.94 Hz, never equal to 60 or 59 whole hertz.
        assert!(!rational_eq(NTSC_60, Rational::hz(60)));
        assert!(!rational_eq(NTSC_60, Rational::hz(59)));
        assert!(rational_eq(
            NTSC_60,
            Rational {
                numerator: 120_000,
                denominator: 2_002
            }
        ));
        assert_eq!(rational_cmp(NTSC_60, Rational::hz(60)), Ordering::Less);
        // Large values stay exact in u64.
        let big = Rational {
            numerator: u32::MAX,
            denominator: u32::MAX - 1,
        };
        assert!(!rational_eq(big, Rational::hz(1)));
        assert!(rational_eq(big, big));
        // 0/0 means "let Windows choose" in CCD: it equals nothing.
        let unset = Rational {
            numerator: 0,
            denominator: 0,
        };
        assert!(!rational_eq(unset, Rational::hz(240)));
        assert!(!rational_eq(unset, unset));
    }

    #[test]
    fn req_disp_17_refresh_labels() {
        assert_eq!(refresh_label(Rational::hz(240)), "240 Hz");
        assert_eq!(refresh_label(NTSC_60), "59.94 Hz");
        assert_eq!(
            refresh_label(Rational {
                numerator: 240_000,
                denominator: 1_001
            }),
            "239.76 Hz"
        );
        assert_eq!(
            refresh_label(Rational {
                numerator: 1_439,
                denominator: 10
            }),
            "143.9 Hz"
        );
        assert_eq!(
            to_mode(RawMode {
                width: 1280,
                height: 960,
                refresh: Rational::hz(240)
            })
            .label,
            "1280 x 960 at 240 Hz"
        );
    }

    #[test]
    fn req_disp_17_ntsc_rate_kept_exact() {
        let refresh = to_refresh(NTSC_60);
        assert_eq!((refresh.numerator, refresh.denominator), (60_000, 1_001));
        let window_mode = to_mode(RawMode {
            width: 1920,
            height: 1080,
            refresh: NTSC_60,
        });
        // The request the window sends back carries the rational unchanged.
        assert_eq!(to_raw(&window_mode).refresh, NTSC_60);

        let mut monitor = lg();
        monitor.modes = vec![
            RawMode {
                width: 1920,
                height: 1080,
                refresh: NTSC_60,
            },
            mode(1920, 1080, 60),
        ];
        // 59.94 Hz and 60 Hz stay two entries: different rationals.
        assert_eq!(mode_list(&monitor.modes).len(), 2);
        let found = resolve_target(&monitor, 1920, 1080, NTSC_60).unwrap();
        assert_eq!(found.refresh, NTSC_60);
        let found = resolve_target(&monitor, 1920, 1080, Rational::hz(60)).unwrap();
        assert_eq!(found.refresh, Rational::hz(60));

        monitor.modes = vec![RawMode {
            width: 1920,
            height: 1080,
            refresh: NTSC_60,
        }];
        let refused = resolve_target(&monitor, 1920, 1080, Rational::hz(60)).unwrap_err();
        assert_eq!(refused.code(), ErrorCode::RefreshNotAvailable);
        let offered: Vec<String> = refused
            .to_ipc()
            .context
            .offered_refresh
            .into_iter()
            .map(|rate| rate.label)
            .collect();
        assert_eq!(offered, vec![String::from("59.94 Hz")]);
    }

    #[test]
    fn req_disp_17_within_one_hz_is_integer_math() {
        assert!(within_one_hz(NTSC_60, 59));
        assert!(within_one_hz(NTSC_60, 60));
        assert!(!within_one_hz(NTSC_60, 61));
        assert!(!within_one_hz(NTSC_60, 58));
        assert!(within_one_hz(Rational::hz(240), 240));
        // Strictly within: 239 is a full hertz away from 240/1.
        assert!(!within_one_hz(Rational::hz(240), 239));
        let ntsc_240 = Rational {
            numerator: 240_000,
            denominator: 1_001,
        };
        assert!(within_one_hz(ntsc_240, 239));
        assert!(within_one_hz(ntsc_240, 240));
        assert!(!within_one_hz(
            Rational {
                numerator: 0,
                denominator: 0
            },
            0
        ));
    }

    #[test]
    fn req_disp_17_gdi_entries_take_the_ccd_rational() {
        let current = Some(NTSC_60);
        let preferred = Some(Rational::hz(240));
        assert_eq!(rate_for_gdi_entry(59, current, preferred), NTSC_60);
        assert_eq!(rate_for_gdi_entry(60, current, preferred), NTSC_60);
        assert_eq!(
            rate_for_gdi_entry(240, current, preferred),
            Rational::hz(240)
        );
        assert_eq!(
            rate_for_gdi_entry(144, current, preferred),
            Rational::hz(144)
        );
        assert_eq!(rate_for_gdi_entry(239, None, preferred), Rational::hz(239));
        assert_eq!(rate_for_gdi_entry(75, None, None), Rational::hz(75));
    }

    #[test]
    fn req_disp_06_gdi_list_keeps_32_bpp_once() {
        let entries = [
            gdi(1920, 1080, 59),
            gdi(1920, 1080, 60),
            GdiMode {
                bits_per_pixel: 16,
                ..gdi(1280, 960, 60)
            },
            gdi(1920, 1080, 144),
            gdi(1920, 1080, 144),
            gdi(1280, 960, 60),
        ];
        let modes = modes_from_gdi(&entries, Some(NTSC_60), Some(NTSC_60));
        assert_eq!(
            modes,
            vec![
                RawMode {
                    width: 1920,
                    height: 1080,
                    refresh: NTSC_60
                },
                mode(1920, 1080, 144),
                RawMode {
                    width: 1280,
                    height: 960,
                    refresh: NTSC_60
                },
            ]
        );
    }

    #[test]
    fn req_disp_06_mode_list_has_no_duplicates() {
        let modes = [
            mode(1280, 960, 60),
            mode(1920, 1080, 240),
            RawMode {
                width: 1920,
                height: 1080,
                refresh: Rational {
                    numerator: 480,
                    denominator: 2,
                },
            },
            mode(1920, 1080, 144),
        ];
        let labels: Vec<String> = mode_list(&modes).into_iter().map(|m| m.label).collect();
        assert_eq!(
            labels,
            vec![
                "1920 x 1080 at 240 Hz",
                "1920 x 1080 at 144 Hz",
                "1280 x 960 at 60 Hz"
            ]
        );
    }

    #[test]
    fn req_disp_09_presets_from_native_height() {
        let native = mode(1920, 1080, 240);
        let listed = [native, mode(1440, 1080, 240), mode(1350, 1080, 60)];
        let presets = presets(Some(native), Some(native), &listed);
        let sizes: Vec<(Aspect, u32, u32, bool)> = presets
            .iter()
            .map(|p| (p.aspect, p.mode.width, p.mode.height, p.offered))
            .collect();
        assert_eq!(
            sizes,
            vec![
                (Aspect::FourThree, 1440, 1080, true),
                // Listed, but only at 60 Hz, so not offered at the current 240 Hz.
                (Aspect::FiveFour, 1350, 1080, false),
                (Aspect::SixteenTen, 1728, 1080, false),
            ]
        );
        assert!(presets.iter().all(|p| p.mode.refresh.label == "240 Hz"));
        assert!(super::presets(None, Some(native), &listed).is_empty());
    }

    #[test]
    fn req_disp_09_presets_at_the_current_rate() {
        let native = mode(1920, 1080, 240);
        let current = mode(1920, 1080, 144);
        let listed = [native, current, mode(1440, 1080, 144)];
        let at_144 = presets(Some(native), Some(current), &listed);
        assert!(at_144.iter().all(|p| p.mode.refresh.label == "144 Hz"));
        assert!(at_144[0].offered);
        // Not active: the native rate.
        let inactive = presets(Some(native), None, &listed);
        assert!(inactive.iter().all(|p| p.mode.refresh.label == "240 Hz"));
        assert!(!inactive[0].offered);
    }

    #[test]
    fn req_disp_09_preset_widths_round_halves_up() {
        assert_eq!(preset_width(1080, 4, 3), 1440);
        assert_eq!(preset_width(1080, 5, 4), 1350);
        assert_eq!(preset_width(1080, 16, 10), 1728);
        // 1228.8 rounds to 1229; 1638.4 to 1638; 1125 is whole.
        assert_eq!(preset_width(768, 16, 10), 1229);
        assert_eq!(preset_width(1024, 16, 10), 1638);
        assert_eq!(preset_width(900, 5, 4), 1125);
        // 1093 x 5 / 4 is 1366.25 and rounds down; 1094 x 5 / 4 is 1367.5, a half, and
        // rounds up.
        assert_eq!(preset_width(1093, 5, 4), 1366);
        assert_eq!(preset_width(1094, 5, 4), 1368);
        assert_eq!(preset_width(u32::MAX, 16, 10), u32::MAX);
    }

    #[test]
    fn req_disp_11_custom_size_takes_the_same_path() {
        let monitor = lg();
        // Listed at the current 240 Hz.
        let found = resolve_custom_size(&monitor, 1280, 960).unwrap();
        assert_eq!(found, mode(1280, 960, 240));
        // Not listed at any rate.
        let refused = resolve_custom_size(&monitor, 1440, 1080).unwrap_err();
        assert_eq!(refused.code(), ErrorCode::ModeNotOffered);
        let context = refused.to_ipc().context;
        assert_eq!(context.monitor.as_deref(), Some("LG ULTRAGEAR"));
        assert_eq!(
            context.mode.map(|requested| requested.label),
            Some(String::from("1440 x 1080 at 240 Hz"))
        );
        assert!(context.offered_refresh.is_empty());
        // The preset path refuses the same size the same way.
        let preset = resolve_target(&monitor, 1440, 1080, Rational::hz(240)).unwrap_err();
        assert_eq!(preset.code(), ErrorCode::ModeNotOffered);

        let mut gone = monitor;
        gone.current = None;
        gone.native = None;
        let refused = resolve_custom_size(&gone, 1280, 960).unwrap_err();
        assert_eq!(refused.code(), ErrorCode::Internal);
    }

    #[test]
    fn req_disp_18_refresh_not_available_lists_offered_rates() {
        let monitor = lg();
        let refused = resolve_target(&monitor, 1920, 1080, Rational::hz(165)).unwrap_err();
        assert_eq!(refused.code(), ErrorCode::RefreshNotAvailable);
        let context = refused.to_ipc().context;
        let offered: Vec<&str> = context
            .offered_refresh
            .iter()
            .map(|rate| rate.label.as_str())
            .collect();
        assert_eq!(
            offered,
            vec![
                "240 Hz", "144 Hz", "120 Hz", "100 Hz", "75 Hz", "60 Hz", "48 Hz"
            ]
        );
        assert_eq!(
            context.mode.map(|requested| requested.label),
            Some(String::from("1920 x 1080 at 165 Hz"))
        );
        // A listed size at another listed rate is fine.
        assert_eq!(
            resolve_target(&monitor, 1920, 1080, Rational::hz(144)).unwrap(),
            mode(1920, 1080, 144)
        );
        // The window never sends 0/0 or a zero size.
        let unset = Rational {
            numerator: 0,
            denominator: 0,
        };
        for (width, height, rate) in [(1920, 1080, unset), (0, 1080, Rational::hz(240))] {
            let refused = resolve_target(&monitor, width, height, rate).unwrap_err();
            assert_eq!(refused.code(), ErrorCode::Internal);
        }
    }

    #[test]
    fn req_disp_21_restore_native_targets_the_preferred_mode() {
        let mut monitor = lg();
        monitor.current = Some(mode(1280, 960, 240));
        // ResSwap changed it, baseline at 144 Hz: the native size at 144 Hz is listed.
        assert_eq!(
            restore_native_target(&monitor, Some(Rational::hz(144))).unwrap(),
            mode(1920, 1080, 144)
        );
        // Baseline rate not listed at the native size: the preferred mode at its own rate.
        assert_eq!(
            restore_native_target(&monitor, Some(Rational::hz(165))).unwrap(),
            mode(1920, 1080, 240)
        );
        // No journal entry: the current rate when listed at the native size.
        monitor.current = Some(mode(1920, 1080, 60));
        assert_eq!(
            restore_native_target(&monitor, None).unwrap(),
            mode(1920, 1080, 60)
        );
        monitor.current = Some(RawMode {
            width: 1280,
            height: 960,
            refresh: NTSC_60,
        });
        assert_eq!(
            restore_native_target(&monitor, None).unwrap(),
            mode(1920, 1080, 240)
        );
        monitor.native = None;
        let refused = restore_native_target(&monitor, None).unwrap_err();
        assert_eq!(refused.code(), ErrorCode::Internal);
    }

    #[test]
    fn req_disp_19_stored_confirmed_modes_match_identity_size_and_rate() {
        let live = known_monitors(FixtureProfile::TwoMonitors);
        let lg = live[0].id.clone();
        let stretched = mode(1280, 960, 240);
        let entry = |monitor: MonitorId, kept: RawMode| ConfirmedMode {
            monitor,
            mode: to_mode(kept),
        };
        let confirmed = vec![entry(lg.clone(), stretched)];
        assert!(stored_confirmed(&confirmed, &live, &lg, stretched));
        // An equal rational written another way still matches.
        let halves = RawMode {
            refresh: Rational {
                numerator: 480,
                denominator: 2,
            },
            ..stretched
        };
        assert!(stored_confirmed(&confirmed, &live, &lg, halves));
        // Another rate or size is not confirmed.
        assert!(!stored_confirmed(
            &confirmed,
            &live,
            &lg,
            mode(1280, 960, 60)
        ));
        assert!(!stored_confirmed(
            &confirmed,
            &live,
            &lg,
            mode(1280, 1024, 240)
        ));
        // Another monitor is not confirmed.
        let samsung = live[2].id.clone();
        assert!(!stored_confirmed(&confirmed, &live, &samsung, stretched));
        // A stored entry from before the LG moved port still names it by its EDID ids.
        let moved = MonitorId {
            device_path: String::from("path-before-the-move"),
            ..lg.clone()
        };
        let confirmed = vec![entry(moved, stretched)];
        assert!(stored_confirmed(&confirmed, &live, &lg, stretched));
    }

    #[test]
    fn req_safe_04_baseline_and_preferred_count_as_confirmed() {
        let live = known_monitors(FixtureProfile::TestPc);
        let lg = live[0].id.clone();
        let monitor = &live[0];
        let baseline = Some(mode(1920, 1080, 144));
        // The baseline exactly.
        assert!(confirmed_without_storing(
            monitor,
            baseline,
            mode(1920, 1080, 144)
        ));
        // The preferred mode at its own rate.
        assert!(confirmed_without_storing(
            monitor,
            baseline,
            mode(1920, 1080, 240)
        ));
        // The native size at another rate is not confirmed until kept.
        assert!(!confirmed_without_storing(
            monitor,
            baseline,
            mode(1920, 1080, 60)
        ));
        assert!(!confirmed_without_storing(
            monitor,
            None,
            mode(1280, 960, 240)
        ));
        assert!(is_confirmed(&[], &live, &lg, None, mode(1920, 1080, 240)));
        assert!(!is_confirmed(&[], &live, &lg, None, mode(1280, 960, 240)));
        let confirmed = vec![ConfirmedMode {
            monitor: lg.clone(),
            mode: to_mode(mode(1280, 960, 240)),
        }];
        assert!(is_confirmed(
            &confirmed,
            &live,
            &lg,
            None,
            mode(1280, 960, 240)
        ));
        // A monitor missing from the read is never confirmed.
        assert!(!is_confirmed(
            &confirmed,
            &live[1..],
            &lg,
            None,
            mode(1280, 960, 240)
        ));
    }

    #[test]
    fn req_disp_17_read_back_rules() {
        let ccd = [NTSC_60, Rational::hz(240)];
        // A CCD rate must come back exactly.
        assert!(rate_verifies(NTSC_60, NTSC_60, &ccd));
        assert!(!rate_verifies(NTSC_60, Rational::hz(60), &ccd));
        assert!(rate_verifies(Rational::hz(240), Rational::hz(240), &ccd));
        assert!(!rate_verifies(
            Rational::hz(240),
            Rational {
                numerator: 240_000,
                denominator: 1_001
            },
            &ccd
        ));
        // A whole-hertz GDI rate passes within 1 Hz.
        let ntsc_144 = Rational {
            numerator: 144_000,
            denominator: 1_001,
        };
        assert!(rate_verifies(Rational::hz(144), ntsc_144, &ccd));
        assert!(!rate_verifies(Rational::hz(144), Rational::hz(120), &ccd));
        // A size that differs never verifies.
        assert!(read_back_verifies(
            mode(1280, 960, 240),
            mode(1280, 960, 240),
            &ccd
        ));
        assert!(!read_back_verifies(
            mode(1280, 960, 240),
            mode(1920, 1080, 240),
            &ccd
        ));
    }

    #[test]
    fn req_safe_02_scaling_numbers_round_trip() {
        for scaling in [
            Scaling::Identity,
            Scaling::Centered,
            Scaling::Stretched,
            Scaling::AspectRatio,
            Scaling::Custom,
            Scaling::Preferred,
        ] {
            assert_eq!(scaling_from_raw(scaling_to_raw(scaling)), scaling);
        }
        // The DISPLAYCONFIG_SCALING values of windows 0.62.2.
        assert_eq!(scaling_to_raw(Scaling::Stretched), 3);
        assert_eq!(scaling_to_raw(Scaling::Preferred), 128);
        assert_eq!(scaling_to_raw(Scaling::Unknown), 128);
        assert_eq!(scaling_from_raw(0), Scaling::Unknown);
    }
}
