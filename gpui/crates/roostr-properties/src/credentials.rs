//! A credential's harness actions (`credential-actions.ts`) and the progress
//! of reconnects and Checks started in this window
//! (`credential-reconnect.svelte.ts`): table cells, cards, the cell popover
//! and the Properties pane's Status row all read the same progress, and it
//! outlives the popover.
//!
//! Connect runs on this computer's harness: the sign-in window opens here
//! and the login is saved on the credential. Check works from any device:
//! it writes `check_requested` and the computer serving the credential
//! stamps `checked_at` (a window on that computer asks it straight).

use std::collections::HashMap;
use std::time::Duration;

use anyhow::{Result, anyhow};
use gpui_kit::*;
use roostr_core::harness::{Unreachable, harness};
use roostr_core::io;
use roostr_core::types::field_str;
use roostr_core::{Fields, Glyph, QueryRow, Store, Value, api};
use serde_json::{Value as Json, json};

pub(crate) const POLL: Duration = Duration::from_millis(3_000);
pub(crate) const CONNECT_TIMEOUT_MS: i64 = 5 * 60_000;
/// The fields the owning harness rewrites while a sign-in runs.
const HARNESS_FIELDS: [&str; 4] = ["status", "error", "checked_at", "auth"];
pub(crate) const CONNECT_TIMEOUT_TEXT: &str =
    "Still not signed in after 5 minutes. Connect again when you are ready.";
const CHECK_REQUEST_KEY: &str = "check_requested";
const CHECK_POLL: Duration = Duration::from_millis(2_500);
const CHECK_TIMEOUT_MS: i64 = 60_000;
pub(crate) const GOOGLE_ACCOUNT: &str = "google-account";
pub(crate) const COOKIE_PREFIX: &str = "cookie_";
pub(crate) const COOKIE_META: &str = "cookie_meta";
/// The harness's Error badge on an object whose run hit a dead login.
const SIGNED_OUT_PREFIX: &str = "credential signed out: ";

pub(crate) fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

/// What a credential cell reconnects.
#[derive(Clone, Debug, PartialEq)]
pub enum CredentialTarget {
    /// That credential (a credential row's Status or Error).
    Credential(String),
    /// The credentials another object's signed-out Error names.
    Error(SharedString),
}

/// The credential's status in words.
pub fn credential_status_text(status: &str) -> String {
    match if status.is_empty() { "missing" } else { status } {
        "active" => "Connected".into(),
        "connecting" => "Waiting for sign-in".into(),
        "needs_auth" => "Signed out - reconnect".into(),
        "broken" => "Broken".into(),
        "missing" => "Not connected".into(),
        other => other.replace('_', " "),
    }
}

/// The status badge: glyph and options palette color name ("" = neutral).
pub fn credential_status_badge(status: &str) -> (Glyph, &'static str) {
    match if status.is_empty() { "missing" } else { status } {
        "active" => (Glyph::BadgeCheck, "lime"),
        "connecting" | "needs_auth" => (Glyph::BadgeClock, "orange"),
        "broken" => (Glyph::BadgeX, "red"),
        _ => (Glyph::BadgeDashed, ""),
    }
}

/// Connect can sign this in: a Google account, or a browser sign-in with its login page and proof cookie.
pub(crate) fn can_connect(fields: &Fields) -> bool {
    if field_str(fields, "service") == GOOGLE_ACCOUNT {
        return true;
    }
    ["login_url", "session_host", "session_cookie"]
        .iter()
        .all(|k| !field_str(fields, k).trim().is_empty())
}

/// A pasted key (`key_*`, not `key_fields`) or a saved cookie value (`cookie_*`, not `cookie_meta`).
pub(crate) fn is_secret_field(key: &str) -> bool {
    if let Some(rest) = key.strip_prefix("key_") {
        return !rest.is_empty() && key != "key_fields";
    }
    key.starts_with(COOKIE_PREFIX) && key.len() > COOKIE_PREFIX.len() && key != COOKIE_META
}

pub(crate) fn connect_label(status: &str) -> &'static str {
    if status == "active" || status == "needs_auth" {
        "Reconnect"
    } else {
        "Connect"
    }
}

/// What this window may run for a credential, and what to say about the rest.
pub(crate) struct Gate {
    pub can_connect: bool,
    pub can_check: bool,
    pub check_here: bool,
    pub reason: String,
}

