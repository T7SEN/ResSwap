//! Settings migrations: one step per schema version, run on the raw JSON in version order
//! before the settings are used (REQ-SET-03, D-004).

#![forbid(unsafe_code)]
