//! Auto-switch: the poller that reads the process list every 2 s while a watched profile
//! needs it, and the state machine that applies once and restores once (REQ-WATCH-01 to
//! REQ-WATCH-10, D-007). It reads processes only through the `ProcessList` trait.

#![forbid(unsafe_code)]