pub(crate) fn action_gate(
    paired: bool,
    this_machine: &str,
    served_by: &str,
    computer: &str,
    fields: &Fields,
) -> Gate {
    let gate = |can_connect, can_check, check_here, reason: String| Gate {
        can_connect,
        can_check,
        check_here,
        reason,
    };
    if served_by.is_empty() {
        return gate(
            false,
            false,
            false,
            "Set Served by to a computer first.".into(),
        );
    }
    let linked = paired && !this_machine.is_empty();
    if linked && this_machine == served_by {
        return gate(true, true, true, String::new());
    }
    if !can_connect(fields) {
        return gate(
            false,
            true,
            false,
            format!("Open Roostr on {computer} to connect this credential."),
        );
    }
    if linked {
        return gate(true, true, false, String::new());
    }
    gate(
        false,
        true,
        false,
        "Pair this tab with one of your computers to sign in there.".into(),
    )
}

pub(crate) fn connecting_hint(fields: &Fields, place: &str, hands_to: &str) -> String {
    let place = if place.is_empty() {
        "its computer"
    } else {
        place
    };
    let then = if hands_to.is_empty() {
        String::new()
    } else {
        format!("; Roostr then hands the login to {hands_to}")
    };
    if field_str(fields, "service") == GOOGLE_ACCOUNT {
        let account = field_str(fields, "account");
        let account = if account.is_empty() {
            "the Google account"
        } else {
            account
        };
        format!("Sign in to {account} in the browser window gws opens on {place}{then}.")
    } else {
        format!("Sign in in the Chrome window on {place}{then}.")
    }
}

fn field_num(fields: &Fields, key: &str) -> i64 {
    fields.get(key).and_then(Value::as_number).unwrap_or(0.) as i64
}

/// A Check asked for and not yet answered (under a minute old): its time, or 0.
pub(crate) fn pending_check(fields: &Fields, now: i64) -> i64 {
    let at = field_num(fields, CHECK_REQUEST_KEY);
    if at > field_num(fields, "checked_at") && now - at < CHECK_TIMEOUT_MS {
        at
    } else {
        0
    }
}

fn check_answered(fields: &Fields, requested_at: i64, before: i64) -> bool {
    let at = field_num(fields, "checked_at");
    at >= requested_at || at != before
}

/// The harness's answer.
pub(crate) struct ActionResult {
    pub status: String,
    pub error: String,
}

/// POST `/credentials/<action>`; a body with `error` and no `status` is a refusal.
pub(crate) fn run_action(action: &str, id: &str, cx: &App) -> Task<Result<ActionResult>> {
    let Some(h) = harness(cx) else {
        return Task::ready(Err(anyhow!(Unreachable("no harness".into()))));
    };
    let path = format!("/credentials/{action}");
    let body = json!({ "id": id });
    io::spawn(cx, async move {
        let answer = h.post_json(&path, &body).await?;
        let status = answer
            .get("status")
            .and_then(Json::as_str)
            .unwrap_or("")
            .to_string();
        let error = answer
            .get("error")
            .and_then(Json::as_str)
            .unwrap_or("")
            .to_string();
        if !error.is_empty() && status.is_empty() {
            return Err(anyhow!(error));
        }
        Ok(ActionResult { status, error })
    })
}

/// The credential's fields, fetched fresh.
pub(crate) fn read_fields(id: &str, cx: &App) -> Task<Option<Fields>> {
    let task = Store::global(cx).read(cx).fetch_object(id, cx);
    cx.background_spawn(async move { task.await.ok().map(|o| o.fields) })
}

/// Let the views re-read the credential (`onchanged`).
pub(crate) fn changed(id: &str, cx: &mut App) {
    let id = id.to_string();
    Store::global(cx).update(cx, |store, cx| {
        if store.object(&id).is_some() {
            store.reload_object(&id, cx);
        }
    });
}

// ── Signed-out errors on other objects ────────────────────────────────

fn signed_out_list(error: &str) -> Option<String> {
    let rest = error.strip_prefix(SIGNED_OUT_PREFIX)?;
    let rest = rest
        .strip_suffix(" - reconnect it")
        .or_else(|| rest.strip_suffix(" - reconnect them"))
        .unwrap_or(rest);
    Some(rest.to_string())
}

/// The credential names a signed-out badge lists ([] for any other Error).
pub(crate) fn signed_out_names(error: &str) -> Vec<String> {
    signed_out_list(error)
        .unwrap_or_default()
        .split(", ")
        .map(|n| n.trim().to_string())
        .filter(|n| !n.is_empty())
        .collect()
}

/// Credential ids an object lists in its Credentials property.
pub(crate) fn linked_credential_ids(fields: &Fields) -> Vec<String> {
    let Some(v) = fields.get("credentials") else {
        return vec![];
    };
    let single = [v.clone()];
    let items: &[Value] = if v.values_value.is_some() {
        v.items()
    } else {
        &single
    };
    items
        .iter()
        .map(|i| {
            i.link_value
                .as_ref()
                .map(|l| l.target_id.clone())
                .filter(|t| !t.is_empty())
                .or_else(|| i.string_value.clone())
                .unwrap_or_default()
        })
        .filter(|s| !s.is_empty())
        .collect()
}

