//! Settings the player can change, the profiles, and settings.json, the one versioned file
//! that holds both (D-004, D-007). The store that keeps it on disk and its migrations sit in
//! the child modules.

#![forbid(unsafe_code)]

pub mod atomic;
pub mod fake;
pub mod migrations;

use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use ts_rs::TS;

use crate::display::modes;
use crate::display::monitors::same_monitor;
use crate::display::types::{Mode, MonitorId};
use crate::error::{AppError, ErrorContext, Failure, SettingsSource};
use crate::profiles::{Profile, is_lowercase_uuid};
use crate::settings::atomic::{Backup, DiskStore, MAX_FILE_BYTES, Store};
use crate::settings::migrations::{MigrationError, SETTINGS_STEPS};
use crate::system::clock::Clock;

/// The settings schema this build writes and reads (REQ-SET-03).
pub const SCHEMA_VERSION: u32 = 1;

/// The settings file in the data folder (REQ-SET-01).
pub const SETTINGS_FILE: &str = "settings.json";

/// The last good copy a save keeps (REQ-SET-04).
pub const SETTINGS_BACKUP_FILE: &str = "settings.json.bak";

/// Where settings.json keeps its version: in the settings object, the one place the window
/// can read it, so the file has a single version field.
const VERSION_POINTER: &str = "/settings/schemaVersion";

/// The countdown before an unconfirmed mode reverts, in seconds (D-007).
pub const DEFAULT_REVERT_COUNTDOWN_SECS: u32 = 10;

/// The wait after a profile's exe exits before ResSwap restores, in seconds (D-007).
pub const DEFAULT_RESTORE_DELAY_SECS: u32 = 10;

/// The time between two process snapshots while auto-switch runs (D-007).
pub const WATCHER_INTERVAL_MS: u64 = 2_000;

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

impl Settings {
    /// Copies the fields save_settings accepts. The caller validates them first; the revert
    /// countdown range joins that check when it is set (REQ-SET-08).
    pub fn apply_input(&mut self, input: SettingsInput) {
        self.revert_countdown_secs = input.revert_countdown_secs;
        self.check_for_updates = input.check_for_updates;
        self.show_hidden_monitors = input.show_hidden_monitors;
        self.selected_monitor = input.selected_monitor;
        self.play_stretched_profile = input.play_stretched_profile;
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

/// settings.json as a whole: the settings and the profiles in one file, so one atomic write
/// saves both. Rust only; it never crosses the IPC. Its version is `settings.schemaVersion`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SettingsFile {
    pub settings: Settings,
    /// In the order the player sees them in the window and the tray.
    pub profiles: Vec<Profile>,
}

impl SettingsFile {
    /// The first check that fails, if any. Load and save run the same checks, so a file
    /// ResSwap wrote always loads again. Checks that depend on the PC (a monitor present, a
    /// mode offered) run when a value is used, because a monitor may be unplugged. The
    /// countdown and restore delay ranges, and the other profile limits, join this check
    /// when they are set (REQ-SET-08, REQ-PROF-03).
    fn problem(&self) -> Option<&'static str> {
        let settings = &self.settings;
        if settings.schema_version != SCHEMA_VERSION {
            return Some("settings.schemaVersion is not the version this build writes");
        }
        if settings
            .play_stretched_profile
            .as_deref()
            .is_some_and(|id| !is_lowercase_uuid(id))
        {
            return Some("playStretchedProfile is not a lowercase UUID");
        }
        let mut modes = settings
            .confirmed_modes
            .iter()
            .map(|confirmed| &confirmed.mode)
            .chain(self.profiles.iter().map(|profile| &profile.mode));
        if modes.any(|mode| mode.refresh.denominator == 0) {
            return Some("a refresh rate has the denominator 0");
        }
        for (index, profile) in self.profiles.iter().enumerate() {
            if !is_lowercase_uuid(&profile.id) {
                return Some("a profile ID is not a lowercase UUID");
            }
            if self.profiles[..index]
                .iter()
                .any(|earlier| earlier.id == profile.id)
            {
                return Some("two profiles share an ID");
            }
        }
        None
    }
}

