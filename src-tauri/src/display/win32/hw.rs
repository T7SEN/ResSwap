//! The route switch for the hardware rounds. Every build uses the default route; a debug
//! build on the Win32 backend may override it with RESSWAP_HW, so each option can be tried
//! on the player's PC before the choice is fixed in code (REQ-DISP-14). A release build
//! never reads the variable (REQ-SEC-05).

use std::sync::OnceLock;

use crate::display::{GdiFlag, Route, RouteChoice};

/// Which Windows control carries a full-screen request.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScalingControl {
    /// The GDI call carries DM_DISPLAYFIXEDOUTPUT with DMDFO_STRETCH (GDI route default).
    Gdi,
    /// The CCD path carries DISPLAYCONFIG_SCALING_STRETCHED (CCD route default).
    Ccd,
    /// The GDI call leaves DM_DISPLAYFIXEDOUTPUT clear even for a full-screen request, which
    /// "Scaling the desktop image" maps to DMDFO_DEFAULT. GDI route only.
    GdiDefault,
}

/// The route, the GDI flag and the scaling control one run applies with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HwChoice {
    pub route: RouteChoice,
    pub scaling: ScalingControl,
}

impl HwChoice {
    /// GDI with CDS_FULLSCREEN and DMDFO_STRETCH: the research default until the hardware
    /// round fixes the route.
    pub const DEFAULT: Self = Self {
        route: RouteChoice {
            route: Route::Gdi,
            gdi_flag: Some(GdiFlag::Fullscreen),
        },
        scaling: ScalingControl::Gdi,
    };
}

/// How loudly a note is logged.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NoteLevel {
    Info,
    Warn,
}

/// One remark about the RESSWAP_HW value, logged once when the backend first picks its route.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HwNote {
    pub level: NoteLevel,
    pub text: String,
}

/// The choice and every remark about the value it came from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HwParse {
    pub choice: HwChoice,
    pub notes: Vec<HwNote>,
}

static CHOICE: OnceLock<HwChoice> = OnceLock::new();

/// The choice of this run, read once. The first call logs every note.
pub fn hw_choice() -> HwChoice {
    *CHOICE.get_or_init(|| {
        let parsed = hw_parse();
        for note in &parsed.notes {
            match note.level {
                NoteLevel::Info => log::info!("RESSWAP_HW: {}", note.text),
                NoteLevel::Warn => log::warn!("RESSWAP_HW: {}", note.text),
            }
        }
        log::info!(
            "display route {:?}, GDI flag {:?}, scaling control {:?}",
            parsed.choice.route.route,
            parsed.choice.route.gdi_flag,
            parsed.choice.scaling
        );
        parsed.choice
    })
}

/// The choice this process would make, with its notes, without logging; for the
/// diagnostics tool. Reads RESSWAP_HW only in a debug build.
pub fn hw_parse() -> HwParse {
    choose(requested_hw().as_deref())
}

/// The default in a release build whatever the value; the parsed value in a debug build.
fn choose(requested: Option<&str>) -> HwParse {
    parse_hw(requested.filter(|_| cfg!(debug_assertions)))
}

#[cfg(debug_assertions)]
fn requested_hw() -> Option<String> {
    std::env::var("RESSWAP_HW").ok()
}

#[cfg(not(debug_assertions))]
fn requested_hw() -> Option<String> {
    None
}