#[derive(Clone, Debug)]
pub(crate) struct CredentialRef {
    pub id: String,
    pub name: String,
}

/// The credentials a signed-out badge means, in the badge's order: names
/// looked up among the object's own linked credentials first.
pub(crate) fn credentials_for_error(
    error: &str,
    linked: &[CredentialRef],
    all: &[CredentialRef],
) -> Vec<CredentialRef> {
    let Some(list) = signed_out_list(error) else {
        return vec![];
    };
    let hay = format!(", {list}, ");
    let at = |c: &CredentialRef| -> Option<usize> {
        if c.name.is_empty() {
            None
        } else {
            hay.find(&format!(", {}, ", c.name))
        }
    };
    let mine: Vec<CredentialRef> = linked.iter().filter(|c| at(c).is_some()).cloned().collect();
    let mut found = mine.clone();
    found.extend(
        all.iter()
            .filter(|c| {
                at(c).is_some()
                    && !mine.iter().any(|m| m.name == c.name)
                    && !mine.iter().any(|m| m.id == c.id)
            })
            .cloned(),
    );
    // "Acme, Inc" listed is that one credential, not also "Acme" and "Inc".
    let inside = |c: &CredentialRef| {
        let ac = at(c).unwrap_or(0);
        found.iter().any(|d| {
            let ad = at(d).unwrap_or(0);
            d.name.len() > c.name.len() && ad <= ac && ac + c.name.len() <= ad + d.name.len()
        })
    };
    let mut out: Vec<CredentialRef> = found.iter().filter(|c| !inside(c)).cloned().collect();
    out.sort_by_key(|c| at(c).unwrap_or(usize::MAX));
    out
}

/// What a view cell can reconnect: a credential row's Status or Error while it
/// is not connected, or another object's signed-out Error. None otherwise.
pub fn reconnect_target(row: &QueryRow, key: &str) -> Option<CredentialTarget> {
    if row.type_key == "credential" {
        if key != "status" && key != "error" {
            return None;
        }
        let status = field_str(&row.fields, "status");
        if status == "active" || (key == "error" && field_str(&row.fields, "error").is_empty()) {
            return None;
        }
        return Some(CredentialTarget::Credential(row.id.clone()));
    }
    if key != "error" {
        return None;
    }
    let error = field_str(&row.fields, "error");
    (!signed_out_names(error).is_empty()).then(|| CredentialTarget::Error(error.to_string().into()))
}

/// A credential's Status cell: its status, or the progress of a Check / reconnect
/// running in this window (`credentialCellBadge`). Returns (text, badge glyph, color name).
pub fn credential_cell_badge(row: &QueryRow, cx: &App) -> (SharedString, Glyph, &'static str) {
    let status = field_str(&row.fields, "status");
    let progress = Progress::global(cx).read(cx);
    let (clock, orange) = credential_status_badge("connecting");
    if progress.checking(&row.id, &row.fields) {
        return ("Checking…".into(), clock, orange);
    }
    match progress.reconnect.get(&row.id) {
        Some(p) if p.phase == ReconnectPhase::Starting => {
            return ("Starting sign-in…".into(), clock, orange);
        }
        Some(p) if p.phase == ReconnectPhase::Waiting => {
            return ("Waiting for sign-in…".into(), clock, orange);
        }
        Some(p) if p.phase == ReconnectPhase::Failed && status != "active" => {
            let (x, red) = credential_status_badge("broken");
            return (p.error.clone().into(), x, red);
        }
        _ => {}
    }
    let (glyph, color) = credential_status_badge(status);
    (credential_status_text(status).into(), glyph, color)
}

// ── Progress, tab-wide ────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ReconnectPhase {
    Starting,
    Waiting,
    Connected,
    Failed,
}

