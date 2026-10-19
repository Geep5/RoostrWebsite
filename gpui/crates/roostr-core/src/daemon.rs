//! Client for the local glon-odin daemon (`glonOdin/README.md`, API).
//!
//! A native app running as the vault's user reads the service token from
//! `$GLON_DATA/api-token` (mode 0600) - the same credential the operator
//! tools use. Browser tabs pair instead ([`crate::web`]).
//!
//! The requests block; [`Backend`] futures run them when polled, which
//! [`crate::io::spawn`] does on GPUI's background threads.

use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context as _, Result, anyhow, bail};
use serde::de::DeserializeOwned;
use serde_json::Value as Json;
use ureq::Agent;

use crate::backend::{
    Backend, BackendEvent, JsonMap, SseParser, encode_path_segment, mutate_result,
};
use crate::io::{BoxFuture, BoxStream, boxed, boxed_stream};
use crate::types::{Object, ObjectSummary, QueryResult, RelationDef, Settings, Space};

pub const DEFAULT_API: &str = "http://127.0.0.1:7333";
/// Objects with long histories serialize large; the default 10 MiB cap is too small.
const BODY_LIMIT: u64 = 512 * 1024 * 1024;

/// Where to reach the daemon and how to authenticate.
#[derive(Clone, Debug)]
pub struct DaemonConfig {
    pub base_url: String,
    pub token: String,
    /// Where the token came from, for error messages.
    pub token_source: String,
}

impl DaemonConfig {
    /// `ROOSTR_API` (default `http://127.0.0.1:7333`) and the token from
    /// `ROOSTR_API_TOKEN`, else `$GLON_DATA/api-token` (default `~/.glon`).
    pub fn from_env() -> Result<Self> {
        let base_url = std::env::var("ROOSTR_API")
            .unwrap_or_else(|_| DEFAULT_API.to_string())
            .trim_end_matches('/')
            .to_string();
        if let Ok(token) = std::env::var("ROOSTR_API_TOKEN") {
            return Ok(Self {
                base_url,
                token: token.trim().to_string(),
                token_source: "ROOSTR_API_TOKEN".into(),
            });
        }
        let path = token_path();
        let token = std::fs::read_to_string(&path)
            .with_context(|| format!("Reading the daemon's service token at {}. Start glon-odin (`./glon-odin serve`) as this user, or set GLON_DATA.", path.display()))?;
        Ok(Self {
            base_url,
            token: token.trim().to_string(),
            token_source: path.display().to_string(),
        })
    }
}

/// `$GLON_DATA/api-token`, `GLON_DATA` defaulting to `~/.glon`.
pub fn token_path() -> PathBuf {
    let data = std::env::var_os("GLON_DATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| dirs::home_dir().unwrap_or_default().join(".glon"));
    data.join("api-token")
}

pub struct DaemonBackend {
    client: Arc<Client>,
}

/// The blocking client a [`DaemonBackend`] future owns while it runs.
struct Client {
    config: DaemonConfig,
    agent: Agent,
    /// No global timeout: the SSE stream stays open indefinitely.
    stream_agent: Agent,
}

impl DaemonBackend {
    pub fn new(config: DaemonConfig) -> Self {
        let agent = Agent::new_with_config(
            Agent::config_builder()
                .http_status_as_error(false)
                .timeout_connect(Some(Duration::from_secs(3)))
                .timeout_global(Some(Duration::from_secs(120)))
                .build(),
        );
        let stream_agent = Agent::new_with_config(
            Agent::config_builder()
                .http_status_as_error(false)
                .timeout_connect(Some(Duration::from_secs(3)))
                .build(),
        );
        Self {
            client: Arc::new(Client {
                config,
                agent,
                stream_agent,
            }),
        }
    }

    pub fn config(&self) -> &DaemonConfig {
        &self.client.config
    }

    /// Run `call` on the client when the future is polled.
    fn run<T: Send + 'static>(
        &self,
        call: impl FnOnce(&Client) -> Result<T> + Send + 'static,
    ) -> BoxFuture<Result<T>> {
        let client = self.client.clone();
        boxed(async move { call(&client) })
    }
}

impl Client {
    fn url(&self, path: &str) -> String {
        format!("{}{}", self.config.base_url, path)
    }

    fn auth(&self) -> String {
        format!("Bearer {}", self.config.token)
    }

    fn get<T: DeserializeOwned>(&self, path: &str) -> Result<T> {
        let response = self
            .agent
            .get(self.url(path))
            .header("Authorization", self.auth())
            .call()
            .map_err(|e| self.unreachable(e))?;
        self.decode(path, response)
    }

    fn post<T: DeserializeOwned>(&self, path: &str, body: &Json) -> Result<T> {
        let response = self
            .agent
            .post(self.url(path))
            .header("Authorization", self.auth())
            .send_json(body)
            .map_err(|e| self.unreachable(e))?;
        self.decode(path, response)
    }

    fn unreachable(&self, error: ureq::Error) -> anyhow::Error {
        anyhow!(
            "Cannot reach the Roostr daemon at {}: {error}. Is `glon-odin serve` running?",
            self.config.base_url
        )
    }

