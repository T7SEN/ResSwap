//! Settings the player can change, and their defaults (D-004, D-007). The store that keeps
//! them on disk and its migrations sit in the child modules.

#![forbid(unsafe_code)]

pub mod atomic;
pub mod fake;
pub mod migrations;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::display::types::{Mode, MonitorId};

/// The settings schema this build writes and reads (REQ-SET-03).
pub const SCHEMA_VERSION: u32 = 1;

/// The countdown before an unconfirmed mode reverts, in seconds (D-007).
pub const DEFAULT_REVERT_COUNTDOWN_SECS: u32 = 10;

/// What a global hotkey does.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum HotkeyAction {
    PlayStretched,
    RestoreNative,
}

/// One global hotkey and the action it runs; set only by set_hotkey (D-007).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HotkeyBinding {
    pub action: HotkeyAction,
    pub accelerator: String,
}

/// A mode the player kept on this monitor, so it applies later without the revert countdown.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConfirmedMode {
    pub monitor: MonitorId,
    pub mode: Mode,
}

/// The player's settings as settings.json holds them, with the defaults of D-007.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Settings {
    /// Read-only over the IPC; the migrations own it (D-004).
    pub schema_version: u32,
    /// 10 by default (D-007).
    pub revert_countdown_secs: u32,
    /// On by default; the player can switch it off.
    pub check_for_updates: bool,
    /// Off by default.
    pub show_hidden_monitors: bool,
    /// The monitor Home shows.
    pub selected_monitor: Option<MonitorId>,
    /// The profile that Play stretched applies from the hotkey and Home.
    pub play_stretched_profile: Option<String>,
    /// Read from the HKCU Run value; changed only by set_autostart.
    pub start_with_windows: bool,
    /// Empty by default; changed only by set_hotkey (D-007).
    pub hotkeys: Vec<HotkeyBinding>,
    /// Changed only by confirm_mode.
    pub confirmed_modes: Vec<ConfirmedMode>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            revert_countdown_secs: DEFAULT_REVERT_COUNTDOWN_SECS,
            check_for_updates: true,
            show_hidden_monitors: false,
            selected_monitor: None,
            play_stretched_profile: None,
            start_with_windows: false,
            hotkeys: Vec::new(),
            confirmed_modes: Vec::new(),
        }
    }
}

/// The fields save_settings accepts.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct SettingsInput {
    pub revert_countdown_secs: u32,
    pub check_for_updates: bool,
    pub show_hidden_monitors: bool,
    pub selected_monitor: Option<MonitorId>,
    pub play_stretched_profile: Option<String>,
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::*;
    use crate::display::modes::to_mode;
    use crate::display::types::ScalingChoice;
    use crate::display::{Rational, RawMode};
    use crate::profiles::Profile;

    fn monitor() -> MonitorId {
        MonitorId {
            device_path: String::from("fake-device-path:test-pc:lg-ultragear"),
            edid_manufacturer_id: Some(0x1E6D),
            edid_product_code: Some(0x5B8E),
            connector_instance: 0,
        }
    }

    fn stretched() -> Mode {
        to_mode(RawMode {
            width: 1280,
            height: 960,
            refresh: Rational::hz(240),
        })
    }

    /// `value` with one field no struct knows, added to the object at `pointer`.
    fn with_unknown_field(value: &Value, pointer: &str) -> Value {
        let mut changed = value.clone();
        changed
            .pointer_mut(pointer)
            .and_then(Value::as_object_mut)
            .unwrap_or_else(|| panic!("no object at {pointer:?}"))
            .insert(String::from("unknownField"), json!(1));
        changed
    }

    /// Serde does not pass deny_unknown_fields to nested structs, so each nested struct
    /// carries its own: an unknown field at any depth fails the read, which is what sends a
    /// damaged file to SETTINGS_CORRUPT.
    #[test]
    fn req_set_05_unknown_nested_fields_are_refused() {
        let settings = Settings {
            selected_monitor: Some(monitor()),
            hotkeys: vec![HotkeyBinding {
                action: HotkeyAction::PlayStretched,
                accelerator: String::from("Ctrl+Alt+F9"),
            }],
            confirmed_modes: vec![ConfirmedMode {
                monitor: monitor(),
                mode: stretched(),
            }],
            ..Settings::default()
        };
        let value = serde_json::to_value(&settings).unwrap();
        // The attribute changes reading only: the written form stays, and it reads back.
        assert_eq!(
            value["confirmedModes"][0]["mode"],
            json!({
                "width": 1280,
                "height": 960,
                "refresh": { "numerator": 240, "denominator": 1, "label": "240 Hz" },
                "label": "1280 x 960 at 240 Hz"
            })
        );
        assert_eq!(
            serde_json::from_value::<Settings>(value.clone()).unwrap(),
            settings
        );
        for pointer in [
            "",
            "/selectedMonitor",
            "/hotkeys/0",
            "/confirmedModes/0",
            "/confirmedModes/0/monitor",
            "/confirmedModes/0/mode",
            "/confirmedModes/0/mode/refresh",
        ] {
            let read = serde_json::from_value::<Settings>(with_unknown_field(&value, pointer));
            assert!(read.is_err(), "{pointer:?}");
        }

        let profile = Profile {
            id: String::from("0f8fad5b-d9cb-469f-a165-70867728950e"),
            name: String::from("Example"),
            exe: String::from("game.exe"),
            monitor: monitor(),
            mode: stretched(),
            scaling: ScalingChoice::FullScreen,
            turn_off: vec![monitor()],
            auto_switch: false,
            restore_delay_secs: 10,
            launch_path: None,
        };
        let value = serde_json::to_value(&profile).unwrap();
        assert_eq!(
            serde_json::from_value::<Profile>(value.clone()).unwrap(),
            profile
        );
        for pointer in ["", "/monitor", "/mode", "/mode/refresh", "/turnOff/0"] {
            let read = serde_json::from_value::<Profile>(with_unknown_field(&value, pointer));
            assert!(read.is_err(), "{pointer:?}");
        }
    }
}
