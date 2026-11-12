//! The pnputil.exe arguments for the graphics driver restart: `/restart-device <instance ID>`,
//! never `/reboot`, so ResSwap never restarts Windows (REQ-SEC-01, REQ-NV-20, REQ-NV-21).

#![forbid(unsafe_code)]

use crate::elevation::Token;
use crate::vendor::InstanceId;

/// The arguments of one driver restart: the NVIDIA adapter's instance ID.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RestartDeviceArgs {
    instance_id: InstanceId,
}

impl RestartDeviceArgs {
    /// Restarts the device with `instance_id`, read from ResSwap's own enumeration.
    pub fn new(instance_id: &InstanceId) -> Self {
        Self {
            instance_id: instance_id.clone(),
        }
    }

    /// The device this restart targets.
    pub fn instance_id(&self) -> &InstanceId {
        &self.instance_id
    }

    /// The tokens in order. The ID holds no space, so it is passed without quotes.
    pub fn tokens(&self) -> Vec<Token> {
        vec![
            Token::Fixed("/restart-device"),
            Token::Value(self.instance_id.as_str().to_owned()),
        ]
    }
}