#[derive(Clone, Debug)]
pub(crate) struct ReconnectState {
    pub phase: ReconnectPhase,
    pub error: String,
    pub fields: Fields,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CheckPhase {
    Checking,
    Checked,
    Unanswered,
    Failed,
}

#[derive(Clone, Debug)]
pub(crate) struct CheckState {
    pub phase: CheckPhase,
    pub error: String,
    pub fields: Fields,
    pub requested_at: i64,
}

/// Reconnects and Checks started (or followed) in this window, per credential.
pub(crate) struct Progress {
    reconnect: HashMap<String, ReconnectState>,
    reconnect_tasks: HashMap<String, Task<()>>,
    checks: HashMap<String, CheckState>,
    check_tasks: HashMap<String, Task<()>>,
}

struct GlobalProgress(Entity<Progress>);

impl Global for GlobalProgress {}

/// A credential's status after a sign-in, as a reconnect outcome.
fn settled(fields: Fields, fallback_error: &str) -> ReconnectState {
    let status = field_str(&fields, "status").to_string();
    match status.as_str() {
        "connecting" => ReconnectState {
            phase: ReconnectPhase::Waiting,
            error: String::new(),
            fields,
        },
        "active" => ReconnectState {
            phase: ReconnectPhase::Connected,
            error: String::new(),
            fields,
        },
        _ => {
            let own = field_str(&fields, "error").to_string();
            let error = if !own.is_empty() {
                own
            } else if !fallback_error.is_empty() {
                fallback_error.to_string()
            } else {
                credential_status_text(&status)
            };
            ReconnectState {
                phase: ReconnectPhase::Failed,
                error,
                fields,
            }
        }
    }
}

fn harness_fields_moved(a: &Fields, b: &Fields) -> bool {
    HARNESS_FIELDS.iter().any(|k| a.get(*k) != b.get(*k))
}

impl Progress {
    pub(crate) fn global(cx: &App) -> Entity<Self> {
        cx.try_global::<GlobalProgress>()
            .map(|g| g.0.clone())
            .expect("roostr_properties::init not called")
    }

    pub(crate) fn init(cx: &mut App) {
        if cx.try_global::<GlobalProgress>().is_none() {
            let e = cx.new(|_| Self {
                reconnect: HashMap::new(),
                reconnect_tasks: HashMap::new(),
                checks: HashMap::new(),
                check_tasks: HashMap::new(),
            });
            cx.set_global(GlobalProgress(e));
        }
    }

    pub(crate) fn reconnect_state(&self, id: &str) -> Option<&ReconnectState> {
        self.reconnect.get(id)
    }

    pub(crate) fn check_state(&self, id: &str) -> Option<&CheckState> {
        self.checks.get(id)
    }

    /// A Check is waiting: one this window follows, or one another device asked for.
    pub(crate) fn checking(&self, id: &str, fields: &Fields) -> bool {
        let s = self.checks.get(id);
        let pending = pending_check(fields, now_ms());
        s.is_some_and(|s| s.phase == CheckPhase::Checking)
            || (pending > 0 && Some(pending) != s.map(|s| s.requested_at))
    }

    /// Connect this credential (or, `already_connecting`, follow a sign-in started elsewhere).
    pub(crate) fn reconnect(&mut self, id: &str, already_connecting: bool, cx: &mut Context<Self>) {
        if self
            .reconnect
            .get(id)
            .is_some_and(|s| matches!(s.phase, ReconnectPhase::Starting | ReconnectPhase::Waiting))
        {
            return;
        }
        let id_owned = id.to_string();
        let run = (!already_connecting).then(|| run_action("connect", id, cx));
        self.set_reconnect(
            id,
            ReconnectState {
                phase: if already_connecting {
                    ReconnectPhase::Waiting
                } else {
                    ReconnectPhase::Starting
                },
                error: String::new(),
                fields: Fields::new(),
            },
            cx,
        );
        let task = cx.spawn(async move |this, cx| {
            let id = id_owned;
            if let Some(run) = run {
                let result = run.await;
                cx.update(|cx| changed(&id, cx));
                let state = match result {
                    Err(error) => ReconnectState {
                        phase: ReconnectPhase::Failed,
                        error: error.to_string(),
                        fields: Fields::new(),
                    },
                    Ok(r) => settled(
                        Fields::from([("status".to_string(), Value::string(r.status))]),
                        &r.error,
                    ),
                };
                let waiting = state.phase == ReconnectPhase::Waiting;
                let _ = this.update(cx, |this, cx| this.set_reconnect(&id, state, cx));
                if !waiting {
                    let _ = this.update(cx, |this, _| this.reconnect_tasks.remove(&id));
                    return;
                }
            }
            // Follow the `connecting` status the harness writes (pollWhileConnecting).
            let deadline = now_ms() + CONNECT_TIMEOUT_MS;
            let mut last = Fields::new();
            loop {
                cx.background_executor().timer(POLL).await;
                if now_ms() > deadline {
                    let _ = this.update(cx, |this, cx| {
                        let state = ReconnectState {
                            phase: ReconnectPhase::Failed,
                            error: CONNECT_TIMEOUT_TEXT.into(),
                            fields: last.clone(),
                        };
                        this.set_reconnect(&id, state, cx);
                    });
                    break;
                }
                let read = cx.update(|cx| read_fields(&id, cx));
                let Some(fresh) = read.await else { continue };
                if harness_fields_moved(&fresh, &last) {
                    last = fresh.clone();
                    let _ = this.update(cx, |this, cx| {
                        this.set_reconnect(&id, settled(fresh.clone(), ""), cx)
                    });
                    cx.update(|cx| changed(&id, cx));
                }
                if field_str(&fresh, "status") != "connecting" {
                    break;
                }
            }
            let _ = this.update(cx, |this, _| this.reconnect_tasks.remove(&id));
        });
        self.reconnect_tasks.insert(id.to_string(), task);
    }