/// Why one of the settings files could not be used. Each becomes the detail of
/// SETTINGS_CORRUPT.
#[derive(Debug, thiserror::Error)]
enum Damage {
    #[error("the read failed: {0}")]
    Unreadable(std::io::Error),
    #[error("the file is larger than the 1 MiB cap")]
    TooLarge,
    #[error("the file is not valid JSON: {0}")]
    NotJson(serde_json::Error),
    #[error("{0}")]
    Version(MigrationError),
    #[error("the file does not match the settings shape: {0}")]
    Shape(serde_json::Error),
    #[error("a value failed its check: {0}")]
    Check(&'static str),
}

/// Reads one settings file: the size cap, the JSON, the migrations on the raw JSON, the
/// shape with unknown fields refused at every depth, then the checks (REQ-SET-03).
fn read_file(store: &dyn Store, name: &str) -> Result<Option<SettingsFile>, Damage> {
    let Some(bytes) = store.read(name).map_err(Damage::Unreadable)? else {
        return Ok(None);
    };
    if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > MAX_FILE_BYTES {
        return Err(Damage::TooLarge);
    }
    let raw: Value = serde_json::from_slice(&bytes).map_err(Damage::NotJson)?;
    let current = migrations::run(raw, VERSION_POINTER, SETTINGS_STEPS, SCHEMA_VERSION)
        .map_err(Damage::Version)?;
    let file: SettingsFile = serde_json::from_value(current).map_err(Damage::Shape)?;
    match file.problem() {
        Some(problem) => Err(Damage::Check(problem)),
        None => Ok(Some(file)),
    }
}

/// What reading settings.json at start gave.
#[derive(Debug)]
pub struct Loaded {
    pub file: SettingsFile,
    /// SETTINGS_CORRUPT when settings.json was damaged or written by a newer build, for
    /// start-up to raise (REQ-SET-05).
    pub error: Option<AppError>,
    /// Set when such a file could not be moved aside. Saves are then refused, so it is never
    /// overwritten without a copy.
    pub write_refused: Option<String>,
}

/// Reads settings.json from `store`. A missing file gives the defaults with nothing to
/// report: a first run, or a file the player deleted. A damaged file, or one a newer build
/// wrote, is moved aside as settings.damaged-<ms>.json or settings.newer-<ms>.json and kept;
/// memory gets settings.json.bak, or the defaults when that fails too; those settings are
/// written back to settings.json without touching the last good copy; and the result
/// carries SETTINGS_CORRUPT (REQ-SET-05). A newer file is never migrated down. `now_ms`
/// names the kept file.
pub fn load(store: &dyn Store, now_ms: u64) -> Loaded {
    let damage = match read_file(store, SETTINGS_FILE) {
        Ok(Some(file)) => {
            return Loaded {
                file,
                error: None,
                write_refused: None,
            };
        }
        Ok(None) => {
            return Loaded {
                file: SettingsFile::default(),
                error: None,
                write_refused: None,
            };
        }
        Err(damage) => damage,
    };
    let kind = match damage {
        Damage::Version(MigrationError::Newer { .. }) => "newer",
        _ => "damaged",
    };
    let aside = format!("settings.{kind}-{now_ms}.json");
    let mut detail = format!("{SETTINGS_FILE}: {damage}");
    let moved = store.rename(SETTINGS_FILE, &aside);
    let (file, source) = match read_file(store, SETTINGS_BACKUP_FILE) {
        Ok(Some(file)) => (file, SettingsSource::LastGoodCopy),
        Ok(None) => {
            detail.push_str("; there is no last good copy");
            (SettingsFile::default(), SettingsSource::Defaults)
        }
        Err(backup) => {
            detail.push_str(&format!("; {SETTINGS_BACKUP_FILE}: {backup}"));
            (SettingsFile::default(), SettingsSource::Defaults)
        }
    };
    let write_refused = match moved {
        Ok(()) => {
            detail.push_str(&format!("; kept as {aside}"));
            // The next start then reads these settings, and the last good copy stays.
            if let Err(error) = save(store, &file, Backup::Skip) {
                detail.push_str(&format!("; writing them back failed: {error}"));
            }
            None
        }
        Err(error) => {
            detail.push_str(&format!(
                "; it could not be moved aside ({error}), so saves are refused until the next start"
            ));
            Some(format!(
                "{SETTINGS_FILE} holds a {kind} file that could not be moved aside: {error}"
            ))
        }
    };
    let context = ErrorContext {
        settings_source: Some(source),
        ..ErrorContext::default()
    };
    Loaded {
        file,
        error: Some(AppError::SettingsCorrupt(Box::new(
            Failure::new(detail).with_context(context),
        ))),
        write_refused,
    }
}

/// Writes `file` to settings.json through the store's temp file, flush and rename
/// (REQ-SET-04). A failure is SETTINGS_WRITE_FAILED and the old file stays (REQ-SET-06).
pub fn save(store: &dyn Store, file: &SettingsFile, backup: Backup) -> Result<(), AppError> {
    if let Some(problem) = file.problem() {
        return Err(AppError::internal(format!(
            "settings that fail their check were not written: {problem}"
        )));
    }
    let bytes = serde_json::to_vec_pretty(file)
        .map_err(|error| write_failed(format!("the settings did not serialise: {error}")))?;
    store
        .replace(SETTINGS_FILE, &bytes, backup)
        .map_err(|error| write_failed(format!("{SETTINGS_FILE}: {error}")))
}

fn write_failed(detail: String) -> AppError {
    AppError::SettingsWriteFailed(Box::new(Failure::new(detail)))
}

/// Whether two modes are the same: equal size and equal rationals, never the labels
/// (REQ-DISP-17). The one rule lives in display/modes.rs.
pub fn same_mode(a: &Mode, b: &Mode) -> bool {
    modes::same_mode(modes::to_raw(a), modes::to_raw(b))
}

/// Whether `mode` is stored as confirmed on `monitor` (REQ-DISP-19).
pub fn is_confirmed(confirmed: &[ConfirmedMode], monitor: &MonitorId, mode: &Mode) -> bool {
    confirmed
        .iter()
        .any(|entry| same_monitor(&entry.monitor, monitor) && same_mode(&entry.mode, mode))
}

/// Adds `mode` as confirmed on `monitor` unless it is stored already, by monitor identity,
/// size and refresh rational (REQ-DISP-19). Returns whether it was added.
pub fn add_confirmed(confirmed: &mut Vec<ConfirmedMode>, monitor: &MonitorId, mode: &Mode) -> bool {
    if is_confirmed(confirmed, monitor, mode) {
        return false;
    }
    confirmed.push(ConfirmedMode {
        monitor: monitor.clone(),
        mode: mode.clone(),
    });
    true
}

struct StoreState {
    file: SettingsFile,
    write_refused: Option<String>,
}

/// settings.json in memory and on disk. Memory changes only after the write succeeds, so the
/// two never disagree (REQ-SET-06). Rust only reads and writes it (REQ-SET-01).
pub struct SettingsStore {
    store: Arc<dyn Store>,
    state: Mutex<StoreState>,
}

impl SettingsStore {
    /// Reads settings.json from `folder`, the data folder the caller chose. The error is
    /// SETTINGS_CORRUPT for start-up to raise (REQ-SET-05).
    pub fn open(folder: &Path, clock: &dyn Clock) -> (Self, Option<AppError>) {
        Self::load(Arc::new(DiskStore::new(folder)), clock)
    }

