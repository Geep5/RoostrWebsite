//! Client for the agent harness on this computer (`http://127.0.0.1:7334`,
//! `glonOdin/harness/src/authserver.ts`): credentials, capabilities, files,
//! workspace pickers, agent status.
//!
//! The harness authorizes through the daemon. Several routes accept only a
//! UI session, so the app mints one with the daemon service token
//! (`POST /api/local-auth/session {origin}`) and sends it with a fixed
//! loopback `Origin`. Sessions last 24 h and the daemon holds only 64, so one
//! is kept in memory and on disk and re-minted only on expiry or a 401.
//!
//! In the browser the tab's pairing session ([`crate::web`]) is already a UI
//! session for the page's origin, which the browser sends.
//!
//! Requests are futures; run them with [`crate::io::spawn`] (they block on
//! the desktop).

use std::sync::Arc;
#[cfg(not(target_family = "wasm"))]
use std::{path::PathBuf, sync::Mutex, time::Duration};

use anyhow::Result;
#[cfg(not(target_family = "wasm"))]
use anyhow::{Context as _, anyhow, bail};
use gpui_kit::{App, Global};
#[cfg(not(target_family = "wasm"))]
use serde::{Deserialize, Serialize};
use serde_json::Value as Json;
#[cfg(not(target_family = "wasm"))]
use ureq::Agent;

#[cfg(not(target_family = "wasm"))]
use crate::daemon::DaemonConfig;

pub const DEFAULT_HARNESS: &str = "http://127.0.0.1:7334";
/// The loopback origin this app's UI sessions are bound to.
#[cfg(not(target_family = "wasm"))]
pub const APP_ORIGIN: &str = "http://127.0.0.1:7390";
#[cfg(not(target_family = "wasm"))]
const BODY_LIMIT: u64 = 2 * 1024 * 1024 * 1024;

#[cfg(not(target_family = "wasm"))]
#[derive(Clone, Debug, Serialize, Deserialize)]
struct Session {
    token: String,
    /// Epoch ms.
    expires_at: i64,
}

pub struct Harness {
    base_url: String,
    #[cfg(not(target_family = "wasm"))]
    daemon: DaemonConfig,
    #[cfg(not(target_family = "wasm"))]
    agent: Agent,
    #[cfg(not(target_family = "wasm"))]
    session: Mutex<Option<Session>>,
    #[cfg(not(target_family = "wasm"))]
    session_path: Option<PathBuf>,
}

/// The harness client, when this app talks to a local daemon.
pub struct GlobalHarness(pub Option<Arc<Harness>>);

impl Global for GlobalHarness {}

/// The harness for this computer, if any (none for a daemon-less backend).
pub fn harness(cx: &App) -> Option<Arc<Harness>> {
    cx.try_global::<GlobalHarness>().and_then(|g| g.0.clone())
}

/// A request that failed because nothing listens on the harness port.
pub fn is_unreachable(error: &anyhow::Error) -> bool {
    error.downcast_ref::<Unreachable>().is_some()
}

#[derive(Debug)]
pub struct Unreachable(pub String);

impl std::fmt::Display for Unreachable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Roostr isn't running on this computer ({}).", self.0)
    }
}

impl std::error::Error for Unreachable {}

#[cfg(target_family = "wasm")]
impl Harness {
    /// The harness on the visitor's computer, authorized by the tab's pairing.
    pub fn web() -> Self {
        Self {
            base_url: DEFAULT_HARNESS.to_string(),
        }
    }

    /// One request with the pairing session.
    async fn send(
        &self,
        method: &str,
        path: &str,
        body: Option<(&[u8], &str)>,
    ) -> Result<(u16, Vec<u8>)> {
        let mut headers = Vec::new();
        if let Some((_, content_type)) = body {
            headers.push(("Content-Type", content_type));
        }
        let response = crate::web::paired_fetch(
            &self.base_url,
            method,
            path,
            &headers,
            body.map(|(bytes, _)| bytes),
            None,
        )
        .await
        .map_err(|error| match error.downcast::<crate::web::NetworkError>() {
            Ok(network) => anyhow::Error::new(Unreachable(network.0)),
            Err(error) => error,
        })?;
        let status = response.status();
        let bytes = crate::web::response_bytes(&response).await?;
        Ok((status, bytes))
    }
}

#[cfg(not(target_family = "wasm"))]
impl Harness {
    /// `ROOSTR_HARNESS` (default `http://127.0.0.1:7334`).
    pub fn new(daemon: DaemonConfig) -> Self {
        let base_url = std::env::var("ROOSTR_HARNESS")
            .unwrap_or_else(|_| DEFAULT_HARNESS.to_string())
            .trim_end_matches('/')
            .to_string();
        let agent = Agent::new_with_config(
            Agent::config_builder()
                .http_status_as_error(false)
                .timeout_connect(Some(Duration::from_secs(3)))
                // File downloads may wait on peer-to-peer transfer.
                .timeout_global(Some(Duration::from_secs(300)))
                .build(),
        );
        let session_path =
            dirs::config_dir().map(|d| d.join("Roostr").join("harness-session.json"));
        let session = session_path
            .as_ref()
            .and_then(|p| std::fs::read(p).ok())
            .and_then(|b| serde_json::from_slice::<Session>(&b).ok());
        Self {
            base_url,
            daemon,
            agent,
            session: Mutex::new(session),
            session_path,
        }
    }