    fn set_reconnect(&mut self, id: &str, state: ReconnectState, cx: &mut Context<Self>) {
        self.reconnect.insert(id.to_string(), state);
        cx.notify();
    }

    /// Check this credential: `here` asks the serving harness straight; else
    /// write the request and follow; `pending` follows a request already written.
    pub(crate) fn check(
        &mut self,
        id: &str,
        fields: &Fields,
        here: bool,
        pending: i64,
        cx: &mut Context<Self>,
    ) {
        if self
            .checks
            .get(id)
            .is_some_and(|s| s.phase == CheckPhase::Checking)
        {
            return;
        }
        let before = field_num(fields, "checked_at");
        let requested_at = if pending > 0 { pending } else { now_ms() };
        let id_owned = id.to_string();
        let fields = fields.clone();
        self.set_check(
            id,
            CheckState {
                phase: CheckPhase::Checking,
                error: String::new(),
                fields: fields.clone(),
                requested_at,
            },
            cx,
        );
        let task = cx.spawn(async move |this, cx| {
            let id = id_owned;
            let finish = |state: CheckState, cx: &mut AsyncApp| {
                let _ = this.update(cx, |this, cx| {
                    this.set_check(&id, state, cx);
                    this.check_tasks.remove(&id);
                });
                cx.update(|cx| changed(&id, cx));
            };
            if pending == 0 {
                if here {
                    let run = cx.update(|cx| run_action("check", &id, cx));
                    match run.await {
                        Err(error) => {
                            finish(
                                CheckState {
                                    phase: CheckPhase::Failed,
                                    error: error.to_string(),
                                    fields,
                                    requested_at,
                                },
                                cx,
                            );
                        }
                        Ok(result) => {
                            let read = cx.update(|cx| read_fields(&id, cx));
                            let answer = read.await.unwrap_or_else(|| {
                                let mut f = fields.clone();
                                f.insert("status".into(), Value::string(result.status));
                                f.insert("error".into(), Value::string(result.error));
                                f
                            });
                            finish(
                                CheckState {
                                    phase: CheckPhase::Checked,
                                    error: String::new(),
                                    fields: answer,
                                    requested_at,
                                },
                                cx,
                            );
                        }
                    }
                    return;
                }
                let ask = cx.update(|cx| {
                    api::note::set_field(&id, CHECK_REQUEST_KEY, Value::int(requested_at), cx)
                });
                if let Err(error) = ask.await {
                    finish(
                        CheckState {
                            phase: CheckPhase::Failed,
                            error: error.to_string(),
                            fields,
                            requested_at,
                        },
                        cx,
                    );
                    return;
                }
                cx.update(|cx| changed(&id, cx));
            }
            loop {
                cx.background_executor().timer(CHECK_POLL).await;
                if now_ms() - requested_at > CHECK_TIMEOUT_MS {
                    let _ = this.update(cx, |this, cx| {
                        this.set_check(
                            &id,
                            CheckState {
                                phase: CheckPhase::Unanswered,
                                error: String::new(),
                                fields: fields.clone(),
                                requested_at,
                            },
                            cx,
                        );
                        this.check_tasks.remove(&id);
                    });
                    return;
                }
                let read = cx.update(|cx| read_fields(&id, cx));
                if let Some(fresh) = read.await
                    && check_answered(&fresh, requested_at, before)
                {
                    finish(
                        CheckState {
                            phase: CheckPhase::Checked,
                            error: String::new(),
                            fields: fresh,
                            requested_at,
                        },
                        cx,
                    );
                    return;
                }
            }
        });
        self.check_tasks.insert(id.to_string(), task);
    }

    fn set_check(&mut self, id: &str, state: CheckState, cx: &mut Context<Self>) {
        self.checks.insert(id.to_string(), state);
        cx.notify();
    }

