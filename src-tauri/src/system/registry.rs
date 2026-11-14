//! Registry reads as the standard user. Reads only: every registry write goes through
//! reg.exe (REQ-NV-18). The display class subkeys are listed fresh on every call, so a
//! caller finds the NVIDIA adapter's key by enumeration and never from a stored number
//! (REQ-NV-15).

use windows::Win32::Foundation::{
    ERROR_FILE_NOT_FOUND, ERROR_MORE_DATA, ERROR_NO_MORE_ITEMS, ERROR_SUCCESS, WIN32_ERROR,
};
use windows::Win32::System::Registry::{
    HKEY, HKEY_LOCAL_MACHINE, KEY_READ, REG_EXPAND_SZ, REG_MULTI_SZ, REG_SZ, REG_VALUE_TYPE,
    RegCloseKey, RegEnumKeyExW, RegOpenKeyExW, RegQueryValueExW,
};
use windows::core::{HSTRING, PWSTR};

use crate::error::{AppError, Failure};
use crate::vendor::nvidia::nv_modes::{
    NvModesValue, RawKind, RawValue, RegistryReader, check_nv_modes,
};
use crate::vendor::{AdapterKey, DISPLAY_CLASS, NVIDIA_INSTANCE_PREFIX};

/// The display class key under HKEY_LOCAL_MACHINE, without the subkey number.
const CLASS_PATH: &str = r"SYSTEM\CurrentControlSet\Control\Class";

/// The longest key name the registry allows is 255 characters (Registry element size
/// limits), plus the terminating NUL.
const KEY_NAME_CAPACITY: usize = 256;

/// How often a value read starts again when the value grew between the size query and the
/// read (ERROR_MORE_DATA).
const READ_ATTEMPTS: usize = 4;

/// Owns one open registry key and closes it exactly once.
pub(crate) struct OwnedKey(HKEY);

impl OwnedKey {
    /// Opens `path` under `root` for reading only.
    pub(crate) fn open_read(root: HKEY, path: &str) -> Result<Self, WIN32_ERROR> {
        let path = HSTRING::from(path);
        let mut key = HKEY::default();
        // SAFETY: `path` is a NUL-terminated wide string that lives until the call returns;
        // `key` is a valid out pointer on this stack frame; KEY_READ asks for no write access.
        let status = unsafe { RegOpenKeyExW(root, &path, None, KEY_READ, &mut key) };
        if status != ERROR_SUCCESS {
            return Err(status);
        }
        Ok(Self(key))
    }

    /// Lends the key for one call. Callers never store it and never close it.
    pub(crate) fn raw(&self) -> HKEY {
        self.0
    }
}

impl Drop for OwnedKey {
    fn drop(&mut self) {
        // SAFETY: self.0 was opened by RegOpenKeyExW in open_read, is owned by this value
        // alone and is closed nowhere else.
        let status = unsafe { RegCloseKey(self.0) };
        if status != ERROR_SUCCESS {
            log::warn!("RegCloseKey failed: {}", status.0);
        }
    }
}

/// One display class subkey as listed: its four digits and the values that tell which
/// adapter it belongs to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DisplayClassKey {
    /// The four digits, for example "0001".
    pub key: String,
    /// MatchingDeviceId, for example one that starts with `pci\ven_10de&`.
    pub matching_device_id: Option<String>,
    /// DriverDesc, the adapter's name as its driver wrote it; for messages only, never to
    /// pick a key.
    pub driver_desc: Option<String>,
    /// The Win32 error when the subkey could not be opened; the values are then `None`.
    pub open_error: Option<u32>,
}

impl DisplayClassKey {
    /// Whether MatchingDeviceId names an NVIDIA PCI device (vendor 10DE), compared without
    /// regard to case.
    pub fn is_nvidia(&self) -> bool {
        self.matching_device_id
            .as_deref()
            .is_some_and(|id| starts_with_ignore_case(id, NVIDIA_INSTANCE_PREFIX))
    }
}

