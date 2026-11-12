//! Events the core broadcasts to every open window. Each event name always carries the
//! same payload type, because only the typed helpers below send them.

#![forbid(unsafe_code)]

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, Runtime};
use ts_rs::TS;

use crate::ipc::types::{
    AppState, CountdownTick, ModeReverted, PendingChange, RaisedError, UpdateInfo, WatcherMatch,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "kebab-case")]
pub enum CoreEvent {
    StateChanged,
    ModePendingConfirm,
    CountdownTick,
    ModeReverted,
    WatcherMatched,
    ErrorRaised,
    UpdateAvailable,
}

impl CoreEvent {
    pub const ALL: [Self; 7] = [
        Self::StateChanged,
        Self::ModePendingConfirm,
        Self::CountdownTick,
        Self::ModeReverted,
        Self::WatcherMatched,
        Self::ErrorRaised,
        Self::UpdateAvailable,
    ];

    /// The event name the window listens for; the same string serde writes.
    pub const fn name(self) -> &'static str {
        match self {
            Self::StateChanged => "state-changed",
            Self::ModePendingConfirm => "mode-pending-confirm",
            Self::CountdownTick => "countdown-tick",
            Self::ModeReverted => "mode-reverted",
            Self::WatcherMatched => "watcher-matched",
            Self::ErrorRaised => "error-raised",
            Self::UpdateAvailable => "update-available",
        }
    }
}

/// Where the core sends its events: Tauri's broadcast emit in the app, a recorder in tests.
pub trait EventSink: Send + Sync {
    /// Whether any window could receive an event. With none, the core builds no snapshot
    /// and sends nothing, so it stays light while a game runs.
    fn has_listeners(&self) -> bool;
    /// Broadcasts one event to every window.
    fn send(&self, event: CoreEvent, payload: serde_json::Value);
}

/// The sink before the app is set up: nobody listens.
#[derive(Debug, Default)]
pub struct NoListeners;

impl EventSink for NoListeners {
    fn has_listeners(&self) -> bool {
        false
    }

    fn send(&self, _event: CoreEvent, _payload: serde_json::Value) {}
}

/// Tauri's broadcast emit. Never `emit_to` or `emit_filter`, so the mocked core can
/// reproduce every event.
pub struct TauriSink<R: Runtime> {
    app: AppHandle<R>,
}

impl<R: Runtime> TauriSink<R> {
    pub fn new(app: AppHandle<R>) -> Self {
        Self { app }
    }
}

impl<R: Runtime> EventSink for TauriSink<R> {
    fn has_listeners(&self) -> bool {
        !self.app.webview_windows().is_empty()
    }

    fn send(&self, event: CoreEvent, payload: serde_json::Value) {
        if let Err(error) = self.app.emit(event.name(), payload) {
            log::warn!("{} not sent: {error}", event.name());
        }
    }
}

fn send<T: Serialize>(sink: &dyn EventSink, event: CoreEvent, payload: &T) {
    match serde_json::to_value(payload) {
        Ok(value) => sink.send(event, value),
        Err(error) => log::error!("{} payload did not serialise: {error}", event.name()),
    }
}

pub fn emit_state_changed(sink: &dyn EventSink, state: &AppState) {
    send(sink, CoreEvent::StateChanged, state);
}

pub fn emit_mode_pending_confirm(sink: &dyn EventSink, pending: &PendingChange) {
    send(sink, CoreEvent::ModePendingConfirm, pending);
}

pub fn emit_countdown_tick(sink: &dyn EventSink, tick: &CountdownTick) {
    send(sink, CoreEvent::CountdownTick, tick);
}

pub fn emit_mode_reverted(sink: &dyn EventSink, reverted: &ModeReverted) {
    send(sink, CoreEvent::ModeReverted, reverted);
}

pub fn emit_watcher_matched(sink: &dyn EventSink, matched: &WatcherMatch) {
    send(sink, CoreEvent::WatcherMatched, matched);
}

pub fn emit_error_raised(sink: &dyn EventSink, raised: &RaisedError) {
    send(sink, CoreEvent::ErrorRaised, raised);
}

pub fn emit_update_available(sink: &dyn EventSink, update: &UpdateInfo) {
    send(sink, CoreEvent::UpdateAvailable, update);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn req_ui_15_event_names_match_their_wire_form() {
        for event in CoreEvent::ALL {
            assert_eq!(
                serde_json::to_value(event).unwrap(),
                serde_json::Value::from(event.name())
            );
        }
    }
}
