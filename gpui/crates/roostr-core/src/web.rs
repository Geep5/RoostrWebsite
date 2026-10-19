//! The browser transport (`src/lib/local-transport.ts`): a tab on
//! roostr.space (or a localhost dev server) pairs with the daemon on the
//! visitor's own computer by its one-use code or by signing the harness's
//! challenge with the vault owner key, keeps the session in
//! `localStorage` (`roostr-local-pairing`, shared with the Svelte app), and
//! sends it as `Authorization: Bearer` to the daemon and the harness; the
//! browser adds the page's `Origin`, which the session is bound to. Any 401
//! drops the session. The commit stream is a streamed `fetch` (EventSource
//! cannot send the header).

use std::cell::RefCell;
use std::pin::Pin;
use std::task::{Context, Poll};

use anyhow::{Result, anyhow, bail};
use futures::Stream;
use futures::channel::mpsc;
use js_sys::{Reflect, Uint8Array};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value as Json;
use wasm_bindgen::{JsCast as _, JsValue};
use wasm_bindgen_futures::JsFuture;
use web_sys::{AbortController, AbortSignal, RequestCredentials, RequestInit, RequestRedirect};

use crate::backend::{
    Backend, BackendEvent, JsonMap, SseParser, encode_path_segment, mutate_result,
};
use crate::io::{BoxFuture, BoxStream};
use crate::types::{Object, ObjectSummary, QueryResult, RelationDef, Settings, Space};

/// The daemon on the visitor's computer.
pub const LOCAL_API: &str = "http://127.0.0.1:7333";
/// Its harness.
pub const HARNESS_API: &str = crate::harness::DEFAULT_HARNESS;
const SESSION_KEY: &str = "roostr-local-pairing";

// ------------------------------------------------------------------- fetch

/// `fetch` itself rejected: nothing answered (or CORS refused the answer).
#[derive(Debug)]
pub struct NetworkError(pub String);

impl std::fmt::Display for NetworkError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for NetworkError {}

fn js_message(value: &JsValue) -> String {
    value
        .dyn_ref::<js_sys::Error>()
        .map(|e| String::from(e.message()))
        .or_else(|| value.as_string())
        .unwrap_or_else(|| format!("{value:?}"))
}

fn window() -> Result<web_sys::Window> {
    web_sys::window().ok_or_else(|| anyhow!("no browser window"))
}

/// One `fetch` (`credentials: "omit"`, `redirect: "error"`). A rejected fetch
/// is a [`NetworkError`]; any HTTP status resolves.
pub async fn fetch(
    method: &str,
    url: &str,
    headers: &[(&str, &str)],
    body: Option<&[u8]>,
    signal: Option<&AbortSignal>,
) -> Result<web_sys::Response> {
    let init = RequestInit::new();
    init.set_method(method);
    init.set_credentials(RequestCredentials::Omit);
    init.set_redirect(RequestRedirect::Error);
    if let Some(bytes) = body {
        init.set_body(&Uint8Array::from(bytes).into());
    }
    if let Some(signal) = signal {
        init.set_signal(Some(signal));
    }
    let request = web_sys::Request::new_with_str_and_init(url, &init)
        .map_err(|e| anyhow!("bad request to {url}: {}", js_message(&e)))?;
    for (name, value) in headers {
        request
            .headers()
            .set(name, value)
            .map_err(|e| anyhow!("bad header {name}: {}", js_message(&e)))?;
    }
    let answer = JsFuture::from(window()?.fetch_with_request(&request))
        .await
        .map_err(|e| anyhow::Error::new(NetworkError(js_message(&e))))?;
    answer
        .dyn_into::<web_sys::Response>()
        .map_err(|_| anyhow!("fetch did not answer a Response"))
}

/// The whole body.
pub async fn response_bytes(response: &web_sys::Response) -> Result<Vec<u8>> {
    let promise = response
        .array_buffer()
        .map_err(|e| anyhow!("unreadable response: {}", js_message(&e)))?;
    let buffer = JsFuture::from(promise)
        .await
        .map_err(|e| anyhow::Error::new(NetworkError(js_message(&e))))?;
    Ok(Uint8Array::new(&buffer).to_vec())
}