/// Parses `key=value` pairs separated by commas. Spaces around a pair are ignored; keys and
/// values are matched exactly, in lower case. An unknown key or value keeps the default
/// for that part and adds a warning.
pub fn parse_hw(value: Option<&str>) -> HwParse {
    let mut notes = Vec::new();
    let mut route = None;
    let mut gdi_flag = None;
    let mut scaling = None;
    let mut warn = |text: String| {
        notes.push(HwNote {
            level: NoteLevel::Warn,
            text,
        });
    };
    let mut infos = Vec::new();

    for pair in value
        .unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|pair| !pair.is_empty())
    {
        let Some((key, choice)) = pair.split_once('=') else {
            warn(format!("ignored {pair:?}: not a key=value pair"));
            continue;
        };
        match (key, choice) {
            ("route", "gdi") => route = Some(Route::Gdi),
            ("route", "ccd") => route = Some(Route::Ccd),
            ("gdiflag", "fullscreen") => gdi_flag = Some(GdiFlag::Fullscreen),
            ("gdiflag", "zero") => gdi_flag = Some(GdiFlag::Zero),
            ("scaling", "gdi") => scaling = Some(ScalingControl::Gdi),
            ("scaling", "ccd") => scaling = Some(ScalingControl::Ccd),
            ("scaling", "gdidefault") => scaling = Some(ScalingControl::GdiDefault),
            ("scaling", "nvapi") => infos.push(format!(
                "{pair} is read by the NVIDIA extras; this build keeps the route's own control"
            )),
            ("nvroute", "nvapi" | "nvmodes" | "nativetiming") => infos.push(format!(
                "{pair} is read by the NVIDIA extras; this build does not use it"
            )),
            ("route" | "gdiflag" | "scaling" | "nvroute", _) => {
                warn(format!("ignored {pair:?}: unknown value for {key}"));
            }
            _ => warn(format!("ignored {pair:?}: unknown key")),
        }
    }

    let route = route.unwrap_or(HwChoice::DEFAULT.route.route);
    let route_choice = match route {
        Route::Gdi => RouteChoice {
            route,
            gdi_flag: Some(gdi_flag.unwrap_or(GdiFlag::Fullscreen)),
        },
        Route::Ccd => {
            if gdi_flag.is_some() {
                warn(String::from(
                    "ignored gdiflag: it applies to the GDI route only",
                ));
            }
            RouteChoice {
                route,
                gdi_flag: None,
            }
        }
    };
    let own = match route {
        Route::Gdi => ScalingControl::Gdi,
        Route::Ccd => ScalingControl::Ccd,
    };
    let scaling = match (route, scaling) {
        (_, None) => own,
        (Route::Gdi, Some(control @ (ScalingControl::Gdi | ScalingControl::GdiDefault)))
        | (Route::Ccd, Some(control @ ScalingControl::Ccd)) => control,
        (_, Some(control)) => {
            warn(format!(
                "ignored scaling {control:?} on the {route:?} route; this build uses {own:?}"
            ));
            own
        }
    };

    notes.extend(infos.into_iter().map(|text| HwNote {
        level: NoteLevel::Info,
        text,
    }));
    HwParse {
        choice: HwChoice {
            route: route_choice,
            scaling,
        },
        notes,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn choice(value: &str) -> HwChoice {
        parse_hw(Some(value)).choice
    }

    fn warnings(value: &str) -> usize {
        parse_hw(Some(value))
            .notes
            .iter()
            .filter(|note| note.level == NoteLevel::Warn)
            .count()
    }

    #[test]
    fn req_disp_14_resswap_hw_default_is_gdi_fullscreen() {
        assert_eq!(parse_hw(None).choice, HwChoice::DEFAULT);
        assert!(parse_hw(None).notes.is_empty());
        assert_eq!(choice(""), HwChoice::DEFAULT);
        assert_eq!(choice(" , "), HwChoice::DEFAULT);
        assert_eq!(
            HwChoice::DEFAULT.route,
            RouteChoice {
                route: Route::Gdi,
                gdi_flag: Some(GdiFlag::Fullscreen)
            }
        );
    }

    #[test]
    fn req_disp_14_resswap_hw_picks_each_route_and_flag() {
        assert_eq!(
            choice("route=gdi,gdiflag=zero"),
            HwChoice {
                route: RouteChoice {
                    route: Route::Gdi,
                    gdi_flag: Some(GdiFlag::Zero)
                },
                scaling: ScalingControl::Gdi,
            }
        );
        assert_eq!(choice("route=gdi, gdiflag=fullscreen"), HwChoice::DEFAULT);
        assert_eq!(
            choice("route=ccd"),
            HwChoice {
                route: RouteChoice {
                    route: Route::Ccd,
                    gdi_flag: None
                },
                scaling: ScalingControl::Ccd,
            }
        );
        // A flag alone keeps the GDI route.
        assert_eq!(choice("gdiflag=zero").route.gdi_flag, Some(GdiFlag::Zero));
        assert_eq!(
            choice("route=gdi,scaling=gdidefault").scaling,
            ScalingControl::GdiDefault
        );
        assert_eq!(warnings("route=gdi,gdiflag=zero,scaling=gdi"), 0);
    }

    #[test]
    fn req_disp_14_resswap_hw_unknown_parts_keep_the_default() {
        for value in [
            "route=both",
            "Route=ccd",
            "route",
            "gdiflag=1",
            "colour=blue",
            "scaling=stretch",
        ] {
            let parsed = parse_hw(Some(value));
            assert_eq!(parsed.choice, HwChoice::DEFAULT, "{value}");
            assert_eq!(warnings(value), 1, "{value}");
        }
        // gdiflag on the CCD route is ignored with a warning.
        let parsed = parse_hw(Some("route=ccd,gdiflag=zero"));
        assert_eq!(parsed.choice.route.gdi_flag, None);
        assert_eq!(warnings("route=ccd,gdiflag=zero"), 1);
        // A control that does not belong to the route falls back to the route's own.
        assert_eq!(choice("route=gdi,scaling=ccd").scaling, ScalingControl::Gdi);
        assert_eq!(choice("route=ccd,scaling=gdi").scaling, ScalingControl::Ccd);
        assert_eq!(
            choice("route=ccd,scaling=gdidefault").scaling,
            ScalingControl::Ccd
        );
        assert_eq!(warnings("route=gdi,scaling=ccd"), 1);
    }

    #[test]
    fn req_disp_14_resswap_hw_nvidia_keys_are_accepted_and_noted() {
        let parsed = parse_hw(Some("route=ccd,nvroute=nvmodes,scaling=nvapi"));
        assert_eq!(parsed.choice.route.route, Route::Ccd);
        assert_eq!(parsed.choice.scaling, ScalingControl::Ccd);
        assert_eq!(parsed.notes.len(), 2);
        assert!(
            parsed
                .notes
                .iter()
                .all(|note| note.level == NoteLevel::Info)
        );
        assert_eq!(warnings("nvroute=nvapi"), 0);
        assert_eq!(warnings("nvroute=nativetiming"), 0);
        assert_eq!(warnings("nvroute=other"), 1);
    }

    #[test]
    fn req_sec_05_resswap_hw_only_in_debug_builds() {
        let parsed = choose(Some("route=ccd"));
        if cfg!(debug_assertions) {
            assert_eq!(parsed.choice.route.route, Route::Ccd);
        } else {
            assert_eq!(parsed.choice, HwChoice::DEFAULT);
        }
    }
}
