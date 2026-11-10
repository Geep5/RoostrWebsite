//! Direct lines on the app's side ([`roostr_direct`]): while a vault is open
//! over a backend that carries signals ([`Backend::direct_lines`]), the
//! [`crate::Store`] runs one viewer for this install - it says hello, answers
//! the hosts' offers, and each channel message enters the receive path as
//! the same [`BackendEvent::Live`] a relay event or a store's live frame
//! becomes, so dedup by (run, seq) and the chat's progress are unchanged.
//!
//! The store is the single owner: the replica hears signals on its relay
//! subscription, a daemon-backed app through the store's `/api/events`
//! `signal` frames, and either way they reach the one viewer.

use std::sync::Arc;

use roostr_direct::{Delivery, Direct, Scope};
use serde_json::Value as Json;

use crate::backend::{Backend, BackendEvent, JsonMap};
use crate::live::LiveEvent;
use crate::ui_state::UiState;

/// The [`UiState`] key of this install's device id.
pub const DEVICE_KEY: &str = "direct-device";

/// This install's stable device id for direct lines, made on first use and
/// kept with the device's UI state (`localStorage` in the browser, the
/// config dir's `ui-state.json` on the desktop).
pub fn device_id(ui: &mut UiState) -> String {
    if let Some(id) = ui.get::<String>(DEVICE_KEY).filter(|id| !id.is_empty()) {
        return id;
    }
    let id = uuid::Uuid::new_v4().to_string();
    ui.set(DEVICE_KEY, &id);
    id
}

/// `signal_publish`'s parameters: the payload and, for a shared space, its id.
pub fn publish_params(signal: Json, space: Option<String>) -> JsonMap {
    let mut params = JsonMap::new();
    params.insert("event".into(), signal);
    if let Some(space) = space {
        params.insert("space".into(), Json::String(space));
    }
    params
}

/// A channel message as the receive path takes it: the live payload, one
/// per message, becomes the [`BackendEvent::Live`] its relay copy would.
pub fn channel_event(text: &str) -> Option<BackendEvent> {
    let payload: Json = serde_json::from_str(text).ok()?;
    LiveEvent::parse(&payload).map(BackendEvent::Live)
}

/// Whether a line introduced in `line` may speak of an object of
/// `object_space` (`""` = personal, `None` = not known here): the account's
/// own line of anything, a shared space's line only of that space's objects
/// - another member's computer must not write into this account's chats.
pub fn line_may_speak(line: &Scope, object_space: Option<&str>) -> bool {
    match line {
        None => true,
        Some(space) => object_space == Some(space.as_str()),
    }
}

/// Start the viewer of `device` over `backend`: signals go out through
/// [`Backend::publish_signal`], channel messages to `deliver`.
#[cfg(not(target_family = "wasm"))]
pub fn start(
    backend: Arc<dyn Backend>,
    device: String,
    deliver: impl Fn(Delivery) + Send + Sync + 'static,
) -> Direct {
    Direct::start(
        device,
        roostr_direct::platform_peers(),
        publisher(backend),
        Arc::new(deliver),
    )
}

/// Start the viewer of `device` over `backend`: signals go out through
/// [`Backend::publish_signal`], channel messages to `deliver`.
#[cfg(target_family = "wasm")]
pub fn start(
    backend: Arc<dyn Backend>,
    device: String,
    deliver: impl Fn(Delivery) + 'static,
) -> Direct {
    Direct::start(
        device,
        roostr_direct::platform_peers(),
        publisher(backend),
        Arc::new(deliver),
    )
}

fn publisher(backend: Arc<dyn Backend>) -> roostr_direct::Publish {
    Arc::new(move |signal, space| {
        let sent = backend.publish_signal(signal, space);
        Box::pin(async move { sent.await.map_err(|error| format!("{error:#}")) })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::SseParser;

    const PAYLOAD: &str = r#"{"v":1,"run":"r1","agent":"a","object":"o","thread":"__discussion__","seq":4,"kind":"step","text":"shell_exec: ls","ts":9}"#;

    #[test]
    fn a_channel_message_is_the_live_event_its_relay_copy_is() {
        let mut parser = SseParser::default();
        let frame = format!("data: {{\"type\":\"live\",\"origin\":\"relay\",\"live\":{PAYLOAD}}}");
        assert!(parser.line(&frame).is_none());
        let relayed = parser.line("").expect("the relay copy");
        let direct = channel_event(PAYLOAD).expect("the channel message");
        assert_eq!(direct, relayed);
        assert!(matches!(&direct, BackendEvent::Live(e) if e.seq == 4 && e.run == "r1"));
        assert_eq!(channel_event("not json"), None);
        assert_eq!(channel_event(r#"{"v":2,"run":"r1","object":"o"}"#), None);
    }

    #[test]
    fn a_shared_spaces_line_speaks_only_of_that_space() {
        assert!(line_may_speak(&None, Some("")), "own line, personal object");
        assert!(line_may_speak(&None, Some("sp")), "own line, any space");
        assert!(line_may_speak(&None, None), "own line, object not loaded");
        let space = Some("sp".to_string());
        assert!(line_may_speak(&space, Some("sp")));
        assert!(
            !line_may_speak(&space, Some("")),
            "not into a personal chat"
        );
        assert!(
            !line_may_speak(&space, Some("other")),
            "not into another space"
        );
        assert!(
            !line_may_speak(&space, None),
            "not of an object unknown here"
        );
    }

    #[test]
    fn the_device_id_is_made_once_and_kept() {
        let dir = std::env::temp_dir().join(format!("roostr-direct-{}", uuid::Uuid::new_v4()));
        let path = dir.join("ui-state.json");
        let mut ui = UiState::load_from(Some(path.clone()));
        let id = device_id(&mut ui);
        assert_eq!(id.len(), 36, "a uuid: {id}");
        assert_eq!(device_id(&mut ui), id, "the same id again");
        let mut reloaded = UiState::load_from(Some(path));
        assert_eq!(device_id(&mut reloaded), id, "kept across restarts");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn publish_params_carry_the_space_only_for_shared_scopes() {
        let personal = publish_params(serde_json::json!({"v":1}), None);
        assert_eq!(Json::Object(personal), serde_json::json!({"event":{"v":1}}));
        let shared = publish_params(serde_json::json!({"v":1}), Some("sp".into()));
        assert_eq!(
            Json::Object(shared),
            serde_json::json!({"event":{"v":1},"space":"sp"})
        );
    }
}