    /// Follow a Check another device asked for, unless this window already followed that one.
    pub(crate) fn follow_check(&mut self, id: &str, fields: &Fields, cx: &mut Context<Self>) {
        let pending = pending_check(fields, now_ms());
        if pending > 0 && self.checks.get(id).map(|s| s.requested_at) != Some(pending) {
            self.check(id, fields, false, pending, cx);
        }
    }
}

/// What a Check says (`checkLine`): text and tone ("muted" | "ok" | "error").
pub(crate) fn check_line(
    phase: CheckPhase,
    error: &str,
    fields: &Fields,
    computer: &str,
) -> (String, &'static str) {
    match phase {
        CheckPhase::Checking => (
            format!(
                "Checking on {}…",
                if computer.is_empty() {
                    "its computer"
                } else {
                    computer
                }
            ),
            "muted",
        ),
        CheckPhase::Unanswered => (
            format!(
                "{} hasn't answered - is Roostr running there?",
                if computer.is_empty() {
                    "Its computer"
                } else {
                    computer
                }
            ),
            "error",
        ),
        CheckPhase::Failed => (error.to_string(), "error"),
        CheckPhase::Checked => {
            let status = field_str(fields, "status");
            let status = if status.is_empty() { "missing" } else { status };
            let err = field_str(fields, "error");
            let text = |s: &str| {
                if err.is_empty() {
                    credential_status_text(s)
                } else {
                    err.to_string()
                }
            };
            match status {
                "active" => ("Connected".into(), "ok"),
                "connecting" => (text(status), "muted"),
                _ => (text(status), "error"),
            }
        }
    }
}

/// The line under a credential's Status for its current Check, if any.
pub(crate) fn check_says(
    progress: &Progress,
    id: &str,
    fields: &Fields,
    computer: &str,
) -> Option<(String, &'static str)> {
    if progress.checking(id, fields) {
        return Some(check_line(CheckPhase::Checking, "", fields, computer));
    }
    progress
        .check_state(id)
        .map(|s| check_line(s.phase, &s.error, &s.fields, computer))
}

#[cfg(test)]
mod tests {
    // `gpui_kit::*` brings GPUI's own `test` attribute; these are plain tests.
    use std::prelude::v1::test;

    use super::*;

    fn s(v: &str) -> Value {
        Value::string(v)
    }

    fn i(n: i64) -> Value {
        Value::int(n)
    }

    fn fields<const N: usize>(entries: [(&str, Value); N]) -> Fields {
        entries
            .into_iter()
            .map(|(k, v)| (k.to_string(), v))
            .collect()
    }

    fn badge(names: &[&str]) -> String {
        format!(
            "credential signed out: {} - reconnect {}",
            names.join(", "),
            if names.len() == 1 { "it" } else { "them" }
        )
    }

    fn cred(id: &str, name: &str) -> CredentialRef {
        CredentialRef {
            id: id.into(),
            name: name.into(),
        }
    }

    fn found(error: &str, linked: &[CredentialRef], all: &[CredentialRef]) -> Vec<String> {
        credentials_for_error(error, linked, all)
            .into_iter()
            .map(|c| c.id)
            .collect()
    }

    #[test]
    fn reads_the_names_the_harness_badge_lists() {
        assert_eq!(signed_out_names(&badge(&["X (Twitter)"])), ["X (Twitter)"]);
        assert_eq!(
            signed_out_names(&badge(&["Gmail", "LinkedIn"])),
            ["Gmail", "LinkedIn"]
        );
        assert!(signed_out_names("run failed: timeout").is_empty());
        assert!(signed_out_names("").is_empty());
    }

    #[test]
    fn prefers_the_credentials_the_object_lists_over_a_same_named_one_elsewhere() {
        let linked = [cred("mine", "Gmail")];
        let all = [
            cred("other", "Gmail"),
            cred("mine", "Gmail"),
            cred("li", "LinkedIn"),
        ];
        assert_eq!(
            found(&badge(&["LinkedIn", "Gmail"]), &linked, &all),
            ["li", "mine"]
        );
    }

    #[test]
    fn falls_back_to_every_credential_with_the_name_when_the_object_lists_none() {
        let all = [cred("a", "Gmail"), cred("b", "Gmail"), cred("c", "Slack")];
        assert_eq!(found(&badge(&["Gmail"]), &[], &all), ["a", "b"]);
    }

    #[test]
    fn a_name_with_a_comma_matches_whole_and_partial_names_do_not() {
        let all = [
            cred("acme", "Acme, Inc"),
            cred("acm", "Acme"),
            cred("inc", "Inc"),
        ];
        assert_eq!(found(&badge(&["Acme, Inc"]), &[], &all), ["acme"]);
        assert!(
            found(&badge(&["Acme, Inc"]), &[cred("acme", "Acme, Inc")], &all)
                .contains(&"acme".to_string())
        );
        // Cut off by the 300-character limit: no " - reconnect" tail, last name partial.
        assert_eq!(
            found(
                "credential signed out: Gmail, Linked",
                &[],
                &[cred("g", "Gmail"), cred("l", "LinkedIn")]
            ),
            ["g"]
        );
        assert!(found("Gmail is down", &[], &[cred("g", "Gmail")]).is_empty());
    }