/// `AbortSignal.timeout(ms)`.
pub fn timeout_signal(ms: u32) -> Option<AbortSignal> {
    Some(AbortSignal::timeout_with_u32(ms))
}

// ----------------------------------------------------------------- pairing

/// A daemon-minted UI session for this page's origin.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PairedSession {
    pub token: String,
    /// Epoch ms.
    #[serde(rename = "expiresAt")]
    pub expires_at: f64,
    pub role: String,
}

fn storage() -> Option<web_sys::Storage> {
    web_sys::window()?.local_storage().ok()?
}

fn is_token(token: &str) -> bool {
    token.len() == 64 && token.bytes().all(|b| b.is_ascii_hexdigit())
}

fn valid(session: &PairedSession) -> bool {
    is_token(&session.token)
        && session.role == "ui"
        && session.expires_at.is_finite()
        && session.expires_at > js_sys::Date::now()
}

/// The stored session while it is well-formed and unexpired (`pairedSession`).
pub fn paired_session() -> Option<PairedSession> {
    let raw = storage()?.get_item(SESSION_KEY).ok()??;
    serde_json::from_str::<PairedSession>(&raw)
        .ok()
        .filter(valid)
}

thread_local! {
    static LISTENERS: RefCell<Vec<Box<dyn Fn()>>> = const { RefCell::new(Vec::new()) };
}

/// Call `callback` whenever this tab's pairing is stored or dropped, here
/// or (through the `storage` event) by another tab of the origin.
pub fn on_pairing_change(callback: impl Fn() + 'static) {
    let first = LISTENERS.with(|l| {
        let mut l = l.borrow_mut();
        l.push(Box::new(callback));
        l.len() == 1
    });
    if first && let Some(window) = web_sys::window() {
        let listener = wasm_bindgen::closure::Closure::<dyn Fn(web_sys::StorageEvent)>::new(
            |event: web_sys::StorageEvent| {
                if event.key().as_deref() == Some(SESSION_KEY) {
                    notify();
                }
            },
        );
        let _ =
            window.add_event_listener_with_callback("storage", listener.as_ref().unchecked_ref());
        // Lives as long as the page.
        listener.forget();
    }
}

fn notify() {
    // Listeners may register more listeners; call a snapshot count.
    let count = LISTENERS.with(|l| l.borrow().len());
    for index in 0..count {
        LISTENERS.with(|l| {
            if let Some(callback) = l.borrow().get(index) {
                callback();
            }
        });
    }
}

/// Forget the session (`unpairLocal`).
pub fn unpair() {
    if let Some(storage) = storage() {
        let _ = storage.remove_item(SESSION_KEY);
    }
    notify();
}

/// Keep a session the daemon minted, after checking its shape (`storeSession`).
fn store_session(value: &Json) -> Result<PairedSession> {
    let session = serde_json::from_value::<PairedSession>(value.clone())
        .ok()
        .filter(valid)
        .ok_or_else(|| anyhow!("The daemon returned an invalid pairing session."))?;
    let stored = serde_json::to_string(&session)?;
    storage()
        .ok_or_else(|| {
            anyhow!("This browser blocks local storage, so the pairing cannot be kept.")
        })?
        .set_item(SESSION_KEY, &stored)
        .map_err(|_| {
            anyhow!("This browser blocks local storage, so the pairing cannot be kept.")
        })?;
    notify();
    Ok(session)
}

/// Pair with the daemon's one-use code (`pairLocal`). Errors are the
/// sentences the pairing gate shows.
pub async fn pair(code: &str) -> Result<PairedSession> {
    let code = code.trim();
    if code.is_empty() {
        bail!("Enter the one-use code printed in your daemon terminal.");
    }
    let body = serde_json::to_vec(&serde_json::json!({ "code": code }))?;
    let response = fetch(
        "POST",
        &format!("{LOCAL_API}/api/pair"),
        &[("Content-Type", "application/json")],
        Some(&body),
        None,
    )
    .await
    .map_err(|_| anyhow!("Could not reach the local daemon. Start it and try again."))?;
    if !response.ok() {
        bail!(
            "Pairing failed. The code may have expired or already been used; obtain a new code from the daemon terminal."
        );
    }
    let value: Json = serde_json::from_slice(&response_bytes(&response).await?)
        .map_err(|_| anyhow!("The daemon returned an invalid pairing session."))?;
    store_session(&value)
}

