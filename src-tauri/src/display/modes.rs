//! Refresh rationals, labels, mode lists and presets (REQ-DISP-09, REQ-DISP-17).

#![forbid(unsafe_code)]

use std::cmp::Ordering;

use crate::display::types::{Aspect, Mode, Preset, RefreshRate};
use crate::display::{Rational, RawMode};

/// Two rates are equal when n1 * d2 == n2 * d1, computed in u64. Never compare labels,
/// floats or GDI integers (REQ-DISP-17).
pub fn rational_eq(a: Rational, b: Rational) -> bool {
    u64::from(a.numerator) * u64::from(b.denominator)
        == u64::from(b.numerator) * u64::from(a.denominator)
}

/// Orders rates by value, computed in u64 like `rational_eq`.
pub fn rational_cmp(a: Rational, b: Rational) -> Ordering {
    (u64::from(a.numerator) * u64::from(b.denominator))
        .cmp(&(u64::from(b.numerator) * u64::from(a.denominator)))
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

/// Every mode once, largest first: sizes by width then height, rates from high to low.
/// Entries of one size whose rationals are equal collapse into one.
pub fn mode_list(modes: &[RawMode]) -> Vec<Mode> {
    let mut unique: Vec<RawMode> = Vec::with_capacity(modes.len());
    for mode in modes {
        let seen = unique.iter().any(|kept| {
            kept.width == mode.width
                && kept.height == mode.height
                && rational_eq(kept.refresh, mode.refresh)
        });
        if !seen {
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

/// Whether the list holds this exact size at this exact rate.
pub fn offers(modes: &[RawMode], width: u32, height: u32, rate: Rational) -> bool {
    modes
        .iter()
        .any(|mode| mode.width == width && mode.height == height && rational_eq(mode.refresh, rate))
}

/// The width for `height` at `num:den`, rounded to the nearest whole pixel with halves up,
/// in integers: (2 * height * num + den) / (2 * den).
pub fn preset_width(height: u32, num: u32, den: u32) -> u32 {
    let wide = (2 * u64::from(height) * u64::from(num) + u64::from(den)) / (2 * u64::from(den));
    u32::try_from(wide).unwrap_or(u32::MAX)
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

#[cfg(test)]
mod tests {
    use super::*;

    const NTSC_60: Rational = Rational {
        numerator: 60_000,
        denominator: 1_001,
    };

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
        assert_eq!(rational_cmp(NTSC_60, Rational::hz(60)), Ordering::Less);
    }

    #[test]
    fn req_disp_17_refresh_labels() {
        assert_eq!(refresh_label(Rational::hz(240)), "240 Hz");
        assert_eq!(refresh_label(NTSC_60), "59.94 Hz");
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
    fn req_disp_09_presets_from_native_height() {
        let native = RawMode {
            width: 1920,
            height: 1080,
            refresh: Rational::hz(240),
        };
        let listed = [
            native,
            RawMode {
                width: 1440,
                height: 1080,
                refresh: Rational::hz(240),
            },
            RawMode {
                width: 1350,
                height: 1080,
                refresh: Rational::hz(60),
            },
        ];
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
    fn req_disp_09_preset_widths_round_halves_up() {
        assert_eq!(preset_width(768, 16, 10), 1229);
        assert_eq!(preset_width(900, 5, 4), 1125);
        assert_eq!(preset_width(1080, 4, 3), 1440);
    }

    #[test]
    fn req_disp_06_mode_list_has_no_duplicates() {
        let modes = [
            RawMode {
                width: 1280,
                height: 960,
                refresh: Rational::hz(60),
            },
            RawMode {
                width: 1920,
                height: 1080,
                refresh: Rational::hz(240),
            },
            RawMode {
                width: 1920,
                height: 1080,
                refresh: Rational {
                    numerator: 480,
                    denominator: 2,
                },
            },
            RawMode {
                width: 1920,
                height: 1080,
                refresh: Rational::hz(144),
            },
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
}