    #[test]
    fn reads_an_agents_credentials_links_and_plain_ids() {
        let linked: Fields = serde_json::from_value(json!({
            "credentials": { "valuesValue": { "items": [
                { "linkValue": { "targetId": "c1", "relationKey": "credentials" } },
                { "stringValue": "c2" },
            ] } },
        }))
        .expect("fields fixture");
        assert_eq!(linked_credential_ids(&linked), ["c1", "c2"]);
        assert_eq!(
            linked_credential_ids(&fields([("credentials", s("c3"))])),
            ["c3"]
        );
        assert!(linked_credential_ids(&Fields::new()).is_empty());
    }

    #[test]
    fn which_cells_reconnect() {
        let row = |id: &str, type_key: &str, fields: Fields| QueryRow {
            id: id.into(),
            type_key: type_key.into(),
            fields,
            ..Default::default()
        };
        let credential = |status: &str, error: &str| {
            let mut f = fields([("status", s(status))]);
            if !error.is_empty() {
                f.insert("error".into(), s(error));
            }
            row("c", "credential", f)
        };
        let this = Some(CredentialTarget::Credential("c".into()));
        assert_eq!(
            reconnect_target(&credential("needs_auth", ""), "status"),
            this
        );
        assert_eq!(
            reconnect_target(&credential("needs_auth", "session expired"), "error"),
            this
        );
        assert_eq!(
            reconnect_target(&credential("needs_auth", ""), "error"),
            None
        );
        assert_eq!(reconnect_target(&credential("active", ""), "status"), None);
        assert_eq!(
            reconnect_target(&credential("needs_auth", ""), "name"),
            None
        );
        let task = |error: &str| row("t", "task", fields([("error", s(error))]));
        let gmail = badge(&["Gmail"]);
        assert_eq!(
            reconnect_target(&task(&gmail), "error"),
            Some(CredentialTarget::Error(gmail.clone().into()))
        );
        assert_eq!(reconnect_target(&task("run failed"), "error"), None);
        assert_eq!(reconnect_target(&task(&gmail), "status"), None);
    }

    // ── Action gate ──

    fn website() -> Fields {
        fields([
            ("login_url", s("https://x.com/login")),
            ("session_host", s("x.com")),
            ("session_cookie", s("auth_token")),
        ])
    }

    fn google() -> Fields {
        fields([
            ("service", s("google-account")),
            ("account", s("me@example.com")),
        ])
    }

    fn database() -> Fields {
        fields([("service", s("postgres")), ("key_url", s("postgres://x"))])
    }

    /// (can_connect, can_check, check_here, reason)
    fn gate(paired: bool, this: &str, served_by: &str, f: &Fields) -> (bool, bool, bool, String) {
        let g = action_gate(paired, this, served_by, "Mac", f);
        (g.can_connect, g.can_check, g.check_here, g.reason)
    }

    #[test]
    fn the_window_paired_with_the_serving_computer_does_everything_and_checks_straight() {
        for f in [website(), google(), database()] {
            assert_eq!(
                gate(true, "m1", "m1", &f),
                (true, true, true, String::new())
            );
        }
    }

    #[test]
    fn a_website_or_google_sign_in_connects_from_any_paired_computer() {
        for f in [website(), google()] {
            assert_eq!(
                gate(true, "m2", "m1", &f),
                (true, true, false, String::new())
            );
        }
    }

    #[test]
    fn anything_else_connects_only_on_the_serving_computer_and_checks_from_anywhere() {
        assert_eq!(
            gate(true, "m2", "m1", &database()),
            (
                false,
                true,
                false,
                "Open Roostr on Mac to connect this credential.".into()
            )
        );
    }

    #[test]
    fn an_unpaired_window_still_checks_and_without_served_by_nobody_acts() {
        assert_eq!(
            gate(false, "", "m1", &website()),
            (
                false,
                true,
                false,
                "Pair this tab with one of your computers to sign in there.".into()
            )
        );
        for f in [google(), database()] {
            assert!(gate(false, "", "m1", &f).1);
        }
        assert_eq!(
            gate(false, "", "m1", &database()).3,
            "Open Roostr on Mac to connect this credential."
        );
        assert_eq!(
            gate(true, "m1", "", &website()),
            (
                false,
                false,
                false,
                "Set Served by to a computer first.".into()
            )
        );
    }

