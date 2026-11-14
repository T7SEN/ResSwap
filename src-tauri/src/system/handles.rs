//! Owners that close Windows handles when they go out of scope, so no raw handle leaves the
//! module that opened it.

use windows::Win32::Devices::DeviceAndDriverInstallation::{
    HDEVINFO, SetupDiCreateDeviceInfoList, SetupDiDestroyDeviceInfoList,
};

/// Owns one SetupAPI device information set and destroys it exactly once.
pub(crate) struct DeviceInfoList(HDEVINFO);

impl DeviceInfoList {
    /// An empty device information set with no class, which SetupDiOpenDeviceInterfaceW then
    /// fills with one device (SetupDiCreateDeviceInfoList).
    pub(crate) fn create_empty() -> windows::core::Result<Self> {
        // SAFETY: SetupDiCreateDeviceInfoList takes no pointer here: both optional parameters
        // are None, so it reads no caller memory. It returns an error rather than an invalid
        // set, and the set it returns is owned by this value alone.
        let list = unsafe { SetupDiCreateDeviceInfoList(None, None) }?;
        Ok(Self(list))
    }

    /// Lends the set for one call. Callers never store it and never destroy it.
    pub(crate) fn raw(&self) -> HDEVINFO {
        self.0
    }
}

impl Drop for DeviceInfoList {
    fn drop(&mut self) {
        if self.0.is_invalid() {
            return;
        }
        // SAFETY: self.0 came from a successful SetupDiCreateDeviceInfoList call, is valid
        // (checked above), is owned by this value alone and is destroyed nowhere else.
        let destroyed = unsafe { SetupDiDestroyDeviceInfoList(self.0) };
        if let Err(error) = destroyed {
            log::warn!("SetupDiDestroyDeviceInfoList failed: {error}");
        }
    }
}
