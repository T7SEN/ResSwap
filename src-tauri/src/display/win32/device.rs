//! The adapter behind a CCD adapter device path: its device instance ID, description and
//! driver key, read through SetupAPI. DISPLAYCONFIG_ADAPTER_NAME says adapterDevicePath "can
//! be used with SetupAPI.dll"; it is opened as the name of a device interface
//! (SetupDiOpenDeviceInterfaceW), and SetupDiGetDeviceInstanceIdW names the device behind it
//! (REQ-DISP-02, REQ-DISP-07). When the open fails, the instance ID stays empty, so the
//! monitor counts as virtual and sits behind the show-hidden toggle.

use windows::Win32::Devices::DeviceAndDriverInstallation::{
    MAX_DEVICE_ID_LEN, SETUP_DI_REGISTRY_PROPERTY, SP_DEVICE_INTERFACE_DATA,
    SP_DEVICE_INTERFACE_DETAIL_DATA_W, SP_DEVINFO_DATA, SPDRP_DEVICEDESC, SPDRP_DRIVER,
    SetupDiGetDeviceInstanceIdW, SetupDiGetDeviceInterfaceDetailW,
    SetupDiGetDeviceRegistryPropertyW, SetupDiOpenDeviceInterfaceW,
};
use windows::Win32::Foundation::{ERROR_INSUFFICIENT_BUFFER, WIN32_ERROR};
use windows::core::HSTRING;

use crate::display::win32::ccd::wide_to_string;
use crate::error::AppError;
use crate::system::handles::DeviceInfoList;

/// The largest property ResSwap reads, in bytes; descriptions and driver keys are far
/// shorter.
const PROPERTY_CAPACITY: usize = 2048;

/// The adapter device behind one adapter device path, as plain data.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AdapterDevice {
    /// The device instance ID, for example one that starts with `PCI\VEN_10DE&`.
    pub instance_id: String,
    /// SPDRP_DEVICEDESC, the adapter's description.
    pub description: Option<String>,
    /// SPDRP_DRIVER, the driver key under the device setup classes, of the form
    /// `{4d36e968-e325-11ce-bfc1-08002be10318}\NNNN` for a display adapter.
    pub driver_key: Option<String>,
}

/// Opens the device that exposes `interface_path` and reads its instance ID, description and
/// driver key. Changes nothing.
pub fn adapter_device(interface_path: &str) -> Result<AdapterDevice, AppError> {
    let failed = |call: &str, error: windows::core::Error| {
        AppError::internal(format!("{call} failed for an adapter device path: {error}"))
    };
    let list = DeviceInfoList::create_empty()
        .map_err(|error| failed("SetupDiCreateDeviceInfoList", error))?;
    let path = HSTRING::from(interface_path);
    let mut interface = SP_DEVICE_INTERFACE_DATA {
        cbSize: size_of::<SP_DEVICE_INTERFACE_DATA>() as u32,
        ..Default::default()
    };
    // SAFETY: the set is owned by `list` and valid; `path` is a NUL-terminated device
    // interface path that lives until the call returns; `interface` is an
    // SP_DEVICE_INTERFACE_DATA on this frame with cbSize set to its size; open flags 0.
    unsafe { SetupDiOpenDeviceInterfaceW(list.raw(), &path, 0, Some(&mut interface)) }
        .map_err(|error| failed("SetupDiOpenDeviceInterfaceW", error))?;

    let device = interface_device(&list, &interface)
        .map_err(|error| failed("SetupDiGetDeviceInterfaceDetailW", error))?;

    let mut id = [0u16; MAX_DEVICE_ID_LEN as usize];
    // SAFETY: the set and `device` come from the calls above; `id` holds MAX_DEVICE_ID_LEN
    // wide characters and the crate passes that length, so the call writes inside it; the
    // required size is not asked for (None).
    unsafe { SetupDiGetDeviceInstanceIdW(list.raw(), &device, Some(&mut id), None) }
        .map_err(|error| failed("SetupDiGetDeviceInstanceIdW", error))?;

    Ok(AdapterDevice {
        instance_id: wide_to_string(&id),
        description: text_property(&list, &device, SPDRP_DEVICEDESC),
        driver_key: text_property(&list, &device, SPDRP_DRIVER),
    })
}

/// The device that supports `interface`, in the two calls SetupDiGetDeviceInterfaceDetail
/// describes: the first reports the detail size, the second fills the detail and the device.
fn interface_device(
    list: &DeviceInfoList,
    interface: &SP_DEVICE_INTERFACE_DATA,
) -> windows::core::Result<SP_DEVINFO_DATA> {
    let mut required = 0u32;
    // SAFETY: the set is owned by `list`; `interface` was filled by
    // SetupDiOpenDeviceInterfaceW; no detail buffer and a size of 0 ask only for the size,
    // written to `required` on this frame.
    let sized = unsafe {
        SetupDiGetDeviceInterfaceDetailW(list.raw(), interface, None, 0, Some(&mut required), None)
    };
    if let Err(error) = sized
        && WIN32_ERROR::from_error(&error) != Some(ERROR_INSUFFICIENT_BUFFER)
    {
        return Err(error);
    }
    let header = size_of::<SP_DEVICE_INTERFACE_DETAIL_DATA_W>();
    let bytes = (required as usize).max(header);
    // u32 words keep the buffer aligned for the detail struct's cbSize field.
    let mut buffer = vec![0u32; bytes.div_ceil(4)];
    let detail = buffer
        .as_mut_ptr()
        .cast::<SP_DEVICE_INTERFACE_DETAIL_DATA_W>();
    if let Some(first) = buffer.first_mut() {
        // cbSize is the size of the fixed part of the struct, not of the buffer
        // (SP_DEVICE_INTERFACE_DETAIL_DATA_W).
        *first = header as u32;
    }
    let mut device = SP_DEVINFO_DATA {
        cbSize: size_of::<SP_DEVINFO_DATA>() as u32,
        ..Default::default()
    };
    // SAFETY: `buffer` holds at least `required` bytes, is aligned for the detail struct and
    // starts with cbSize set to the struct's fixed size; the size passed is the buffer's own
    // size in bytes; `device` has cbSize set; all outlive the call.
    unsafe {
        SetupDiGetDeviceInterfaceDetailW(
            list.raw(),
            interface,
            Some(detail),
            (buffer.len() * 4) as u32,
            None,
            Some(&mut device),
        )
    }?;
    Ok(device)
}

/// One text property of the device; `None` when it is missing or unreadable.
fn text_property(
    list: &DeviceInfoList,
    device: &SP_DEVINFO_DATA,
    property: SETUP_DI_REGISTRY_PROPERTY,
) -> Option<String> {
    let mut buffer = [0u8; PROPERTY_CAPACITY];
    let mut kind = 0u32;
    // SAFETY: the set and `device` come from adapter_device; `buffer` is PROPERTY_CAPACITY
    // bytes on this frame and the crate passes that length; `kind` is a valid out pointer;
    // the required size is not asked for (None).
    let read = unsafe {
        SetupDiGetDeviceRegistryPropertyW(
            list.raw(),
            device,
            property,
            Some(&mut kind),
            Some(&mut buffer),
            None,
        )
    };
    if read.is_err() {
        return None;
    }
    let units: Vec<u16> = buffer
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| u16::from_le_bytes(*pair))
        .collect();
    let text = wide_to_string(&units);
    (!text.is_empty()).then_some(text)
}