/// Every display class subkey with four digits, in the order the registry lists them, each
/// read fresh. A subkey that cannot be opened is listed with its error, so one unreadable
/// key never hides the others. Changes nothing.
pub fn display_class_keys() -> Result<Vec<DisplayClassKey>, AppError> {
    let class_path = format!(r"{CLASS_PATH}\{DISPLAY_CLASS}");
    let class = OwnedKey::open_read(HKEY_LOCAL_MACHINE, &class_path)
        .map_err(|status| read_failed("RegOpenKeyExW", &class_path, status))?;
    let mut keys = Vec::new();
    for name in subkey_names(&class, &class_path)? {
        if !is_class_key_name(&name) {
            continue;
        }
        keys.push(read_class_key(&name));
    }
    Ok(keys)
}

/// One value under the display class subkey with these four digits, with its type and its
/// strings; `None` when the subkey has no such value. Changes nothing.
pub fn read_class_value(key: &str, value: &str) -> Result<Option<RawValue>, AppError> {
    if !is_class_key_name(key) {
        return Err(AppError::RegistryReadFailed(Box::new(Failure::new(
            format!("refused a display class subkey name that is not four digits: {key:?}"),
        ))));
    }
    let path = class_key_path(key);
    let subkey = OwnedKey::open_read(HKEY_LOCAL_MACHINE, &path)
        .map_err(|status| read_failed("RegOpenKeyExW", &path, status))?;
    read_value(&subkey, value)
        .map_err(|status| read_failed("RegQueryValueExW", &format!(r"{path}\{value}"), status))
}

/// The registry reader on the player's PC: RegOpenKeyExW with KEY_READ and RegQueryValueExW,
/// REG_MULTI_SZ and REG_SZ kept apart. It never writes.
#[derive(Debug, Default)]
pub struct Win32Registry;

impl RegistryReader for Win32Registry {
    fn nv_modes(&self, key: &AdapterKey) -> Result<Option<NvModesValue>, AppError> {
        read_class_value(key.as_str(), "NV_Modes")?
            .map(|raw| check_nv_modes(&raw))
            .transpose()
    }

    fn matching_device_id(&self, key: &AdapterKey) -> Result<Option<String>, AppError> {
        let raw = read_class_value(key.as_str(), "MatchingDeviceId")?;
        Ok(raw.and_then(|raw| text_of(&raw)))
    }
}

/// The string of a REG_SZ value, or of a REG_MULTI_SZ that holds exactly one string.
fn text_of(raw: &RawValue) -> Option<String> {
    match (raw.kind, raw.strings.as_slice()) {
        (RawKind::Sz | RawKind::MultiSz, [text]) => Some(text.clone()),
        _ => None,
    }
}

fn read_class_key(name: &str) -> DisplayClassKey {
    let path = class_key_path(name);
    let subkey = match OwnedKey::open_read(HKEY_LOCAL_MACHINE, &path) {
        Ok(subkey) => subkey,
        Err(status) => {
            log::warn!("RegOpenKeyExW on {path} returned {}", status.0);
            return DisplayClassKey {
                key: name.to_owned(),
                matching_device_id: None,
                driver_desc: None,
                open_error: Some(status.0),
            };
        }
    };
    let text = |value: &str| match read_value(&subkey, value) {
        Ok(raw) => raw.and_then(|raw| text_of(&raw)),
        Err(status) => {
            log::warn!("RegQueryValueExW on {path}\\{value} returned {}", status.0);
            None
        }
    };
    DisplayClassKey {
        key: name.to_owned(),
        matching_device_id: text("MatchingDeviceId"),
        driver_desc: text("DriverDesc"),
        open_error: None,
    }
}

fn class_key_path(key: &str) -> String {
    format!(r"{CLASS_PATH}\{DISPLAY_CLASS}\{key}")
}

/// The names of every direct subkey of `key`, as RegEnumKeyExW lists them.
fn subkey_names(key: &OwnedKey, path: &str) -> Result<Vec<String>, AppError> {
    let mut names = Vec::new();
    for index in 0u32.. {
        let mut buffer = [0u16; KEY_NAME_CAPACITY];
        let mut length = KEY_NAME_CAPACITY as u32;
        // SAFETY: `buffer` holds KEY_NAME_CAPACITY wide characters and `length` says so, so
        // the call writes inside it; `length` is a valid in and out pointer on this frame;
        // the class, its length and the write time are not asked for (None).
        let status = unsafe {
            RegEnumKeyExW(
                key.raw(),
                index,
                Some(PWSTR(buffer.as_mut_ptr())),
                &mut length,
                None,
                None,
                None,
                None,
            )
        };
        if status == ERROR_NO_MORE_ITEMS {
            break;
        }
        if status != ERROR_SUCCESS {
            return Err(read_failed("RegEnumKeyExW", path, status));
        }
        let written = buffer.get(..length as usize).unwrap_or(&buffer);
        names.push(decode_sz(written));
    }
    Ok(names)
}