/// `localStorage` key of the Nostr secret key (hex) a browser tab of this
/// origin signs in with (`src/lib/engine/keys.ts`), shared with the hosted
/// Svelte app at roostr.space/app.
const KEY_STORAGE: &str = "roostr-key";
/// NIP-98 HTTP auth: the harness accepts exactly this kind for owner pairing.
const OWNER_PAIR_KIND: u32 = 27235;

/// An nsec1… or 64-hex secret key (`parseKey`), with the Svelte app's error.
pub fn parse_key(input: &str) -> Result<roostr_nostr::Keys> {
    roostr_nostr::Keys::parse(input.trim())
        .map_err(|_| anyhow!("invalid key: expected nsec1… or 64 hex chars"))
}

/// The key this origin keeps (`loadKey`).
pub fn saved_key() -> Option<roostr_nostr::Keys> {
    let hex = storage()?.get_item(KEY_STORAGE).ok()??;
    roostr_nostr::Keys::parse(&hex).ok()
}

/// Keep `keys` as the origin's key (`saveKey`), unless the origin already
/// keeps one: that may be the hosted app's own identity, whose replica a
/// silent swap would mix with another key's.
fn keep_key(keys: &roostr_nostr::Keys) {
    let Some(storage) = storage() else {
        return;
    };
    if storage.get_item(KEY_STORAGE).ok().flatten().is_none() {
        let _ = storage.set_item(KEY_STORAGE, &keys.secret_hex());
    }
}

