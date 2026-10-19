//! The data surface the UI needs (`LocalBackend` in `src/lib/local-backend.ts`).
//!
//! Every call returns a future. The [`crate::Store`] runs them through
//! [`crate::io::spawn`]: the desktop's [`crate::daemon::DaemonBackend`]
//! blocks while polled, on GPUI's background threads; the browser's
//! [`crate::web::WebDaemonBackend`] fetches on the page's event loop. The
//! daemon-less replica (`roostr-replica`, the engine linked in) implements
//! the same trait and adds an on-device [`crate::account::Account`].

use anyhow::Result;
use serde_json::{Map, Value as Json};

use crate::account::Account;
use crate::io::{BoxFuture, BoxStream, MaybeSend, MaybeSync};
use crate::types::{Object, ObjectSummary, QueryResult, RelationDef, Settings, Space, SyncStatus};

pub type JsonMap = Map<String, Json>;

/// A live notification from the backend's commit stream.
#[derive(Clone, Debug, PartialEq)]
pub enum BackendEvent {
    /// The stream (re)connected: anything may have changed while it was down.
    Hello,
    /// One object committed a change.
    Committed(String),
    /// The replica's relay sync moved (a daemon sends none).
    Status(SyncStatus),
}

pub trait Backend: MaybeSend + MaybeSync + 'static {
    /// Human-readable description for status UI ("glon-odin at http://127.0.0.1:7333").
    fn describe(&self) -> String;
    fn settings(&self) -> BoxFuture<Result<Settings>>;
    fn objects(&self) -> BoxFuture<Result<Vec<ObjectSummary>>>;
    fn object(&self, id: &str) -> BoxFuture<Result<Object>>;
    fn channels(&self) -> BoxFuture<Result<Vec<Space>>>;
    fn relations(&self) -> BoxFuture<Result<Vec<RelationDef>>>;
    /// `POST /api/query` with a QueryBody (filters, sorts, textQuery, setId, type, limit, offset, ...).
    fn query(&self, body: &Json) -> BoxFuture<Result<QueryResult>>;
    /// `POST /api/mutate`. `params` must already carry `now_ms`/`tz_offset_min`
    /// (the [`crate::Store`] adds them). Errors when the daemon answers `ok: false`.
    fn mutate(&self, action: &str, params: JsonMap) -> BoxFuture<Result<JsonMap>>;
    /// `POST /api/serving {objectIds}`: which machine serves each object
    /// (`{ "<id>": {machineId, reason, skills, candidates} }`).
    fn serving(&self, object_ids: &[String]) -> BoxFuture<Result<JsonMap>>;
    /// `GET /api/sync/digest`: `{digest, objects, changes, computedAt}`.
    fn sync_digest(&self) -> BoxFuture<Result<JsonMap>>;
    /// One connection to the commit stream: its events, ending with one `Err`
    /// when the stream fails or closes (the caller reconnects). Dropping the
    /// stream closes the connection.
    fn events(&self) -> BoxStream<Result<BackendEvent>>;
    /// The identity this backend holds on the device (the replica); `None`
    /// for a daemon, whose harness serves identity, profile and join requests.
    fn account(&self) -> Option<std::sync::Arc<dyn Account>> {
        None
    }
}

/// The commit stream's SSE framing (`data:` lines, a blank line dispatches),
/// fed a line at a time. Shared by the desktop and browser clients.
#[derive(Default)]
pub struct SseParser {
    data: String,
}

impl SseParser {
    /// One line without its terminator; returns the event a blank line completes.
    pub fn line(&mut self, line: &str) -> Option<BackendEvent> {
        let line = line.strip_suffix('\r').unwrap_or(line);
        if let Some(rest) = line.strip_prefix("data:") {
            if !self.data.is_empty() {
                self.data.push('\n');
            }
            self.data.push_str(rest.trim_start());
            return None;
        }
        if !line.is_empty() || self.data.is_empty() {
            return None;
        }
        let payload = std::mem::take(&mut self.data);
        let event = serde_json::from_str::<Json>(&payload).ok()?;
        if event.get("hello").and_then(Json::as_bool) == Some(true) {
            return Some(BackendEvent::Hello);
        }
        event
            .get("objectId")
            .and_then(Json::as_str)
            .map(|id| BackendEvent::Committed(id.to_string()))
    }
}

/// Percent-encode one URL path segment (`encodeURIComponent`).
pub fn encode_path_segment(segment: &str) -> String {
    let mut out = String::with_capacity(segment.len());
    for byte in segment.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char)
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

/// `/api/mutate`'s answer, failing on `ok: false`.
pub fn mutate_result(action: &str, result: JsonMap) -> Result<JsonMap> {
    if result.get("ok").and_then(Json::as_bool) != Some(true) {
        let error = result.get("error").and_then(Json::as_str).unwrap_or("");
        anyhow::bail!(if error.is_empty() {
            format!("Mutation {action} failed")
        } else {
            error.to_string()
        });
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Feed a whole SSE body, one line at a time, as both clients do.
    fn events(body: &str) -> Vec<BackendEvent> {
        let mut parser = SseParser::default();
        body.split('\n')
            .filter_map(|line| parser.line(line))
            .collect()
    }

    #[test]
    fn the_commit_stream_yields_parsed_object_ids_not_event_bodies() {
        assert_eq!(
            events("data: {\"hello\":true}\n\ndata: {\"objectId\":\"doc-é\"}\n\n"),
            vec![BackendEvent::Hello, BackendEvent::Committed("doc-é".into())]
        );
    }

    #[test]
    fn crlf_comments_and_multiline_data_make_one_event() {
        assert_eq!(
            events(
                ": heartbeat\r\nevent: changed\r\ndata: {\"objectId\":\r\ndata: \"doc-é\"}\r\n\r\n"
            ),
            vec![BackendEvent::Committed("doc-é".into())]
        );
    }
}