    fn decode<T: DeserializeOwned>(
        &self,
        path: &str,
        mut response: http_response::Response,
    ) -> Result<T> {
        let status = response.status();
        let body = response
            .body_mut()
            .with_config()
            .limit(BODY_LIMIT)
            .read_to_string()?;
        if status.as_u16() == 401 || status.as_u16() == 403 {
            bail!(
                "The daemon refused the service token from {} ({status}).",
                self.config.token_source
            );
        }
        if !status.is_success() {
            let detail = serde_json::from_str::<Json>(&body)
                .ok()
                .and_then(|j| j.get("error").and_then(Json::as_str).map(str::to_string))
                .unwrap_or(body);
            bail!("{path}: {status} {detail}");
        }
        serde_json::from_str(&body).with_context(|| format!("{path}: unexpected response"))
    }
}

/// ureq's response type, named once.
mod http_response {
    pub type Response = ureq::http::Response<ureq::Body>;
}

impl Backend for DaemonBackend {
    fn describe(&self) -> String {
        format!("glon-odin at {}", self.client.config.base_url)
    }

    fn settings(&self) -> BoxFuture<Result<Settings>> {
        self.run(|c| c.get("/api/settings"))
    }

    fn objects(&self) -> BoxFuture<Result<Vec<ObjectSummary>>> {
        self.run(|c| c.get("/api/objects"))
    }

    fn object(&self, id: &str) -> BoxFuture<Result<Object>> {
        let path = format!("/api/objects/{}", encode_path_segment(id));
        self.run(move |c| c.get(&path))
    }

    fn channels(&self) -> BoxFuture<Result<Vec<Space>>> {
        self.run(|c| c.get("/api/channels"))
    }

    fn relations(&self) -> BoxFuture<Result<Vec<RelationDef>>> {
        self.run(|c| c.get("/api/relations"))
    }

    fn query(&self, body: &Json) -> BoxFuture<Result<QueryResult>> {
        let body = body.clone();
        self.run(move |c| c.post("/api/query", &body))
    }

    fn mutate(&self, action: &str, mut params: JsonMap) -> BoxFuture<Result<JsonMap>> {
        let action = action.to_string();
        params.insert("action".into(), Json::String(action.clone()));
        self.run(move |c| {
            let result: JsonMap = c.post("/api/mutate", &Json::Object(params))?;
            mutate_result(&action, result)
        })
    }

    fn serving(&self, object_ids: &[String]) -> BoxFuture<Result<JsonMap>> {
        let body = serde_json::json!({ "objectIds": object_ids });
        self.run(move |c| c.post("/api/serving", &body))
    }

    fn sync_digest(&self) -> BoxFuture<Result<JsonMap>> {
        self.run(|c| c.get("/api/sync/digest"))
    }

    /// A plain thread reads the stream (it blocks for the life of the
    /// connection) and hands events over a channel; it ends with the
    /// connection or once the receiver is gone.
    fn events(&self) -> BoxStream<Result<BackendEvent>> {
        let (tx, rx) = futures::channel::mpsc::unbounded::<Result<BackendEvent>>();
        let client = self.client.clone();
        let spawned = std::thread::Builder::new()
            .name("roostr-events".into())
            .spawn({
                let tx = tx.clone();
                move || {
                    let result =
                        client.stream_events(&mut |event| tx.unbounded_send(Ok(event)).is_ok());
                    if let Err(error) = result {
                        let _ = tx.unbounded_send(Err(error));
                    }
                }
            });
        if let Err(error) = spawned {
            let _ = tx.unbounded_send(Err(anyhow!("could not start the event reader: {error}")));
        }
        boxed_stream(rx)
    }
}

impl Client {
    /// Blocks, delivering commit events until `on_event` returns `false`
    /// (returns `Ok`) or the stream fails (returns `Err`).
    fn stream_events(&self, on_event: &mut dyn FnMut(BackendEvent) -> bool) -> Result<()> {
        let response = self
            .stream_agent
            .get(self.url("/api/events"))
            .header("Authorization", self.auth())
            .header("Accept", "text/event-stream")
            .call()
            .map_err(|e| self.unreachable(e))?;
        let status = response.status();
        if !status.is_success() {
            bail!("/api/events: {status}");
        }
        let reader = BufReader::new(response.into_body().into_reader());
        let mut parser = SseParser::default();
        for line in reader.lines() {
            if let Some(event) = parser.line(&line?)
                && !on_event(event)
            {
                return Ok(());
            }
        }
        bail!("The daemon closed the event stream")
    }
}

#[cfg(test)]
mod tests {
    use std::io::{Read as _, Write as _};
    use std::net::{TcpListener, TcpStream};
    use std::sync::mpsc;

    use super::*;

    const TOKEN: &str = "abababababababababababababababababababababababababababababababab";

    /// One request the fake daemon received.
    struct Request {
        /// Request line and headers, header names lower-cased.
        head: String,
        body: String,
    }