fn is_challenge(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// Pair by proof of ownership (`pairWithOwnerKey`): the harness issues a
/// one-use challenge bound to this origin, `keys` signs it together with the
/// origin and endpoint (NIP-98, kind 27235), and the harness - only if that
/// key is its own computer's identity - has the daemon mint the usual
/// origin-bound session. The key is then kept as the origin's key.
pub async fn pair_with_owner_key(keys: &roostr_nostr::Keys) -> Result<PairedSession> {
    let unreachable = |_| anyhow!("Roostr isn't running on this computer.");
    let issued = fetch(
        "GET",
        &format!("{HARNESS_API}/pair/challenge"),
        &[],
        None,
        None,
    )
    .await
    .map_err(unreachable)?;
    let challenge = response_bytes(&issued)
        .await
        .ok()
        .and_then(|body| serde_json::from_slice::<Json>(&body).ok())
        .and_then(|value| Some(value.get("challenge")?.as_str()?.to_string()))
        .filter(|challenge| issued.ok() && is_challenge(challenge));
    let Some(challenge) = challenge else {
        bail!(
            "Roostr on this computer did not offer pairing ({}).",
            issued.status()
        );
    };
    let url = format!("{HARNESS_API}/pair/owner");
    let origin = window()?
        .location()
        .origin()
        .map_err(|e| anyhow!("no page origin: {}", js_message(&e)))?;
    let event = keys.sign(roostr_nostr::EventTemplate {
        kind: OWNER_PAIR_KIND,
        created_at: (js_sys::Date::now() / 1000.).floor() as u64,
        tags: vec![
            vec!["u".into(), url.clone()],
            vec!["method".into(), "POST".into()],
            vec!["challenge".into(), challenge],
            vec!["origin".into(), origin],
        ],
        content: String::new(),
    });
    let body = serde_json::to_vec(&serde_json::json!({ "event": event }))?;
    let response = fetch(
        "POST",
        &url,
        &[("Content-Type", "application/json")],
        Some(&body),
        None,
    )
    .await
    .map_err(unreachable)?;
    let value: Json = match response_bytes(&response).await {
        Ok(body) => serde_json::from_slice(&body).unwrap_or(Json::Null),
        Err(_) => Json::Null,
    };
    if !response.ok() {
        let error = value
            .get("error")
            .and_then(Json::as_str)
            .filter(|error| !error.is_empty());
        bail!(match error {
            Some(error) => error.to_string(),
            None => format!(
                "Roostr on this computer refused pairing ({}).",
                response.status()
            ),
        });
    }
    let session = store_session(&value)?;
    keep_key(keys);
    Ok(session)
}

/// The daemon's current one-use code, which it serves only to a page on this
/// machine (`GET /api/pair/code`); `None` from roostr.space or an older daemon.
pub async fn daemon_code() -> Option<String> {
    let response = fetch(
        "GET",
        &format!("{LOCAL_API}/api/pair/code"),
        &[],
        None,
        None,
    )
    .await
    .ok()?;
    if !response.ok() {
        return None;
    }
    let value: Json = serde_json::from_slice(&response_bytes(&response).await.ok()?).ok()?;
    let code = value.get("code")?.as_str()?;
    is_token(code).then(|| code.to_string())
}

/// A request to the paired daemon or harness (`pairedFetch`): the session's
/// bearer token on a path under `base`; a 401 drops the pairing.
pub async fn paired_fetch(
    base: &str,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: Option<&[u8]>,
    signal: Option<&AbortSignal>,
) -> Result<web_sys::Response> {
    if !path.starts_with('/') || path.starts_with("//") || path.contains('\\') {
        bail!("Local API paths must be relative to the paired daemon.");
    }
    let session = paired_session().ok_or_else(|| {
        anyhow!("Pairing is missing or expired. Enter a fresh terminal code to reconnect.")
    })?;
    let auth = format!("Bearer {}", session.token);
    let mut all: Vec<(&str, &str)> = headers.to_vec();
    all.push(("Authorization", &auth));
    let response = fetch(method, &format!("{base}{path}"), &all, body, signal).await?;
    if paired_session().map(|s| s.token) != Some(session.token) {
        if let Some(stream) = response.body() {
            let _ = stream.cancel();
        }
        bail!("The pairing changed while this request was in flight. Reconnect and try again.");
    }
    if response.status() == 401 {
        unpair();
        bail!("The daemon rejected this pairing. Enter a fresh terminal code to reconnect.");
    }
    Ok(response)
}

// ----------------------------------------------------------------- backend

/// [`Backend`] over fetch to the paired daemon.
pub struct WebDaemonBackend {
    base_url: String,
}

impl Default for WebDaemonBackend {
    fn default() -> Self {
        Self {
            base_url: LOCAL_API.to_string(),
        }
    }
}

/// A rejected fetch to the daemon, as the desktop words it.
fn unreachable(base: &str, error: anyhow::Error) -> anyhow::Error {
    match error.downcast_ref::<NetworkError>() {
        Some(network) => anyhow!(
            "Cannot reach the Roostr daemon at {base}: {network}. Is `glon-odin serve` running?"
        ),
        None => error,
    }
}

impl WebDaemonBackend {
    fn request<T: DeserializeOwned + 'static>(
        &self,
        method: &'static str,
        path: String,
        body: Option<Json>,
    ) -> BoxFuture<Result<T>> {
        let base = self.base_url.clone();
        Box::pin(async move {
            let bytes = match &body {
                Some(body) => Some(serde_json::to_vec(body)?),
                None => None,
            };
            let headers: &[(&str, &str)] = if bytes.is_some() {
                &[("Content-Type", "application/json")]
            } else {
                &[]
            };
            let response = paired_fetch(&base, method, &path, headers, bytes.as_deref(), None)
                .await
                .map_err(|e| unreachable(&base, e))?;
            let status = response.status();
            let text = String::from_utf8_lossy(&response_bytes(&response).await?).into_owned();
            if status == 403 {
                bail!("The daemon refused this tab's pairing ({status}).");
            }
            if !(200..300).contains(&status) {
                let detail = serde_json::from_str::<Json>(&text)
                    .ok()
                    .and_then(|j| j.get("error").and_then(Json::as_str).map(str::to_string))
                    .unwrap_or(text);
                bail!("{path}: {status} {detail}");
            }
            serde_json::from_str(&text).map_err(|_| anyhow!("{path}: unexpected response"))
        })
    }

    fn get<T: DeserializeOwned + 'static>(&self, path: impl Into<String>) -> BoxFuture<Result<T>> {
        self.request("GET", path.into(), None)
    }

    fn post<T: DeserializeOwned + 'static>(
        &self,
        path: impl Into<String>,
        body: Json,
    ) -> BoxFuture<Result<T>> {
        self.request("POST", path.into(), Some(body))
    }
}

