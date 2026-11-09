//! A capability's Status editor (`CapabilityStatus.svelte`) and its harness
//! requests (`capability-actions.ts`): the serving computer's harness writes
//! the status; this says it in words and offers the requests that change it,
//! only on the computer its Served by names. Waiting requests list inline
//! with Approve / Reject.

use std::collections::HashMap;
use std::time::Duration;

use anyhow::{Result, anyhow};
use roostr_core::create::this_machine_id;
use roostr_core::harness::HarnessError;
use roostr_core::serving::{MachineRow, fetch_machines, machine_name, served_by_machine_id};
use roostr_core::types::{AgentEndpoint, AgentMessage, field_str};
use roostr_core::{State, api};
use roostr_rt::Task;
use roostr_ui::ty;
use roostr_ui::{Alert, Glyph, KitSize, Label, Tag, Theme, radius};
use rune::{El, col, el, row};
use serde::Deserialize;
use serde_json::{Value as Json, json};

use crate::widgets::{act_button, later, note, rule};

/// Request statuses still waiting on a human or running.
const OPEN_REQUEST: [&str; 3] = ["pending", "awaiting_approval", "processing"];
const REQUEST_POLL: Duration = Duration::from_millis(2_000);

/// A capability's status in words.
pub(crate) fn capability_status_text(status: &str) -> String {
    match if status.is_empty() { "missing" } else { status } {
        "active" => "Active".into(),
        "needs_auth" => "Needs sign-in".into(),
        "needs_approval" => "Awaiting approval".into(),
        "processing" => "Working…".into(),
        "disabled" => "Switched off".into(),
        "broken" => "Broken".into(),
        "missing" => "Not installed".into(),
        other => other.replace('_', " "),
    }
}

/// The status row's badge: glyph and palette color name.
pub(crate) fn capability_status_badge(status: &str) -> (Glyph, &'static str) {
    match if status.is_empty() { "missing" } else { status } {
        "active" => (Glyph::BadgeCheck, "lime"),
        "needs_auth" | "needs_approval" | "processing" => (Glyph::BadgeClock, "orange"),
        "broken" => (Glyph::BadgeX, "red"),
        _ => (Glyph::BadgeDashed, ""),
    }
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct CapabilityRequest {
    object_id: String,
    message_id: String,
    operation: String,
    status: String,
    error: String,
    can_approve: bool,
}

fn list_requests(s: &State) -> Task<Result<Vec<CapabilityRequest>>> {
    let Some(h) = s.harness.clone() else {
        return Task::ready(Err(anyhow!("no harness")));
    };
    s.rt.background(async move {
        let answer = h.get_json("/capability-requests").await.map_err(|e| {
            match e.downcast_ref::<HarnessError>() {
                Some(he) => anyhow!("Cannot load approvals (HTTP {}).", he.status),
                None => e,
            }
        })?;
        Ok(serde_json::from_value(
            answer
                .get("requests")
                .cloned()
                .unwrap_or(Json::Array(vec![])),
        )?)
    })
}

/// Stage one operation into the capability's mailbox, sent by this machine's own machine object.
fn stage_request(
    capability_id: String,
    key: String,
    operation: String,
    s: &mut State,
) -> Task<Result<()>> {
    let here = this_machine_id(s);
    let machines = api::query_all(json!({ "type": "machine" }), s);
    let open = list_requests(s);
    s.rt.spawn(async move |cx| {
        let id = here.await;
        if id.is_empty() {
            return Err(anyhow!(
                "Cannot identify this computer. Is the harness running?"
            ));
        }
        let Some(source) = machines
            .await?
            .into_iter()
            .find(|r| field_str(&r.fields, "machine_id") == id)
        else {
            return Err(anyhow!("This computer has not published its object yet."));
        };
        let open = open.await.unwrap_or_default();
        if open.iter().any(|r| {
            r.object_id == capability_id
                && r.operation == operation
                && OPEN_REQUEST.contains(&r.status.as_str())
        }) {
            return Err(anyhow!("This request is already waiting for approval."));
        }
        let message = AgentMessage {
            id: uuid::Uuid::new_v4().to_string(),
            exchange_id: uuid::Uuid::new_v4().to_string(),
            sender: AgentEndpoint {
                object_id: source.id.clone(),
                agent_id: String::new(),
            },
            recipients: vec![AgentEndpoint {
                object_id: capability_id.clone(),
                agent_id: String::new(),
            }],
            text: format!("Request {operation} for {key}."),
            reply_to: String::new(),
            sent_at: chrono::Utc::now().timestamp_millis(),
            title: "Capability request".into(),
            request_reply: true,
            historical: false,
            operation: operation.clone(),
            author: String::new(),
            ..Default::default()
        };
        cx.update(|s| api::mailbox::send(&message, s)).await?;
        Ok(())
    })
}

/// Approve or reject a waiting request; approval executes on this machine's harness.
fn resolve_request(request: &CapabilityRequest, action: &str, s: &State) -> Task<Result<()>> {
    let Some(h) = s.harness.clone() else {
        return Task::ready(Err(anyhow!("no harness")));
    };
    let path = format!("/capability-requests/{action}");
    let body = json!({ "objectId": request.object_id, "messageId": request.message_id });
    s.rt.background(async move {
        let answer = h.post_json(&path, &body).await?;
        if let Some(e) = answer.get("error").and_then(Json::as_str) {
            return Err(anyhow!(e.to_string()));
        }
        Ok(())
    })
}

#[derive(Default)]
struct CapabilityStatus {
    /// Which open this is (a reopen ignores the loads of the one before).
    open: u64,
    machines: Vec<MachineRow>,
    this_machine: String,
    ready: bool,
    busy: bool,
    action_error: String,
    notice: String,
    requests: Vec<CapabilityRequest>,
    request_poll: Option<Task<()>>,
    /// Set after staging: poll until the serving harness shows the request.
    expecting: bool,
}

#[derive(Default)]
struct Capabilities {
    opened: u64,
    by_id: HashMap<String, CapabilityStatus>,
}

fn entry<'a>(s: &'a mut State, id: &str, open: u64) -> Option<&'a mut CapabilityStatus> {
    s.slot::<Capabilities>()
        .by_id
        .get_mut(id)
        .filter(|c| c.open == open)
}

