//! NV_Modes: the registry reader behind a trait, the shape check, and the checked text that
//! may reach reg.exe (REQ-NV-13 to REQ-NV-18). The parser and the serialiser join it with
//! the NVIDIA work.

use crate::error::{AppError, Failure};
use crate::vendor::AdapterKey;

/// A registry value's type as the reader found it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RawKind {
    MultiSz,
    Sz,
    /// Any other type, by its REG_* number.
    Other(u32),
}

/// A registry value as read: its type and its strings, without the NULs between them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RawValue {
    pub kind: RawKind,
    pub strings: Vec<String>,
}

/// The two NV_Modes types ResSwap edits, kept as found when it writes the value back
/// (REQ-NV-16).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NvModesKind {
    MultiSz,
    Sz,
}

impl NvModesKind {
    /// The type name reg.exe takes after /t.
    pub const fn reg_type(self) -> &'static str {
        match self {
            Self::MultiSz => "REG_MULTI_SZ",
            Self::Sz => "REG_SZ",
        }
    }
}

/// NV_Modes text that passed the allow-list: ASCII letters and digits, the space and
/// `{ } * , = ;`. So it holds no quote, backslash, slash, hyphen, percent sign, control
/// character or non-ASCII character, and can never close a quoted argument or start a
/// switch (REQ-SEC-01). Only the vendor module builds one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NvModesData(String);

impl NvModesData {
    /// `None` for empty text or any character outside the allow-list.
    pub(in crate::vendor) fn checked(text: &str) -> Option<Self> {
        let allowed = |byte: u8| {
            byte.is_ascii_alphanumeric()
                || matches!(byte, b' ' | b'{' | b'}' | b'*' | b',' | b'=' | b';')
        };
        (!text.is_empty() && text.bytes().all(allowed)).then(|| Self(text.to_owned()))
    }

    /// The text as it will be written.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// NV_Modes as read and checked: its type and its one string.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NvModesValue {
    pub kind: NvModesKind,
    pub data: NvModesData,
}

/// The shape check that runs before anything else reads NV_Modes: REG_MULTI_SZ with exactly
/// one string, or REG_SZ, inside the allow-list. Anything else is NV_MODES_PARSE_FAILED and
/// nothing is written (REQ-NV-14).
pub(crate) fn check_nv_modes(raw: &RawValue) -> Result<NvModesValue, AppError> {
    let kind = match raw.kind {
        RawKind::MultiSz => NvModesKind::MultiSz,
        RawKind::Sz => NvModesKind::Sz,
        RawKind::Other(number) => {
            return Err(parse_failed(format!("NV_Modes has registry type {number}")));
        }
    };
    let [text] = raw.strings.as_slice() else {
        return Err(parse_failed(format!(
            "NV_Modes holds {} strings, not one",
            raw.strings.len()
        )));
    };
    let data = NvModesData::checked(text).ok_or_else(|| {
        parse_failed("NV_Modes is empty or holds a character outside the allow-list")
    })?;
    Ok(NvModesValue { kind, data })
}

fn parse_failed(detail: impl Into<String>) -> AppError {
    AppError::NvModesParseFailed(Box::new(Failure::new(detail)))
}

/// Every registry read the NVIDIA extras make, as the standard user. Reads only: the write
/// goes through reg.exe (REQ-NV-18).
pub trait RegistryReader: Send + Sync {
    /// NV_Modes under the display class subkey `key`, shape-checked; `None` when that key has
    /// no NV_Modes value. Changes nothing.
    fn nv_modes(&self, key: &AdapterKey) -> Result<Option<NvModesValue>, AppError>;

    /// MatchingDeviceId under the display class subkey `key`, the cross-check that the key
    /// belongs to an NVIDIA adapter. Changes nothing.
    fn matching_device_id(&self, key: &AdapterKey) -> Result<Option<String>, AppError>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::ErrorCode;

    fn raw(kind: RawKind, strings: &[&str]) -> RawValue {
        RawValue {
            kind,
            strings: strings.iter().map(|text| (*text).to_owned()).collect(),
        }
    }

    #[test]
    fn req_nv_14_shape_check() {
        let value = check_nv_modes(&raw(RawKind::MultiSz, &["{*}S 640x480x32,64=1;"])).unwrap();
        assert_eq!(value.kind, NvModesKind::MultiSz);
        assert_eq!(value.kind.reg_type(), "REG_MULTI_SZ");
        assert_eq!(value.data.as_str(), "{*}S 640x480x32,64=1;");
        let single = check_nv_modes(&raw(RawKind::Sz, &["{*}S 640x480x32,64=1;"])).unwrap();
        assert_eq!(single.kind.reg_type(), "REG_SZ");

        for refused in [
            raw(RawKind::Other(3), &["{*}S 640x480x32,64=1;"]),
            raw(RawKind::MultiSz, &[]),
            raw(
                RawKind::MultiSz,
                &["{*}S 640x480x32,64=1;", "{*}S 800x600x32=1;"],
            ),
            raw(RawKind::MultiSz, &[""]),
        ] {
            let error = check_nv_modes(&refused).unwrap_err();
            assert_eq!(error.code(), ErrorCode::NvModesParseFailed, "{refused:?}");
        }
    }

    // Also INJ-E04.
    #[test]
    fn req_sec_01_inj_e03_nv_modes_text_refuses_hostile_characters() {
        for refused in [
            "{*}S 640x480x32,64=1;\" /f",
            "{*}S 640x480x32,64=1;\\",
            "{*}S 640x480x32,64=1; /d",
            "{*}S 640x480x32,64=1; -f",
            "{*}S %PATH%",
            "{*}S 640x480\n",
            "{*}S 640x480\u{0}",
            "{*}S 640x480 \u{e9}",
            "{*}S 640x480 & echo",
        ] {
            assert!(NvModesData::checked(refused).is_none(), "{refused:?}");
        }
    }
}
