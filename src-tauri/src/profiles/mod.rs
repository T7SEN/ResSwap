//! Profiles: a game's exe name, the monitor and mode to apply, and its validated ID.

#![forbid(unsafe_code)]

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::display::types::{Mode, MonitorId, ScalingChoice};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Profile {
    /// Lowercase UUID, created by the window with crypto.randomUUID() for a new profile.
    pub id: String,
    pub name: String,
    /// Exe file name only, for example "game.exe": no folder, ends in ".exe", matched
    /// without regard to case against the process list.
    pub exe: String,
    pub monitor: MonitorId,
    pub mode: Mode,
    pub scaling: ScalingChoice,
    /// Secondary monitors to turn off while the profile is active.
    pub turn_off: Vec<MonitorId>,
    pub auto_switch: bool,
    /// Delay before restoring after the exe exits; 10 by default (D-007).
    pub restore_delay_secs: u32,
    /// What a Play shortcut starts after applying the profile.
    pub launch_path: Option<String>,
}

/// A lowercase UUID in the 8-4-4-4-12 hexadecimal form.
pub fn is_lowercase_uuid(text: &str) -> bool {
    let bytes = text.as_bytes();
    bytes.len() == 36
        && bytes.iter().enumerate().all(|(index, byte)| match index {
            8 | 13 | 18 | 23 => *byte == b'-',
            _ => matches!(byte, b'0'..=b'9' | b'a'..=b'f'),
        })
}

/// A profile ID that passed `is_lowercase_uuid`. Code after the trust line takes this, never
/// the raw text (REQ-PROF-08).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ProfileId(String);

impl ProfileId {
    pub fn parse(text: &str) -> Option<Self> {
        is_lowercase_uuid(text).then(|| Self(text.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn req_prof_08_profile_ids_are_lowercase_uuids() {
        let id = "0f8fad5b-d9cb-469f-a165-70867728950e";
        assert_eq!(
            ProfileId::parse(id).map(|p| p.as_str().to_owned()),
            Some(id.to_owned())
        );
        for refused in [
            "0F8FAD5B-D9CB-469F-A165-70867728950E",
            "0f8fad5b-d9cb-469f-a165-70867728950",
            "0f8fad5b-d9cb-469f-a165-70867728950e0",
            "0f8fad5bd9cb-469f-a165-70867728950e-",
            "0f8fad5b-d9cb-469f-a165-70867728950g",
            "",
            " 0f8fad5b-d9cb-469f-a165-70867728950",
        ] {
            assert!(ProfileId::parse(refused).is_none(), "{refused}");
        }
    }
}