/// Opens a capability's Status editor afresh (GPUI made a new view per open): loads the
/// machine roster, then the harness's requests.
pub(crate) fn open_capability_status(s: &mut State, object_id: &str) {
    let slot = s.slot::<Capabilities>();
    slot.opened += 1;
    let open = slot.opened;
    slot.by_id.insert(
        object_id.to_string(),
        CapabilityStatus {
            open,
            ..CapabilityStatus::default()
        },
    );
    let roster = fetch_machines(s);
    let here = this_machine_id(s);
    let id = object_id.to_string();
    s.rt.spawn(async move |cx| {
        let machines = roster.await.map(|(_, m)| m).unwrap_or_default();
        let this_machine = here.await;
        cx.update(|s| {
            let Some(c) = entry(s, &id, open) else { return };
            c.machines = machines;
            c.this_machine = this_machine;
            c.ready = true;
            load_requests(s, &id, open);
        });
    })
    .detach();
}

/// Drops a capability's Status editor state (and its request poll).
pub(crate) fn forget(s: &mut State, object_id: &str) {
    s.slot::<Capabilities>().by_id.remove(object_id);
}

fn load_requests(s: &mut State, object_id: &str, open: u64) {
    if s.harness.is_none() {
        return;
    }
    let task = list_requests(s);
    let id = object_id.to_string();
    s.rt.spawn(async move |cx| {
        // Harness unreachable: actions report on press.
        let Ok(all) = task.await else { return };
        cx.update(|s| {
            let poll = |s: &State| {
                let id = id.clone();
                s.rt.spawn(async move |cx| {
                    loop {
                        cx.timer(REQUEST_POLL).await;
                        let alive = cx.update(|s| {
                            let alive = entry(s, &id, open).is_some();
                            if alive {
                                load_requests(s, &id, open);
                            }
                            alive
                        });
                        if !alive {
                            return;
                        }
                    }
                })
            };
            let needs_poll = {
                let Some(c) = entry(s, &id, open) else { return };
                c.requests = all.into_iter().filter(|r| r.object_id == id).collect();
                let open_request = c
                    .requests
                    .iter()
                    .any(|r| OPEN_REQUEST.contains(&r.status.as_str()));
                if open_request {
                    c.expecting = false;
                }
                if !open_request && !c.expecting {
                    c.request_poll = None;
                }
                (open_request || c.expecting) && c.request_poll.is_none()
            };
            if needs_poll {
                let task = poll(s);
                if let Some(c) = entry(s, &id, open) {
                    c.request_poll = Some(task);
                }
            }
        });
    })
    .detach();
}

