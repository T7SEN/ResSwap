//! The NVIDIA implementation of the vendor extras: NVAPI custom displays, NV_Modes and GPU
//! scaling (REQ-NV-01 to REQ-NV-28). Every call goes through the `VendorExtras` trait.

pub mod custom_display;
pub mod fake;
pub mod ffi;
pub mod loader;
pub mod nv_modes;
pub mod scaling;