/// Reads one value with its type. `Ok(None)` when the value does not exist.
fn read_value(key: &OwnedKey, name: &str) -> Result<Option<RawValue>, WIN32_ERROR> {
    let name = HSTRING::from(name);
    for _ in 0..READ_ATTEMPTS {
        let mut kind = REG_VALUE_TYPE::default();
        let mut size = 0u32;
        // SAFETY: `name` is a NUL-terminated wide string that lives until the call returns;
        // `kind` and `size` are valid out pointers on this frame; no data buffer is passed, so
        // the call only reports the type and the size in bytes.
        let status = unsafe {
            RegQueryValueExW(
                key.raw(),
                &name,
                None,
                Some(&mut kind),
                None,
                Some(&mut size),
            )
        };
        if status == ERROR_FILE_NOT_FOUND {
            return Ok(None);
        }
        if status != ERROR_SUCCESS {
            return Err(status);
        }
        // One spare wide character, so a string the writer left without its NUL still ends.
        let mut buffer = vec![0u16; (size as usize).div_ceil(2) + 1];
        let mut written = size;
        // SAFETY: `buffer` holds at least `written` bytes (it was sized from `size` rounded
        // up to whole wide characters, plus one), is aligned for u16 data and lives until the
        // call returns; `kind` and `written` are valid in and out pointers on this frame.
        let status = unsafe {
            RegQueryValueExW(
                key.raw(),
                &name,
                None,
                Some(&mut kind),
                Some(buffer.as_mut_ptr().cast::<u8>()),
                Some(&mut written),
            )
        };
        if status == ERROR_MORE_DATA {
            continue;
        }
        if status == ERROR_FILE_NOT_FOUND {
            return Ok(None);
        }
        if status != ERROR_SUCCESS {
            return Err(status);
        }
        let units = buffer
            .get(..(written as usize).div_ceil(2))
            .unwrap_or(&buffer);
        return Ok(Some(decode_value(kind, units)));
    }
    Err(ERROR_MORE_DATA)
}

/// The value as the NV_Modes check reads it: REG_MULTI_SZ and REG_SZ kept apart; any other
/// type by its number, with REG_EXPAND_SZ text kept unexpanded and other data left out.
fn decode_value(kind: REG_VALUE_TYPE, units: &[u16]) -> RawValue {
    if kind == REG_MULTI_SZ {
        RawValue {
            kind: RawKind::MultiSz,
            strings: decode_multi_sz(units),
        }
    } else if kind == REG_SZ {
        RawValue {
            kind: RawKind::Sz,
            strings: vec![decode_sz(units)],
        }
    } else if kind == REG_EXPAND_SZ {
        RawValue {
            kind: RawKind::Other(kind.0),
            strings: vec![decode_sz(units)],
        }
    } else {
        RawValue {
            kind: RawKind::Other(kind.0),
            strings: Vec::new(),
        }
    }
}

/// The text up to the first NUL, or all of it when there is none.
fn decode_sz(units: &[u16]) -> String {
    let end = units
        .iter()
        .position(|&unit| unit == 0)
        .unwrap_or(units.len());
    String::from_utf16_lossy(units.get(..end).unwrap_or(units))
}

/// The strings of a REG_MULTI_SZ: each ends with a NUL and an empty string ends the list
/// (Registry value types). Data the writer left without its final NULs still ends at the
/// last character.
fn decode_multi_sz(units: &[u16]) -> Vec<String> {
    let mut strings = Vec::new();
    for part in units.split(|&unit| unit == 0) {
        if part.is_empty() {
            break;
        }
        strings.push(String::from_utf16_lossy(part));
    }
    strings
}

/// Exactly four ASCII digits.
fn is_class_key_name(name: &str) -> bool {
    name.len() == 4 && name.bytes().all(|byte| byte.is_ascii_digit())
}

fn starts_with_ignore_case(text: &str, prefix: &str) -> bool {
    text.get(..prefix.len())
        .is_some_and(|head| head.eq_ignore_ascii_case(prefix))
}