fn act(
    s: &mut State,
    object_id: &str,
    run: impl FnOnce(&mut State) -> Task<Result<()>>,
    done: &'static str,
    expect: bool,
) {
    let Some(c) = s.slot::<Capabilities>().by_id.get_mut(object_id) else {
        return;
    };
    if c.busy {
        return;
    }
    c.busy = true;
    c.action_error.clear();
    c.notice.clear();
    let open = c.open;
    let run = run(s);
    let id = object_id.to_string();
    s.rt.spawn(async move |cx| {
        let result = run.await;
        cx.update(|s| {
            let Some(c) = entry(s, &id, open) else { return };
            match result {
                Ok(()) => {
                    c.notice = done.into();
                    if expect {
                        c.expecting = true;
                    }
                }
                Err(error) => c.action_error = error.to_string(),
            }
            c.busy = false;
            load_requests(s, &id, open);
        });
    })
    .detach();
}

fn small_note(t: &Theme, line: impl Into<String>) -> El {
    note(t, line, 12.5)
}

fn error_line(t: &Theme, key: &str, line: String) -> El {
    Alert::error(key, line)
        .size(KitSize::XSmall)
        .text_size(12.5)
        .build(t)
}

/// A capability's Status editor. Opens itself on first draw.
pub(crate) fn capability_status(
    s: &State,
    _cx: &mut rune::Cx<State>,
    t: &Theme,
    _layer: &mut Vec<El>,
    object_id: &str,
) -> El {
    let Some(object) = s.store.object(object_id) else {
        return el();
    };
    let none = CapabilityStatus::default();
    let st = match s.get::<Capabilities>().and_then(|c| c.by_id.get(object_id)) {
        Some(st) => st,
        None => {
            let id = object_id.to_string();
            later(s, move |s| {
                if !s.slot::<Capabilities>().by_id.contains_key(&id) {
                    open_capability_status(s, &id);
                }
            });
            &none
        }
    };
    let fields = &object.fields;
    let is_template = object.type_key == "template";
    let key = field_str(fields, "key").to_string();
    let status = field_str(fields, "status").to_string();
    let error = field_str(fields, "error").to_string();
    let served_by = served_by_machine_id(fields, &st.machines);
    let computer = if served_by.is_empty() {
        String::new()
    } else {
        machine_name(&st.machines, &served_by)
    };
    let paired = s.harness.is_some();
    let can_act = paired && !st.this_machine.is_empty() && st.this_machine == served_by;
    let actions: &[(&'static str, &str)] = match status.as_str() {
        "active" => &[
            ("skill.check", "Check"),
            ("skill.disable", "Switch off"),
            ("skill.uninstall", "Uninstall"),
        ],
        "disabled" => &[
            ("skill.enable", "Switch on"),
            ("skill.uninstall", "Uninstall"),
        ],
        "missing" | "broken" | "" => &[("skill.install", "Install")],
        _ => &[],
    };
    let mut c = col().key("capability-status").gap(8.0).min_w(220.0).child(
        Label::new(capability_status_text(&status))
            .text_size(14.0)
            .weight(ty::SEMIBOLD)
            .color(t.fg)
            .build(t),
    );
    if !error.is_empty() {
        c = c.child(error_line(t, "cap-error", error));
    }
    if is_template {
        c = c.child(small_note(
            t,
            "A template does not install; each capability made from it does.",
        ));
    } else if st.ready {
        if can_act {
            let busy = st.busy;
            if !actions.is_empty() {
                let mut line = row().flex_wrap().items_center().gap(6.0);
                for &(operation, label) in actions {
                    let (id, key) = (object_id.to_string(), key.clone());
                    line = line.child(
                        act_button(t, format!("cap-{operation}"), label, busy)
                            .on_press(move |s: &mut State| {
                                let (cap, key) = (id.clone(), key.clone());
                                act(
                                    s,
                                    &id,
                                    move |s| stage_request(cap, key, operation.to_string(), s),
                                    "Request staged. Approve it below.",
                                    true,
                                );
                            })
                            .build(t),
                    );
                }
                c = c.child(line);
            }
            let waiting = st.requests.iter().filter(|r| {
                r.object_id == object_id
                    && (OPEN_REQUEST.contains(&r.status.as_str()) || r.status == "failed")
            });
            for request in waiting {
                let mut line = row().flex_wrap().items_center().gap(6.0).child(
                    Tag::custom(t.hl_light, t.muted, t.hl_light)
                        .rounded(radius::SM)
                        .px(8.0)
                        .py(2.0)
                        .text_size(11.0)
                        .weight(ty::SEMIBOLD)
                        .label(format!(
                            "{} · {}",
                            request.operation,
                            request.status.replace('_', " ")
                        ))
                        .build(t),
                );
                if request.status == "pending" || request.status == "awaiting_approval" {
                    if request.can_approve {
                        let (a, b) = (request.clone(), request.clone());
                        let (ida, idb) = (object_id.to_string(), object_id.to_string());
                        line = line
                            .child(
                                act_button(
                                    t,
                                    format!("approve-{}", request.message_id),
                                    "Approve",
                                    busy,
                                )
                                .on_press(move |s: &mut State| {
                                    let a = a.clone();
                                    act(
                                        s,
                                        &ida,
                                        move |s| resolve_request(&a, "approve", s),
                                        "Approved.",
                                        false,
                                    );
                                })
                                .build(t),
                            )
                            .child(
                                act_button(
                                    t,
                                    format!("reject-{}", request.message_id),
                                    "Reject",
                                    busy,
                                )
                                .on_press(move |s: &mut State| {
                                    let b = b.clone();
                                    act(
                                        s,
                                        &idb,
                                        move |s| resolve_request(&b, "reject", s),
                                        "Rejected.",
                                        false,
                                    );
                                })
                                .build(t),
                            );
                    } else {
                        line = line.child(small_note(t, format!("approve on {computer}")));
                    }
                }
                let mut block = col().gap(4.0).child(el().w_full().pb(2.0).child(rule(t)));
                block = block.child(line);
                if !request.error.is_empty() {
                    block = block.child(small_note(t, request.error.clone()));
                }
                c = c.child(block);
            }
        } else {
            c = c.child(small_note(
                t,
                if served_by.is_empty() {
                    "This capability has no computer yet.".to_string()
                } else {
                    format!("Open Roostr on {computer} to install or check this capability.")
                },
            ));
        }
    }
    if !st.notice.is_empty() {
        c = c.child(small_note(t, st.notice.clone()));
    }
    if !st.action_error.is_empty() {
        c = c.child(error_line(t, "cap-action-error", st.action_error.clone()));
    }
    c
}

#[cfg(test)]
mod tests {
    use roostr_core::Value;
    use roostr_ui::ThemePref;

    use super::*;
    use crate::test_support::*;

    fn view(s: &State, cx: &mut rune::Cx<State>) -> El {
        let t = roostr_ui::theme(ThemePref::Dark, cx);
        let mut layer = vec![];
        let body = capability_status(s, cx, t, &mut layer, "k1");
        roostr_ui::root(body, layer)
    }

    #[test]
    fn the_status_view_says_the_status_and_where_to_act() {
        let vault = Vault::new(
            vec![
                object(
                    "machine-1",
                    "machine",
                    &[
                        ("machine_id", Value::string("m1")),
                        ("name", Value::string("Mac")),
                    ],
                ),
                object(
                    "k1",
                    "capability",
                    &[
                        ("key", Value::string("gws")),
                        ("status", Value::string("active")),
                        ("error", Value::string("Token expired")),
                        ("served_by", Value::string("m1")),
                    ],
                ),
            ],
            vec![],
        );
        let mut app = App::new(vault, view);
        app.s().store.ensure_object("k1");
        app.until("the editor's machines", |s| {
            s.get::<Capabilities>()
                .and_then(|x| x.by_id.get("k1"))
                .is_some_and(|c| c.ready)
        });
        let texts: Vec<String> = app
            .rt
            .elements()
            .into_iter()
            .filter_map(|e| e.text)
            .collect();
        for want in [
            "Active",
            "Token expired",
            "Open Roostr on Mac to install or check this capability.",
        ] {
            assert!(texts.iter().any(|t| t == want), "{want} in {texts:?}");
        }
    }

    #[test]
    fn statuses_read_as_words_and_badges() {
        assert_eq!(
            capability_status_text("needs_approval"),
            "Awaiting approval"
        );
        assert_eq!(capability_status_text(""), "Not installed");
        assert_eq!(
            capability_status_badge("active"),
            (Glyph::BadgeCheck, "lime")
        );
        assert_eq!(capability_status_badge("processing").1, "orange");
    }
}