    /// Reads settings.json from `store`, as `load` describes.
    pub fn load(store: Arc<dyn Store>, clock: &dyn Clock) -> (Self, Option<AppError>) {
        let loaded = load(store.as_ref(), clock.epoch_ms());
        let settings = Self {
            store,
            state: Mutex::new(StoreState {
                file: loaded.file,
                write_refused: loaded.write_refused,
            }),
        };
        (settings, loaded.error)
    }

    // The state is replaced only after a write succeeded, so it stays valid after a panic.
    fn state(&self) -> MutexGuard<'_, StoreState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The whole file as memory holds it.
    pub fn file(&self) -> SettingsFile {
        self.state().file.clone()
    }

    /// The settings as memory holds them.
    pub fn settings(&self) -> Settings {
        self.state().file.settings.clone()
    }

    /// The profiles in their saved order.
    pub fn profiles(&self) -> Vec<Profile> {
        self.state().file.profiles.clone()
    }

    /// Changes a copy of the file with `change`, writes it with the last good copy kept, and
    /// only then keeps it in memory. On SETTINGS_WRITE_FAILED memory keeps the last saved
    /// file and the old settings.json stays (REQ-SET-06).
    pub fn update<T>(&self, change: impl FnOnce(&mut SettingsFile) -> T) -> Result<T, AppError> {
        let mut state = self.state();
        if let Some(reason) = &state.write_refused {
            return Err(write_failed(reason.clone()));
        }
        let mut next = state.file.clone();
        let out = change(&mut next);
        save(self.store.as_ref(), &next, Backup::KeepLastGood)?;
        state.file = next;
        Ok(out)
    }