fn read_failed(call: &str, path: &str, status: WIN32_ERROR) -> AppError {
    AppError::RegistryReadFailed(Box::new(Failure::new(format!(
        r"{call} on HKLM\{path} returned {}",
        status.0
    ))))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::ErrorCode;

    fn wide(text: &str) -> Vec<u16> {
        text.encode_utf16().collect()
    }

    #[test]
    fn req_nv_13_multi_sz_keeps_every_string_and_drops_the_nuls() {
        let mut units = wide("{*}S 640x480x32,64=1;");
        units.push(0);
        units.extend(wide("second"));
        units.extend([0, 0]);
        assert_eq!(
            decode_multi_sz(&units),
            vec![
                String::from("{*}S 640x480x32,64=1;"),
                String::from("second")
            ]
        );
        // A writer that left out the final NULs still gives the whole string.
        assert_eq!(decode_multi_sz(&wide("only")), vec![String::from("only")]);
        // An empty REG_MULTI_SZ holds no string.
        assert!(decode_multi_sz(&[0, 0]).is_empty());
        assert!(decode_multi_sz(&[]).is_empty());
    }

    #[test]
    fn req_nv_16_value_types_stay_apart() {
        let mut units = wide("{*}S 640x480x32,64=1;");
        units.extend([0, 0]);
        let multi = decode_value(REG_MULTI_SZ, &units);
        assert_eq!(multi.kind, RawKind::MultiSz);
        assert_eq!(multi.strings, vec![String::from("{*}S 640x480x32,64=1;")]);

        let single = decode_value(REG_SZ, &units);
        assert_eq!(single.kind, RawKind::Sz);
        assert_eq!(single.strings, vec![String::from("{*}S 640x480x32,64=1;")]);

        // REG_DWORD is type 4: kept as its number, with no strings, so the NV_Modes check
        // refuses it.
        let dword = decode_value(REG_VALUE_TYPE(4), &[1, 0]);
        assert_eq!(dword.kind, RawKind::Other(4));
        assert!(dword.strings.is_empty());
        let refused = check_nv_modes(&dword).unwrap_err();
        assert_eq!(refused.code(), ErrorCode::NvModesParseFailed);
    }

    #[test]
    fn req_nv_15_class_key_names_are_four_digits() {
        for accepted in ["0000", "0001", "0123"] {
            assert!(is_class_key_name(accepted), "{accepted}");
        }
        for refused in [
            "Configuration",
            "Properties",
            "001",
            "00001",
            "00a1",
            r"..\0",
            "",
        ] {
            assert!(!is_class_key_name(refused), "{refused}");
        }
        let refused = read_class_value(r"..\x", "NV_Modes").unwrap_err();
        assert_eq!(refused.code(), ErrorCode::RegistryReadFailed);
    }

    #[test]
    fn req_nv_15_nvidia_key_from_matching_device_id() {
        let key = |id: Option<&str>| DisplayClassKey {
            key: String::from("0001"),
            matching_device_id: id.map(str::to_owned),
            driver_desc: None,
            open_error: None,
        };
        assert!(key(Some(r"pci\ven_10de&dev_2803")).is_nvidia());
        assert!(key(Some(r"PCI\VEN_10DE&DEV_2803&SUBSYS_0")).is_nvidia());
        assert!(!key(Some(r"root\display")).is_nvidia());
        assert!(!key(Some(r"pci\ven_1002&dev_73bf")).is_nvidia());
        assert!(!key(None).is_nvidia());
    }

    #[test]
    fn req_nv_13_text_of_a_value() {
        let raw = |kind, strings: &[&str]| RawValue {
            kind,
            strings: strings.iter().map(|text| (*text).to_owned()).collect(),
        };
        assert_eq!(
            text_of(&raw(RawKind::Sz, &[r"pci\ven_10de&dev_2803"])),
            Some(String::from(r"pci\ven_10de&dev_2803"))
        );
        assert_eq!(text_of(&raw(RawKind::MultiSz, &["a", "b"])), None);
        assert_eq!(text_of(&raw(RawKind::Other(4), &[])), None);
    }

    // Reads HKLM on the PC this runs on; changes nothing.
    #[test]
    #[ignore = "reads the real PC; run by name"]
    fn req_nv_15_list_display_class_keys_on_this_pc() {
        let keys = display_class_keys().unwrap();
        assert!(!keys.is_empty());
    }
}