    #[test]
    fn the_sign_in_hint_names_where_the_window_opened_and_who_gets_the_login() {
        assert_eq!(
            connecting_hint(&website(), "this computer", "Sharky's Mac Studio"),
            "Sign in in the Chrome window on this computer; Roostr then hands the login to Sharky's Mac Studio."
        );
        assert_eq!(
            connecting_hint(&website(), "this computer", ""),
            "Sign in in the Chrome window on this computer."
        );
        assert_eq!(
            connecting_hint(&google(), "this computer", "Mac"),
            "Sign in to me@example.com in the browser window gws opens on this computer; Roostr then hands the login to Mac."
        );
    }

    #[test]
    fn pasted_keys_and_cookie_values_are_secret_but_cookie_details_and_key_list_are_not() {
        for key in [
            "key_api_key",
            "key_refresh_token",
            "cookie_auth_token",
            "cookie_meta_2",
        ] {
            assert!(is_secret_field(key), "{key}");
        }
        for key in [
            "key_fields",
            "key_",
            "cookie_meta",
            "cookie_",
            "session_cookie",
            "status",
        ] {
            assert!(!is_secret_field(key), "{key}");
        }
    }

    // ── Reconnect and Check outcomes ──

    #[test]
    fn a_connect_answer_settles_waiting_connected_or_failed() {
        let phase = |status: &str| settled(fields([("status", s(status))]), "").phase;
        assert_eq!(phase("connecting"), ReconnectPhase::Waiting);
        // Active at once (database, Google already signed in): connected without a poll.
        assert_eq!(phase("active"), ReconnectPhase::Connected);
        let signed_out = settled(
            fields([
                ("status", s("needs_auth")),
                ("error", s("Window closed before sign-in")),
            ]),
            "",
        );
        assert_eq!(signed_out.phase, ReconnectPhase::Failed);
        assert_eq!(signed_out.error, "Window closed before sign-in");
    }

    #[test]
    fn a_check_answer_reads_as_its_line() {
        let active = fields([("status", s("active")), ("checked_at", i(1_002_000))]);
        assert_eq!(
            check_line(CheckPhase::Checked, "", &active, "Mac"),
            ("Connected".to_string(), "ok")
        );
        let gone = fields([
            ("status", s("needs_auth")),
            ("error", s("The X sign-in expired")),
            ("checked_at", i(1_001_000)),
        ]);
        assert_eq!(
            check_line(CheckPhase::Checked, "", &gone, "Mac"),
            ("The X sign-in expired".to_string(), "error")
        );
        assert_eq!(
            check_line(CheckPhase::Checking, "", &Fields::new(), "Mac"),
            ("Checking on Mac…".to_string(), "muted")
        );
        assert_eq!(
            check_line(CheckPhase::Unanswered, "", &Fields::new(), "Mac").0,
            "Mac hasn't answered - is Roostr running there?"
        );
    }

    #[test]
    fn an_answer_counts_once_checked_at_moves_even_from_a_clock_running_behind() {
        // Not yet answered: checked_at is still the value from before the request.
        assert!(!check_answered(
            &fields([("checked_at", i(900_000))]),
            1_000_000,
            900_000
        ));
        // Stamped past the request.
        assert!(check_answered(
            &fields([("checked_at", i(1_002_000))]),
            1_000_000,
            900_000
        ));
        // Stamped by a computer whose clock runs behind: still moved.
        assert!(check_answered(
            &fields([("checked_at", i(950_000))]),
            1_000_000,
            900_000
        ));
    }

    #[test]
    fn a_request_is_pending_until_answered_and_for_a_minute_at_most() {
        let asked = fields([
            ("checked_at", i(900_000)),
            (CHECK_REQUEST_KEY, i(1_000_000)),
        ]);
        assert_eq!(pending_check(&asked, 1_010_000), 1_000_000);
        assert_eq!(pending_check(&asked, 1_000_000 + CHECK_TIMEOUT_MS + 1), 0);
        let answered = fields([
            ("checked_at", i(1_000_100)),
            (CHECK_REQUEST_KEY, i(1_000_000)),
        ]);
        assert_eq!(pending_check(&answered, 1_010_000), 0);
        assert_eq!(
            pending_check(&fields([("checked_at", i(900_000))]), 1_010_000),
            0
        );
    }

    #[test]
    fn a_status_reads_as_its_words() {
        assert_eq!(
            credential_status_text("needs_auth"),
            "Signed out - reconnect"
        );
        assert_eq!(credential_status_text("active"), "Connected");
    }
}
