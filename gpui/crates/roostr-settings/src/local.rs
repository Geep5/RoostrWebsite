//! Calls to this computer's harness that the settings screens make
//! (`client-profile.ts`, `client-identity.ts`, Settings' `/auth/*` and
//! `/claude-plan*`), with their wire shapes.

use anyhow::{Result, anyhow};
use gpui_kit::{App, Task};
use roostr_core::harness::harness;
use roostr_core::io;
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::Value as Json;

/// `GET path` on the harness, decoded. Fails when this app has no harness.
pub fn get<T: DeserializeOwned + Send + 'static>(
    path: impl Into<String>,
    cx: &App,
) -> Task<Result<T>> {
    let path = path.into();
    let harness = harness(cx);
    io::spawn(cx, async move {
        let harness = harness.ok_or_else(|| anyhow!("No local daemon for this app."))?;
        Ok(serde_json::from_value(harness.get_json(&path).await?)?)
    })
}

/// `POST path` with a JSON body on the harness, decoded.
pub fn post<T: DeserializeOwned + Send + 'static>(
    path: impl Into<String>,
    body: Json,
    cx: &App,
) -> Task<Result<T>> {
    let path = path.into();
    let harness = harness(cx);
    io::spawn(cx, async move {
        let harness = harness.ok_or_else(|| anyhow!("No local daemon for this app."))?;
        Ok(serde_json::from_value(
            harness.post_json(&path, &body).await?,
        )?)
    })
}

/// One agent provider's credential state on this computer.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct ProviderStatus {
    #[serde(default)]
    pub mode: String,
    #[serde(default)]
    pub masked: Option<String>,
}

/// `GET /auth/status`.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct AgentAuth {
    #[serde(default)]
    pub kimi: ProviderStatus,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct PlanSpace {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub auth: String,
    #[serde(default)]
    pub error: String,
}

/// The Claude plan, space-wide (harness `claude-plan.ts`).
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanStatus {
    #[serde(default)]
    pub this_computer: String,
    #[serde(default)]
    pub account: String,
    #[serde(default)]
    pub served_by: String,
    #[serde(default)]
    pub signing_in: bool,
    #[serde(default)]
    pub sign_in_error: String,
    #[serde(default)]
    pub spaces: Vec<PlanSpace>,
}

/// A pending request to join one of this identity's spaces.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JoinRequest {
    pub key: String,
    pub space: String,
    pub requester_npub: String,
    /// Epoch ms (may arrive as a float).
    pub at: f64,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub picture: Option<String>,
}

#[derive(Deserialize)]
struct JoinRequests {
    requests: Vec<JoinRequest>,
}

/// `listJoinRequests`: every pending request this daemon (or, in replica
/// mode, this device) holds.
pub fn list_join_requests(cx: &App) -> Task<Result<Vec<JoinRequest>>> {
    let harness = harness(cx);
    let account = roostr_core::account::account(cx);
    io::spawn(cx, async move {
        if let Some(account) = account {
            return account
                .join_requests()
                .await?
                .into_iter()
                .map(|row| serde_json::from_value(row).map_err(|_| anyhow!("Invalid join request")))
                .collect();
        }
        let harness = harness.ok_or_else(|| anyhow!("No local daemon for this app."))?;
        let value = harness
            .get_json("/join-requests")
            .await
            .map_err(|e| with_status(e, |s| format!("Join requests unavailable ({s})")))?;
        let parsed: JoinRequests =
            serde_json::from_value(value).map_err(|_| anyhow!("Invalid join requests response"))?;
        Ok(parsed.requests)
    })
}

/// `clearJoinRequest`: dismiss (or finish) one request.
pub fn clear_join_request(key: &str, cx: &App) -> Task<Result<()>> {
    let harness = harness(cx);
    if let Some(account) = roostr_core::account::account(cx) {
        return io::spawn(cx, account.clear_join_request(key));
    }
    let body = serde_json::json!({ "key": key });
    io::spawn(cx, async move {
        let harness = harness.ok_or_else(|| anyhow!("No local daemon for this app."))?;
        harness
            .post_json("/join-requests/clear", &body)
            .await
            .map_err(|e| with_status(e, |s| format!("Cannot clear join request ({s})")))?;
        Ok(())
    })
}

/// A non-2xx answer as the caller's sentence for its status; a transport
/// failure keeps its own message (what `fetch` throws).
pub fn with_status(error: anyhow::Error, sentence: impl FnOnce(u16) -> String) -> anyhow::Error {
    match error.downcast_ref::<roostr_core::harness::HarnessError>() {
        Some(e) => anyhow!(sentence(e.status)),
        None => error,
    }
}