impl Backend for WebDaemonBackend {
    fn describe(&self) -> String {
        format!("glon-odin at {}", self.base_url)
    }

    fn settings(&self) -> BoxFuture<Result<Settings>> {
        self.get("/api/settings")
    }

    fn objects(&self) -> BoxFuture<Result<Vec<ObjectSummary>>> {
        self.get("/api/objects")
    }

    fn object(&self, id: &str) -> BoxFuture<Result<Object>> {
        self.get(format!("/api/objects/{}", encode_path_segment(id)))
    }

    fn channels(&self) -> BoxFuture<Result<Vec<Space>>> {
        self.get("/api/channels")
    }

    fn relations(&self) -> BoxFuture<Result<Vec<RelationDef>>> {
        self.get("/api/relations")
    }

    fn query(&self, body: &Json) -> BoxFuture<Result<QueryResult>> {
        self.post("/api/query", body.clone())
    }

    fn mutate(&self, action: &str, mut params: JsonMap) -> BoxFuture<Result<JsonMap>> {
        let action = action.to_string();
        params.insert("action".into(), Json::String(action.clone()));
        let request = self.post::<JsonMap>("/api/mutate", Json::Object(params));
        Box::pin(async move { mutate_result(&action, request.await?) })
    }

    fn serving(&self, object_ids: &[String]) -> BoxFuture<Result<JsonMap>> {
        self.post(
            "/api/serving",
            serde_json::json!({ "objectIds": object_ids }),
        )
    }

    fn sync_digest(&self) -> BoxFuture<Result<JsonMap>> {
        self.get("/api/sync/digest")
    }

    /// A task on the page's event loop reads the streamed body into a
    /// channel; dropping the stream aborts the fetch.
    fn events(&self) -> BoxStream<Result<BackendEvent>> {
        let (tx, rx) = mpsc::unbounded::<Result<BackendEvent>>();
        let controller = AbortController::new().ok();
        let signal = controller.as_ref().map(AbortController::signal);
        let base = self.base_url.clone();
        wasm_bindgen_futures::spawn_local(async move {
            let result = read_events(&base, signal.as_ref(), &tx).await;
            if let Err(error) = result {
                let _ = tx.unbounded_send(Err(error));
            }
        });
        Box::pin(AbortOnDrop { rx, controller })
    }
}

/// Reads `/api/events` until it fails, closes, or the receiver goes away.
async fn read_events(
    base: &str,
    signal: Option<&AbortSignal>,
    tx: &mpsc::UnboundedSender<Result<BackendEvent>>,
) -> Result<()> {
    let response = paired_fetch(
        base,
        "GET",
        "/api/events",
        &[("Accept", "text/event-stream")],
        None,
        signal,
    )
    .await
    .map_err(|e| unreachable(base, e))?;
    let status = response.status();
    let Some(body) = response.body().filter(|_| response.ok()) else {
        bail!("/api/events: {status}");
    };
    let reader = body
        .get_reader()
        .dyn_into::<web_sys::ReadableStreamDefaultReader>()
        .map_err(|_| anyhow!("/api/events: unreadable stream"))?;
    let mut parser = SseParser::default();
    let mut pending: Vec<u8> = Vec::new();
    loop {
        let chunk = JsFuture::from(reader.read())
            .await
            .map_err(|e| anyhow!("Local event stream disconnected: {}", js_message(&e)))?;
        if Reflect::get(&chunk, &"done".into())
            .ok()
            .and_then(|d| d.as_bool())
            == Some(true)
        {
            bail!("The daemon closed the event stream");
        }
        let Ok(value) = Reflect::get(&chunk, &"value".into()) else {
            continue;
        };
        pending.extend(Uint8Array::new(&value).to_vec());
        while let Some(newline) = pending.iter().position(|&b| b == b'\n') {
            let line: Vec<u8> = pending.drain(..=newline).collect();
            let line = String::from_utf8_lossy(&line[..line.len() - 1]);
            if let Some(event) = parser.line(&line)
                && tx.unbounded_send(Ok(event)).is_err()
            {
                let _ = reader.cancel();
                return Ok(());
            }
        }
    }
}