    impl Request {
        fn header(&self, name: &str) -> Option<&str> {
            self.head.lines().find_map(|line| {
                let (key, value) = line.split_once(':')?;
                key.eq_ignore_ascii_case(name).then(|| value.trim())
            })
        }
    }

    fn read_request(stream: &mut TcpStream) -> Request {
        let mut raw = Vec::new();
        let mut byte = [0u8; 1];
        while !raw.ends_with(b"\r\n\r\n") {
            stream.read_exact(&mut byte).expect("request head");
            raw.push(byte[0]);
        }
        let head = String::from_utf8(raw).expect("utf-8 head");
        let length = head
            .lines()
            .find_map(|l| {
                let (k, v) = l.split_once(':')?;
                k.eq_ignore_ascii_case("content-length")
                    .then(|| v.trim().parse::<usize>().ok())?
            })
            .unwrap_or(0);
        let mut body = vec![0u8; length];
        stream.read_exact(&mut body).expect("request body");
        Request {
            head,
            body: String::from_utf8(body).expect("utf-8 body"),
        }
    }

    /// A daemon on a loopback port of this process: each connection gets the
    /// next answer, written in `chunk`-byte pieces, then the connection closes.
    fn fake_daemon(
        answers: Vec<Vec<u8>>,
        chunk: usize,
    ) -> (DaemonBackend, mpsc::Receiver<Request>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("loopback port");
        let base_url = format!("http://{}", listener.local_addr().expect("address"));
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            for answer in answers {
                let Ok((mut stream, _)) = listener.accept() else {
                    return;
                };
                let _ = stream.set_nodelay(true);
                let _ = tx.send(read_request(&mut stream));
                for piece in answer.chunks(chunk) {
                    if stream
                        .write_all(piece)
                        .and_then(|()| stream.flush())
                        .is_err()
                    {
                        break;
                    }
                }
            }
        });
        let backend = DaemonBackend::new(DaemonConfig {
            base_url,
            token: TOKEN.into(),
            token_source: "the test".into(),
        });
        (backend, rx)
    }

    fn json_answer(body: &Json) -> Vec<u8> {
        let body = body.to_string();
        format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
        .into_bytes()
    }

    #[test]
    fn mutations_use_the_action_wire_shape_and_surface_daemon_failures() {
        let (backend, requests) = fake_daemon(
            vec![
                json_answer(&serde_json::json!({ "ok": true, "objectId": "doc" })),
                json_answer(
                    &serde_json::json!({ "ok": false, "error": "writer permission required" }),
                ),
            ],
            usize::MAX,
        );
        let params = serde_json::json!({
            "objectId": "doc", "key": "name", "value": "Title", "action": "must-not-override",
        });
        let Json::Object(params) = params else {
            unreachable!()
        };
        let answer = futures::executor::block_on(backend.mutate("field_set", params))
            .expect("mutation succeeds");
        assert_eq!(
            Json::Object(answer),
            serde_json::json!({ "ok": true, "objectId": "doc" })
        );
        let sent = requests.recv().expect("one request");
        assert!(sent.head.starts_with("POST /api/mutate "), "{}", sent.head);
        // ureq adds "; charset=utf-8": still the JSON media type.
        assert_eq!(
            sent.header("content-type")
                .and_then(|t| t.split(';').next()),
            Some("application/json")
        );
        assert_eq!(
            sent.header("authorization"),
            Some(format!("Bearer {TOKEN}").as_str())
        );
        assert_eq!(
            serde_json::from_str::<Json>(&sent.body).expect("json body"),
            serde_json::json!({ "objectId": "doc", "key": "name", "value": "Title", "action": "field_set" })
        );
        let Json::Object(params) = serde_json::json!({ "objectId": "doc" }) else {
            unreachable!()
        };
        let error = futures::executor::block_on(backend.mutate("field_set", params))
            .expect_err("ok: false fails");
        assert_eq!(error.to_string(), "writer permission required");
    }

    #[test]
    fn the_event_stream_parses_split_utf8_crlf_and_multiline_data_with_bearer_auth() {
        let mut answer =
            b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n"
                .to_vec();
        answer.extend_from_slice(
            ": heartbeat\r\nevent: changed\r\ndata: {\"objectId\":\r\ndata: \"doc-é\"}\r\n\r\n"
                .as_bytes(),
        );
        // Single-byte writes split field names, CRLF, JSON and the two-byte "é".
        let (backend, requests) = fake_daemon(vec![answer], 1);
        let events: Vec<std::result::Result<BackendEvent, String>> =
            futures::executor::block_on_stream(backend.events())
                .map(|event| event.map_err(|e| e.to_string()))
                .collect();
        assert_eq!(
            events,
            vec![
                Ok(BackendEvent::Committed("doc-é".into())),
                Err("The daemon closed the event stream".into()),
            ]
        );
        let sent = requests.recv().expect("one request");
        assert!(sent.head.starts_with("GET /api/events "), "{}", sent.head);
        assert_eq!(
            sent.header("authorization"),
            Some(format!("Bearer {TOKEN}").as_str())
        );
        assert_eq!(sent.header("accept"), Some("text/event-stream"));
    }
}
