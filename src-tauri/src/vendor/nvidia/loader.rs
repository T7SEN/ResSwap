//! Loads nvapi64.dll from System32 only, resolves functions through nvapi_QueryInterface
//! and pairs NvAPI_Initialize with NvAPI_Unload (REQ-NV-02, REQ-NV-03, REQ-SEC-03).