struct AbortOnDrop {
    rx: mpsc::UnboundedReceiver<Result<BackendEvent>>,
    controller: Option<AbortController>,
}

impl Stream for AbortOnDrop {
    type Item = Result<BackendEvent>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        Pin::new(&mut self.rx).poll_next(cx)
    }
}

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        if let Some(controller) = &self.controller {
            controller.abort();
        }
    }
}

// ------------------------------------------------------------ the browser

/// Save `bytes` as a download named `name` (an `<a download>` click on a
/// Blob URL).
pub fn download(bytes: &[u8], name: &str, mime: &str) -> Result<()> {
    let parts = js_sys::Array::of1(&Uint8Array::from(bytes).into());
    let options = web_sys::BlobPropertyBag::new();
    options.set_type(mime);
    let blob = web_sys::Blob::new_with_u8_array_sequence_and_options(&parts, &options)
        .map_err(|e| anyhow!("could not build the download: {}", js_message(&e)))?;
    let url = web_sys::Url::create_object_url_with_blob(&blob)
        .map_err(|e| anyhow!("could not build the download: {}", js_message(&e)))?;
    let document = window()?.document().ok_or_else(|| anyhow!("no document"))?;
    let anchor = document
        .create_element("a")
        .map_err(|e| anyhow!("{}", js_message(&e)))?
        .dyn_into::<web_sys::HtmlAnchorElement>()
        .map_err(|_| anyhow!("no anchor element"))?;
    anchor.set_href(&url);
    anchor.set_download(name);
    anchor.click();
    // The click has started the download; the URL can go once it has been read.
    let revoke = wasm_bindgen::closure::Closure::once_into_js(move || {
        let _ = web_sys::Url::revoke_object_url(&url);
    });
    let _ = window()?
        .set_timeout_with_callback_and_timeout_and_arguments_0(revoke.unchecked_ref(), 60_000);
    Ok(())
}

/// Open `bytes` in a new browser tab (the system viewer's stand-in).
pub fn open_bytes(bytes: &[u8], mime: &str) -> Result<()> {
    let parts = js_sys::Array::of1(&Uint8Array::from(bytes).into());
    let options = web_sys::BlobPropertyBag::new();
    options.set_type(mime);
    let blob = web_sys::Blob::new_with_u8_array_sequence_and_options(&parts, &options)
        .map_err(|e| anyhow!("could not open the file: {}", js_message(&e)))?;
    let url = web_sys::Url::create_object_url_with_blob(&blob)
        .map_err(|e| anyhow!("could not open the file: {}", js_message(&e)))?;
    window()?
        .open_with_url_and_target(&url, "_blank")
        .map_err(|e| anyhow!("could not open the file: {}", js_message(&e)))?;
    Ok(())
}

/// A request to any URL (a remote image host, Blossom, oEmbed) for
/// [`crate::http`].
pub(crate) async fn request(
    method: &str,
    url: &str,
    headers: &[(&str, &str)],
    body: Option<&[u8]>,
    timeout_ms: Option<u32>,
) -> Result<crate::http::Response> {
    let signal = timeout_ms.and_then(timeout_signal);
    let response = fetch(method, url, headers, body, signal.as_ref()).await?;
    let mut header_list = Vec::new();
    let pairs = js_sys::Array::from(&response.headers());
    for pair in pairs.iter() {
        let pair = js_sys::Array::from(&pair);
        if let (Some(name), Some(value)) = (pair.get(0).as_string(), pair.get(1).as_string()) {
            header_list.push((name, value));
        }
    }
    Ok(crate::http::Response {
        status: response.status(),
        reason: response.status_text(),
        headers: header_list,
        body: response_bytes(&response).await?,
    })
}