    /// Whether `mode` is confirmed on `monitor` by a stored entry (REQ-DISP-19).
    pub fn is_confirmed(&self, monitor: &MonitorId, mode: &Mode) -> bool {
        is_confirmed(&self.state().file.settings.confirmed_modes, monitor, mode)
    }

    /// Stores `mode` as confirmed on `monitor`, with no duplicate (REQ-DISP-19). Ok(false)
    /// when it was stored already, with nothing written.
    pub fn add_confirmed_mode(&self, monitor: &MonitorId, mode: &Mode) -> Result<bool, AppError> {
        if self.is_confirmed(monitor, mode) {
            return Ok(false);
        }
        self.update(|file| add_confirmed(&mut file.settings.confirmed_modes, monitor, mode))
    }
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::*;
    use crate::display::fake::When;
    use crate::display::modes::to_mode;
    use crate::display::types::ScalingChoice;
    use crate::display::{Rational, RawMode};
    use crate::error::ErrorCode;
    use crate::ipc::types::AutoSwitchState;
    use crate::settings::atomic::WriteStage;
    use crate::settings::atomic::tests::TempFolder;
    use crate::settings::fake::{MemoryStore, StoreCall, StoreFault, StoreFile};
    use crate::system::fake::{FAKE_EPOCH_MS, FakeClock};

    const DEFAULTS: &str = include_str!("../../tests/fixtures/settings-v1-defaults.json");
    const FULL: &str = include_str!("../../tests/fixtures/settings-v1-full.json");

    fn monitor() -> MonitorId {
        MonitorId {
            device_path: String::new(),
            edid_manufacturer_id: Some(0x6D1E),
            edid_product_code: Some(0x5B8E),
            connector_instance: 1,
        }
    }

    fn mode(width: u32, height: u32, numerator: u32, denominator: u32) -> Mode {
        to_mode(RawMode {
            width,
            height,
            refresh: Rational {
                numerator,
                denominator,
            },
        })
    }

    fn stretched() -> Mode {
        mode(1280, 960, 240, 1)
    }

    fn full_file() -> SettingsFile {
        serde_json::from_str(FULL).unwrap()
    }

    fn bytes(file: &SettingsFile) -> Vec<u8> {
        serde_json::to_vec_pretty(file).unwrap()
    }

    fn code(error: Option<&AppError>) -> Option<ErrorCode> {
        error.map(AppError::code)
    }