    /// A live UI session token, minting one when absent or expired.
    fn session_token(&self, force_new: bool) -> Result<String> {
        let now = chrono::Utc::now().timestamp_millis();
        let mut guard = self.session.lock().expect("session lock");
        if !force_new
            && let Some(session) = guard.as_ref()
            && session.expires_at > now + 60_000
        {
            return Ok(session.token.clone());
        }
        let mut response = self
            .agent
            .post(format!("{}/api/local-auth/session", self.daemon.base_url))
            .header("Authorization", format!("Bearer {}", self.daemon.token))
            .send_json(serde_json::json!({ "origin": APP_ORIGIN }))
            .map_err(|e| {
                anyhow!(
                    "Cannot reach the Roostr daemon at {}: {e}",
                    self.daemon.base_url
                )
            })?;
        let status = response.status();
        let body: Json = response
            .body_mut()
            .read_json()
            .context("local-auth/session: unexpected response")?;
        if !status.is_success() {
            bail!(
                "The daemon would not open a harness session ({status}): {}",
                body.get("error").and_then(Json::as_str).unwrap_or("")
            );
        }
        let token = body
            .get("token")
            .and_then(Json::as_str)
            .ok_or_else(|| anyhow!("local-auth/session: no token"))?;
        // `expiresAt` arrives as a float with a long fraction.
        let expires_at = body.get("expiresAt").and_then(Json::as_f64).unwrap_or(0.) as i64;
        let session = Session {
            token: token.to_string(),
            expires_at,
        };
        if let Some(path) = &self.session_path
            && let Ok(bytes) = serde_json::to_vec(&session)
        {
            if let Some(dir) = path.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            let _ = std::fs::write(path, bytes);
        }
        *guard = Some(session);
        Ok(token.to_string())
    }

    /// One request with the UI session; a 401 re-mints the session once.
    /// Blocks: polled on a background thread through [`crate::io::spawn`].
    async fn send(
        &self,
        method: &str,
        path: &str,
        body: Option<(&[u8], &str)>,
    ) -> Result<(u16, Vec<u8>)> {
        let mut attempt = 0;
        loop {
            let token = self.session_token(attempt > 0)?;
            let url = format!("{}{}", self.base_url, path);
            let auth = format!("Bearer {token}");
            let result = match (method, body) {
                ("GET", _) => self
                    .agent
                    .get(&url)
                    .header("Authorization", &auth)
                    .header("Origin", APP_ORIGIN)
                    .call(),
                (_, Some((bytes, content_type))) => self
                    .agent
                    .post(&url)
                    .header("Authorization", &auth)
                    .header("Origin", APP_ORIGIN)
                    .header("Content-Type", content_type)
                    .send(bytes),
                (_, None) => self
                    .agent
                    .post(&url)
                    .header("Authorization", &auth)
                    .header("Origin", APP_ORIGIN)
                    .send_empty(),
            };
            let mut response =
                result.map_err(|e| anyhow::Error::new(Unreachable(e.to_string())))?;
            let status = response.status().as_u16();
            let bytes = response
                .body_mut()
                .with_config()
                .limit(BODY_LIMIT)
                .read_to_vec()?;
            if status == 401 && attempt == 0 {
                attempt += 1;
                continue;
            }
            return Ok((status, bytes));
        }
    }
}

impl Harness {
    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    fn json_result(path: &str, status: u16, bytes: &[u8]) -> Result<Json> {
        let json: Json = serde_json::from_slice(bytes)
            .unwrap_or_else(|_| Json::String(String::from_utf8_lossy(bytes).into_owned()));
        if !(200..300).contains(&status) {
            let message = json
                .get("error")
                .and_then(Json::as_str)
                .map(str::to_string)
                .or_else(|| json.as_str().map(str::to_string))
                .unwrap_or_else(|| format!("{path}: HTTP {status}"));
            return Err(HarnessError { status, message }.into());
        }
        Ok(json)
    }

    pub async fn get_json(&self, path: &str) -> Result<Json> {
        let (status, bytes) = self.send("GET", path, None).await?;
        Self::json_result(path, status, &bytes)
    }

    pub async fn post_json(&self, path: &str, body: &Json) -> Result<Json> {
        let bytes = serde_json::to_vec(body)?;
        let (status, out) = self
            .send("POST", path, Some((&bytes, "application/json")))
            .await?;
        Self::json_result(path, status, &out)
    }

    /// Raw upload (e.g. `POST /files?name=&mime=&space=`).
    pub async fn post_bytes(&self, path: &str, bytes: &[u8], content_type: &str) -> Result<Json> {
        let (status, out) = self.send("POST", path, Some((bytes, content_type))).await?;
        Self::json_result(path, status, &out)
    }

    /// Raw download (e.g. `GET /files/<hash>`).
    pub async fn get_bytes(&self, path: &str) -> Result<Vec<u8>> {
        let (status, bytes) = self.send("GET", path, None).await?;
        if !(200..300).contains(&status) {
            Self::json_result(path, status, &bytes)?;
        }
        Ok(bytes)
    }
}

/// A harness answer that was not 2xx, with its `{error}` sentence.
#[derive(Debug)]
pub struct HarnessError {
    pub status: u16,
    pub message: String,
}

impl std::fmt::Display for HarnessError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for HarnessError {}
