//! The reg.exe arguments for the NV_Modes write: `add <key> /v NV_Modes /t <type> /d "<data>"
//! /f`, built only from validated values (REQ-SEC-01, REQ-NV-15, REQ-NV-16).

#![forbid(unsafe_code)]

use crate::elevation::Token;
use crate::vendor::nvidia::nv_modes::{NvModesData, NvModesKind, NvModesValue};
use crate::vendor::{AdapterKey, DISPLAY_CLASS};

/// The fixed part of every key path reg.exe writes; four digits from the adapter's own
/// driver property follow it.
const CLASS_KEY_PREFIX: &str = r"HKLM\SYSTEM\CurrentControlSet\Control\Class";

/// The only value name ResSwap writes.
const VALUE_NAME: &str = "NV_Modes";

/// The arguments of one reg.exe add: the NVIDIA adapter's class key, the type as found and
/// the checked text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RegAddArgs {
    key: AdapterKey,
    kind: NvModesKind,
    data: NvModesData,
}

impl RegAddArgs {
    /// Writes `value` under the display class subkey `key`. Both come from ResSwap's own
    /// enumeration and the NV_Modes checks, so no text from IPC or settings reaches reg.exe.
    pub fn new(key: &AdapterKey, value: &NvModesValue) -> Self {
        Self {
            key: key.clone(),
            kind: value.kind,
            data: value.data.clone(),
        }
    }

    /// The display class subkey this write goes to.
    pub fn key(&self) -> &AdapterKey {
        &self.key
    }

    /// The registry type this write keeps.
    pub fn kind(&self) -> NvModesKind {
        self.kind
    }

    /// The text this write sets.
    pub fn data(&self) -> &NvModesData {
        &self.data
    }

    /// The full key path, for example
    /// `HKLM\SYSTEM\CurrentControlSet\Control\Class\{4d36e968-e325-11ce-bfc1-08002be10318}\0001`.
    pub fn key_path(&self) -> String {
        format!(r"{CLASS_KEY_PREFIX}\{DISPLAY_CLASS}\{}", self.key.as_str())
    }

    /// The tokens in order. Only the switches start with a slash: the data cannot, because
    /// its allow-list has no slash or hyphen.
    pub fn tokens(&self) -> Vec<Token> {
        vec![
            Token::Fixed("add"),
            Token::Value(self.key_path()),
            Token::Fixed("/v"),
            Token::Fixed(VALUE_NAME),
            Token::Fixed("/t"),
            Token::Fixed(self.kind.reg_type()),
            Token::Fixed("/d"),
            Token::Value(self.data.as_str().to_owned()),
            Token::Fixed("/f"),
        ]
    }
}