    fn source(error: Option<&AppError>) -> Option<SettingsSource> {
        error.and_then(|error| error.to_ipc().context.settings_source)
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

    /// The defaults of D-007, held against a fixture: a 10 s revert countdown, update checks
    /// on, Start with Windows off, no hotkeys, no profiles; a 10 s restore delay and a 2 s
    /// watcher interval; auto-switch paused (it is never stored, so every start begins
    /// paused).
    #[test]
    fn req_set_07_defaults_match_the_fixture() {
        let fixture: Value = serde_json::from_str(DEFAULTS).unwrap();
        assert_eq!(
            serde_json::to_value(SettingsFile::default()).unwrap(),
            fixture
        );
        let defaults = Settings::default();
        assert_eq!(defaults.revert_countdown_secs, 10);
        assert!(defaults.check_for_updates);
        assert!(!defaults.start_with_windows);
        assert!(defaults.hotkeys.is_empty());
        assert_eq!(DEFAULT_RESTORE_DELAY_SECS, 10);
        assert_eq!(WATCHER_INTERVAL_MS, 2_000);
        assert!(!AutoSwitchState::default().enabled);
    }

    #[test]
    fn req_set_01_missing_file_gives_the_defaults() {
        let store = MemoryStore::new();
        // A last good copy without settings.json still gives the defaults: deleting
        // settings.json resets the settings.
        store.put(SETTINGS_BACKUP_FILE, &bytes(&full_file()));
        let loaded = load(&store, FAKE_EPOCH_MS);
        assert_eq!(loaded.file, SettingsFile::default());
        assert!(loaded.error.is_none());
        assert!(loaded.write_refused.is_none());
        assert_eq!(
            store.calls(),
            vec![StoreCall::Read(String::from(SETTINGS_FILE))]
        );
    }

    /// The version 1 fixture goes through the whole load: the cap, the JSON, the migrations
    /// (none at version 1), the shape and the checks.
    #[test]
    fn req_set_03_version_1_fixture_loads() {
        let store = MemoryStore::new();
        store.put(SETTINGS_FILE, FULL.as_bytes());
        let loaded = load(&store, FAKE_EPOCH_MS);
        assert!(loaded.error.is_none(), "{:?}", loaded.error);
        let file = loaded.file;
        assert_eq!(file.settings.schema_version, 1);
        assert_eq!(file.profiles.len(), 1);
        assert_eq!(file.profiles[0].exe, "game.exe");
        assert!(is_confirmed(
            &file.settings.confirmed_modes,
            &monitor(),
            &stretched()
        ));
        // Written again, it is the same JSON.
        let fixture: Value = serde_json::from_str(FULL).unwrap();
        assert_eq!(serde_json::to_value(&file).unwrap(), fixture);
    }

    #[test]
    fn req_set_05_damaged_file_falls_back_to_the_last_good_copy() {
        let store = MemoryStore::new();
        store.put(SETTINGS_FILE, b"{ not json");
        store.put(SETTINGS_BACKUP_FILE, &bytes(&full_file()));
        let loaded = load(&store, FAKE_EPOCH_MS);
        assert_eq!(loaded.file, full_file());
        assert_eq!(
            code(loaded.error.as_ref()),
            Some(ErrorCode::SettingsCorrupt)
        );
        assert_eq!(
            source(loaded.error.as_ref()),
            Some(SettingsSource::LastGoodCopy)
        );
        let aside = format!("settings.damaged-{FAKE_EPOCH_MS}.json");
        assert_eq!(store.file(&aside), Some(b"{ not json".to_vec()));
        // The loaded settings are back at settings.json; the last good copy is untouched.
        assert_eq!(store.file(SETTINGS_FILE), Some(bytes(&full_file())));
        assert_eq!(store.file(SETTINGS_BACKUP_FILE), Some(bytes(&full_file())));
        assert!(loaded.write_refused.is_none());
        let detail = loaded
            .error
            .map(|error| error.to_ipc().detail)
            .unwrap_or_default();
        assert!(detail.contains("not valid JSON"), "{detail}");
        assert!(detail.contains(&aside), "{detail}");
        // The next start reads it with nothing to report.
        assert!(load(&store, FAKE_EPOCH_MS + 1).error.is_none());
    }

    #[test]
    fn req_set_05_damaged_file_and_copy_fall_back_to_the_defaults() {
        let store = MemoryStore::new();
        let fixture: Value = serde_json::from_str(FULL).unwrap();
        // An unknown field at any depth is damage (security rule: no unknown fields).
        let unknown = with_unknown_field(&fixture, "/profiles/0/mode");
        store.put(SETTINGS_FILE, unknown.to_string().as_bytes());
        store.put(SETTINGS_BACKUP_FILE, b"[]");
        let loaded = load(&store, FAKE_EPOCH_MS);
        assert_eq!(loaded.file, SettingsFile::default());
        assert_eq!(
            source(loaded.error.as_ref()),
            Some(SettingsSource::Defaults)
        );
        let detail = loaded
            .error
            .map(|error| error.to_ipc().detail)
            .unwrap_or_default();
        assert!(
            detail.contains("does not match the settings shape"),
            "{detail}"
        );
        assert!(detail.contains(SETTINGS_BACKUP_FILE), "{detail}");
        assert_eq!(
            store.file(SETTINGS_FILE),
            Some(bytes(&SettingsFile::default()))
        );
        assert_eq!(store.file(SETTINGS_BACKUP_FILE), Some(b"[]".to_vec()));
    }

    /// Each kind of damage reports SETTINGS_CORRUPT: too large, a missing version, a failed
    /// check, a broken value at a known version.
    #[test]
    fn req_set_05_every_kind_of_damage_is_reported() {
        let mut no_version: Value = serde_json::from_str(FULL).unwrap();
        no_version["settings"]
            .as_object_mut()
            .unwrap()
            .remove("schemaVersion");
        let mut bad_id = full_file();
        bad_id.profiles[0].id = String::from("NOT-A-UUID");
        let mut twice = full_file();
        twice.profiles.push(twice.profiles[0].clone());
        let mut zero_rate = full_file();
        zero_rate.settings.confirmed_modes[0]
            .mode
            .refresh
            .denominator = 0;
        let size = usize::try_from(MAX_FILE_BYTES).unwrap() + 1;
        let cases: Vec<(Vec<u8>, &str)> = vec![
            (vec![b' '; size], "1 MiB cap"),
            (
                no_version.to_string().into_bytes(),
                "schema version is missing",
            ),
            (serde_json::to_vec(&bad_id).unwrap(), "not a lowercase UUID"),
            (
                serde_json::to_vec(&twice).unwrap(),
                "two profiles share an ID",
            ),
            (serde_json::to_vec(&zero_rate).unwrap(), "denominator 0"),
            (
                b"{\"settings\": {\"schemaVersion\": 1}, \"profiles\": []}".to_vec(),
                "settings shape",
            ),
        ];
        for (content, expected) in cases {
            let store = MemoryStore::new();
            store.put(SETTINGS_FILE, &content);
            let loaded = load(&store, FAKE_EPOCH_MS);
            assert_eq!(
                code(loaded.error.as_ref()),
                Some(ErrorCode::SettingsCorrupt)
            );
            let detail = loaded
                .error
                .map(|error| error.to_ipc().detail)
                .unwrap_or_default();
            assert!(detail.contains(expected), "{expected}: {detail}");
            assert_eq!(loaded.file, SettingsFile::default());
        }
    }

    /// A file a newer build wrote is never migrated down and never overwritten: it is kept
    /// as settings.newer-<ms>.json and reported as SETTINGS_CORRUPT, with no new code.
    #[test]
    fn req_set_03_newer_file_is_kept_aside() {
        let store = MemoryStore::new();
        let mut newer: Value = serde_json::from_str(FULL).unwrap();
        newer["settings"]["schemaVersion"] = json!(2);
        newer["laterField"] = json!(true);
        store.put(SETTINGS_FILE, newer.to_string().as_bytes());
        let loaded = load(&store, FAKE_EPOCH_MS);
        assert_eq!(
            source(loaded.error.as_ref()),
            Some(SettingsSource::Defaults)
        );
        let aside = format!("settings.newer-{FAKE_EPOCH_MS}.json");
        assert_eq!(store.file(&aside), Some(newer.to_string().into_bytes()));
        let detail = loaded
            .error
            .map(|error| error.to_ipc().detail)
            .unwrap_or_default();
        assert!(detail.contains("schema version 2 is newer"), "{detail}");
    }

    /// When the damaged file cannot be moved aside, nothing overwrites it: saves are refused
    /// with SETTINGS_WRITE_FAILED, and the last good copy is never replaced by it.
    #[test]
    fn req_set_05_file_that_cannot_move_aside_is_never_overwritten() {
        let store = Arc::new(MemoryStore::new());
        store.put(SETTINGS_FILE, b"damaged");
        store.put(SETTINGS_BACKUP_FILE, &bytes(&full_file()));
        store.script(StoreFault::MoveAside(StoreFile::Settings), When::Once);
        let (settings, error) = SettingsStore::load(store.clone(), &FakeClock::new());
        assert_eq!(source(error.as_ref()), Some(SettingsSource::LastGoodCopy));
        assert_eq!(settings.file(), full_file());
        let refused = settings
            .update(|file| file.settings.check_for_updates = true)
            .unwrap_err();
        assert_eq!(refused.code(), ErrorCode::SettingsWriteFailed);
        assert_eq!(store.file(SETTINGS_FILE), Some(b"damaged".to_vec()));
        assert_eq!(store.file(SETTINGS_BACKUP_FILE), Some(bytes(&full_file())));
        assert!(!settings.settings().check_for_updates);
        assert!(
            !store
                .calls()
                .iter()
                .any(|call| matches!(call, StoreCall::Replace { .. }))
        );
    }

    #[test]
    fn req_set_04_save_keeps_the_last_good_copy() {
        let store = Arc::new(MemoryStore::new());
        let (settings, error) = SettingsStore::load(store.clone(), &FakeClock::new());
        assert!(error.is_none());
        settings
            .update(|file| file.settings.show_hidden_monitors = true)
            .unwrap();
        let first = store.file(SETTINGS_FILE).unwrap();
        assert_eq!(store.file(SETTINGS_BACKUP_FILE), None);
        settings
            .update(|file| file.settings.revert_countdown_secs = 15)
            .unwrap();
        assert_eq!(store.file(SETTINGS_BACKUP_FILE), Some(first));
        assert_eq!(
            store.calls().last(),
            Some(&StoreCall::Replace {
                name: String::from(SETTINGS_FILE),
                backup: Backup::KeepLastGood,
            })
        );
        // What was written reads back as what memory holds.
        let reloaded = load(store.as_ref(), FAKE_EPOCH_MS);
        assert_eq!(reloaded.file, settings.file());
        assert_eq!(reloaded.file.settings.revert_countdown_secs, 15);
    }

    /// Each write stage fails in turn: SETTINGS_WRITE_FAILED, the old file unchanged, memory
    /// keeping the last saved settings.
    #[test]
    fn req_set_06_failed_save_keeps_file_and_memory() {
        for stage in [
            WriteStage::TempWrite,
            WriteStage::Sync,
            WriteStage::BackupCopy,
            WriteStage::Rename,
        ] {
            let store = Arc::new(MemoryStore::new());
            store.put(SETTINGS_FILE, FULL.as_bytes());
            let (settings, _) = SettingsStore::load(store.clone(), &FakeClock::new());
            store.script(StoreFault::Write(StoreFile::Settings, stage), When::Once);
            let error = settings
                .update(|file| file.settings.check_for_updates = true)
                .unwrap_err();
            assert_eq!(error.code(), ErrorCode::SettingsWriteFailed, "{stage:?}");
            assert!(error.to_ipc().detail.contains(stage.label()), "{stage:?}");
            assert_eq!(store.file(SETTINGS_FILE), Some(FULL.as_bytes().to_vec()));
            assert!(!settings.settings().check_for_updates, "{stage:?}");
        }
    }

    /// A change that fails the checks is never written.
    #[test]
    fn req_set_06_invalid_settings_are_not_written() {
        let store = Arc::new(MemoryStore::new());
        let (settings, _) = SettingsStore::load(store.clone(), &FakeClock::new());
        let error = settings
            .update(|file| file.settings.play_stretched_profile = Some(String::from("x")))
            .unwrap_err();
        assert_eq!(error.code(), ErrorCode::Internal);
        assert_eq!(store.file(SETTINGS_FILE), None);
        assert_eq!(settings.settings().play_stretched_profile, None);
    }

    #[test]
    fn req_set_04_disk_round_trip() {
        let folder = TempFolder::new("settings");
        let clock = FakeClock::new();
        let (settings, error) = SettingsStore::open(folder.path(), &clock);
        assert!(error.is_none());
        settings
            .update(|file| {
                file.settings.selected_monitor = Some(monitor());
                file.profiles = full_file().profiles;
            })
            .unwrap();
        settings
            .add_confirmed_mode(&monitor(), &stretched())
            .unwrap();
        let (again, error) = SettingsStore::open(folder.path(), &clock);
        assert!(error.is_none());
        assert_eq!(again.file(), settings.file());
        assert!(folder.file(SETTINGS_BACKUP_FILE).is_some());
        assert!(folder.file("settings.json.tmp").is_none());
    }

    #[test]
    fn req_disp_19_confirmed_modes_have_no_duplicates() {
        let store = Arc::new(MemoryStore::new());
        let (settings, _) = SettingsStore::load(store.clone(), &FakeClock::new());
        let lg = monitor();
        assert!(!settings.is_confirmed(&lg, &stretched()));
        assert_eq!(
            settings.add_confirmed_mode(&lg, &stretched()).ok(),
            Some(true)
        );
        let writes = store.calls().len();
        // The same size and rational: no second entry and no write.
        assert_eq!(
            settings.add_confirmed_mode(&lg, &stretched()).ok(),
            Some(false)
        );
        assert_eq!(store.calls().len(), writes);
        // Equal rationals with other numbers are the same rate.
        assert_eq!(
            settings
                .add_confirmed_mode(&lg, &mode(1280, 960, 480, 2))
                .ok(),
            Some(false)
        );
        // 59.94 Hz is not 60 Hz.
        assert_eq!(
            settings
                .add_confirmed_mode(&lg, &mode(1280, 960, 60_000, 1001))
                .ok(),
            Some(true)
        );
        assert!(!settings.is_confirmed(&lg, &mode(1280, 960, 60, 1)));
        assert!(settings.is_confirmed(&lg, &mode(1280, 960, 120_000, 2002)));
        // Another size, and another monitor, are separate entries.
        assert_eq!(
            settings
                .add_confirmed_mode(&lg, &mode(1440, 1080, 240, 1))
                .ok(),
            Some(true)
        );
        let samsung = MonitorId {
            device_path: String::from("fake-device-path:test-pc:samsung-lf24t35"),
            edid_manufacturer_id: None,
            edid_product_code: None,
            connector_instance: 1,
        };
        assert!(!settings.is_confirmed(&samsung, &stretched()));
        assert_eq!(
            settings.add_confirmed_mode(&samsung, &stretched()).ok(),
            Some(true)
        );
        assert_eq!(settings.settings().confirmed_modes.len(), 4);
        // The LG on another port: a new device path, the same EDID ids and connector.
        let moved = MonitorId {
            device_path: String::from("fake-device-path:test-pc:lg-other-port"),
            ..monitor()
        };
        assert!(settings.is_confirmed(&moved, &stretched()));
        let other_connector = MonitorId {
            connector_instance: monitor().connector_instance + 1,
            ..moved.clone()
        };
        assert!(!settings.is_confirmed(&other_connector, &stretched()));
        // Stored on disk too.
        let reloaded = load(store.as_ref(), FAKE_EPOCH_MS);
        assert_eq!(reloaded.file.settings.confirmed_modes.len(), 4);
    }

    /// confirm_mode while settings.json cannot be written (INJ-D16): the error is
    /// SETTINGS_WRITE_FAILED and the mode is not stored, so it asks again next time.
    #[test]
    fn req_disp_19_inj_d16_failed_write_stores_nothing() {
        let store = Arc::new(MemoryStore::new());
        let (settings, _) = SettingsStore::load(store.clone(), &FakeClock::new());
        store.script(
            StoreFault::Write(StoreFile::Settings, WriteStage::Rename),
            When::Once,
        );
        let error = settings
            .add_confirmed_mode(&monitor(), &stretched())
            .unwrap_err();
        assert_eq!(error.code(), ErrorCode::SettingsWriteFailed);
        assert!(!settings.is_confirmed(&monitor(), &stretched()));
        assert_eq!(
            settings.add_confirmed_mode(&monitor(), &stretched()).ok(),
            Some(true)
        );
    }

    #[test]
    fn req_set_01_input_copies_the_five_fields() {
        let mut settings = Settings::default();
        settings.apply_input(SettingsInput {
            revert_countdown_secs: 20,
            check_for_updates: false,
            show_hidden_monitors: true,
            selected_monitor: Some(monitor()),
            play_stretched_profile: Some(String::from("0f8fad5b-d9cb-469f-a165-70867728950e")),
        });
        assert_eq!(settings.revert_countdown_secs, 20);
        assert!(!settings.check_for_updates);
        assert!(settings.show_hidden_monitors);
        assert_eq!(settings.selected_monitor, Some(monitor()));
        assert!(settings.play_stretched_profile.is_some());
        // Fields save_settings never touches stay.
        assert_eq!(settings.schema_version, SCHEMA_VERSION);
        assert!(settings.hotkeys.is_empty());
    }
}
