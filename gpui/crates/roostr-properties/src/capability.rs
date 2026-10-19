//! A capability's Status editor (`CapabilityStatus.svelte`) and its harness
//! requests (`capability-actions.ts`): the serving computer's harness writes
//! the status; this says it in words and offers the requests that change it,
//! only on the computer its Served by names. Waiting requests list inline
//! with Approve / Reject.

use std::time::Duration;

use anyhow::{Result, anyhow};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use roostr_core::create::this_machine_id;
use roostr_core::harness::harness;
use roostr_core::io;
use roostr_core::serving::{MachineRow, fetch_machines, machine_name, served_by_machine_id};
use roostr_core::types::{AgentEndpoint, AgentMessage, field_str};
use roostr_core::{Glyph, Store, api, theme};
use serde::Deserialize;
use serde_json::{Value as Json, json};

use crate::widgets::act_button;

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

fn list_requests(cx: &App) -> Task<Result<Vec<CapabilityRequest>>> {
    let Some(h) = harness(cx) else {
        return Task::ready(Err(anyhow!("no harness")));
    };
    io::spawn(cx, async move {
        let answer = h.get_json("/capability-requests").await.map_err(|e| {
            match e.downcast_ref::<roostr_core::harness::HarnessError>() {
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
    cx: &mut App,
) -> Task<Result<()>> {
    let here = this_machine_id(cx);
    let machines = api::query_all(json!({ "type": "machine" }), cx);
    let open = list_requests(cx);
    cx.spawn(async move |cx| {
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
        cx.update(|cx| api::mailbox::send(&message, cx)).await?;
        Ok(())
    })
}

/// Approve or reject a waiting request; approval executes on this machine's harness.
fn resolve_request(request: &CapabilityRequest, action: &str, cx: &App) -> Task<Result<()>> {
    let Some(h) = harness(cx) else {
        return Task::ready(Err(anyhow!("no harness")));
    };
    let path = format!("/capability-requests/{action}");
    let body = json!({ "objectId": request.object_id, "messageId": request.message_id });
    io::spawn(cx, async move {
        let answer = h.post_json(&path, &body).await?;
        if let Some(e) = answer.get("error").and_then(Json::as_str) {
            return Err(anyhow!(e.to_string()));
        }
        Ok(())
    })
}

pub(crate) struct CapabilityStatus {
    object_id: String,
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
    _subs: Vec<Subscription>,
}

impl CapabilityStatus {
    pub(crate) fn new(object_id: String, cx: &mut Context<Self>) -> Self {
        let roster = fetch_machines(cx);
        let here = this_machine_id(cx);
        cx.spawn(async move |this, cx| {
            let machines = roster.await.map(|(_, m)| m).unwrap_or_default();
            let this_machine = here.await;
            let _ = this.update(cx, |this, cx| {
                this.machines = machines;
                this.this_machine = this_machine;
                this.ready = true;
                this.load_requests(cx);
                cx.notify();
            });
        })
        .detach();
        Self {
            object_id,
            machines: vec![],
            this_machine: String::new(),
            ready: false,
            busy: false,
            action_error: String::new(),
            notice: String::new(),
            requests: vec![],
            request_poll: None,
            expecting: false,
            _subs: vec![cx.observe(&Store::global(cx), |_, _, cx| cx.notify())],
        }
    }

    fn load_requests(&mut self, cx: &mut Context<Self>) {
        if harness(cx).is_none() {
            return;
        }
        let task = list_requests(cx);
        let id = self.object_id.clone();
        cx.spawn(async move |this, cx| {
            // Harness unreachable: actions report on click.
            let Ok(all) = task.await else { return };
            let _ = this.update(cx, |this, cx| {
                this.requests = all.into_iter().filter(|r| r.object_id == id).collect();
                let open = this
                    .requests
                    .iter()
                    .any(|r| OPEN_REQUEST.contains(&r.status.as_str()));
                if open {
                    this.expecting = false;
                }
                if (open || this.expecting) && this.request_poll.is_none() {
                    this.request_poll = Some(cx.spawn(async move |this, cx| {
                        loop {
                            cx.background_executor().timer(REQUEST_POLL).await;
                            let Ok(()) = this.update(cx, |this, cx| this.load_requests(cx)) else {
                                return;
                            };
                        }
                    }));
                }
                if !open && !this.expecting {
                    this.request_poll = None;
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn act(
        &mut self,
        run: Task<Result<()>>,
        done: &'static str,
        expect: bool,
        cx: &mut Context<Self>,
    ) {
        if self.busy {
            return;
        }
        self.busy = true;
        self.action_error.clear();
        self.notice.clear();
        cx.spawn(async move |this, cx| {
            let result = run.await;
            let _ = this.update(cx, |this, cx| {
                match result {
                    Ok(()) => {
                        this.notice = done.into();
                        if expect {
                            this.expecting = true;
                        }
                    }
                    Err(error) => this.action_error = error.to_string(),
                }
                this.busy = false;
                this.load_requests(cx);
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }
}

fn muted(text: impl Into<SharedString>) -> Div {
    div()
        .text_size(px(12.5))
        .text_color(theme::muted())
        .child(text.into())
}

fn error_line(text: String) -> Div {
    div()
        .text_size(px(12.5))
        .text_color(rgb(0xff6961))
        .child(text)
}

impl Render for CapabilityStatus {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(object) = Store::global(cx).read(cx).object(&self.object_id) else {
            return div();
        };
        let fields = &object.fields;
        let is_template = object.type_key == "template";
        let key = field_str(fields, "key").to_string();
        let status = field_str(fields, "status").to_string();
        let error = field_str(fields, "error").to_string();
        let served_by = served_by_machine_id(fields, &self.machines);
        let computer = if served_by.is_empty() {
            String::new()
        } else {
            machine_name(&self.machines, &served_by)
        };
        let paired = harness(cx).is_some();
        let can_act = paired && !self.this_machine.is_empty() && self.this_machine == served_by;
        let actions: &[(&str, &str)] = match status.as_str() {
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
        let mut col = div()
            .flex()
            .flex_col()
            .gap(px(8.))
            .min_w(px(220.))
            .child(
                div()
                    .text_size(px(14.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(capability_status_text(&status)),
            )
            .when(!error.is_empty(), |d| d.child(error_line(error.clone())));
        if is_template {
            col = col.child(muted(
                "A template does not install; each capability made from it does.",
            ));
        } else if self.ready {
            if can_act {
                let busy = self.busy;
                if !actions.is_empty() {
                    let mut row = div().flex().flex_wrap().items_center().gap(px(6.));
                    for &(operation, label) in actions {
                        let (id, key) = (self.object_id.clone(), key.clone());
                        row = row.child(
                            act_button(SharedString::from(format!("cap-{operation}")), label, busy)
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    let run = stage_request(
                                        id.clone(),
                                        key.clone(),
                                        operation.to_string(),
                                        cx,
                                    );
                                    this.act(run, "Request staged. Approve it below.", true, cx);
                                })),
                        );
                    }
                    col = col.child(row);
                }
                let waiting: Vec<CapabilityRequest> = self
                    .requests
                    .iter()
                    .filter(|r| {
                        r.object_id == self.object_id
                            && (OPEN_REQUEST.contains(&r.status.as_str()) || r.status == "failed")
                    })
                    .cloned()
                    .collect();
                for request in waiting {
                    let mut line = div().flex().flex_wrap().items_center().gap(px(6.)).child(
                        div()
                            .text_size(px(11.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .rounded(theme::radius::sm())
                            .px(px(8.))
                            .py(px(2.))
                            .text_color(theme::muted())
                            .bg(theme::hl_light())
                            .child(format!(
                                "{} · {}",
                                request.operation,
                                request.status.replace('_', " ")
                            )),
                    );
                    if request.status == "pending" || request.status == "awaiting_approval" {
                        if request.can_approve {
                            let (a, b) = (request.clone(), request.clone());
                            line = line
                                .child(
                                    act_button(
                                        SharedString::from(format!(
                                            "approve-{}",
                                            request.message_id
                                        )),
                                        "Approve",
                                        busy,
                                    )
                                    .on_click(cx.listener(
                                        move |this, _, _, cx| {
                                            let run = resolve_request(&a, "approve", cx);
                                            this.act(run, "Approved.", false, cx);
                                        },
                                    )),
                                )
                                .child(
                                    act_button(
                                        SharedString::from(format!(
                                            "reject-{}",
                                            request.message_id
                                        )),
                                        "Reject",
                                        busy,
                                    )
                                    .on_click(cx.listener(
                                        move |this, _, _, cx| {
                                            let run = resolve_request(&b, "reject", cx);
                                            this.act(run, "Rejected.", false, cx);
                                        },
                                    )),
                                );
                        } else {
                            line = line.child(muted(format!("approve on {computer}")));
                        }
                    }
                    let mut block = div()
                        .flex()
                        .flex_col()
                        .gap(px(4.))
                        .border_t_1()
                        .border_color(theme::border())
                        .pt(px(6.))
                        .child(line);
                    if !request.error.is_empty() {
                        block = block.child(muted(request.error.clone()));
                    }
                    col = col.child(block);
                }
            } else {
                col = col.child(muted(if served_by.is_empty() {
                    "This capability has no computer yet.".to_string()
                } else {
                    format!("Open Roostr on {computer} to install or check this capability.")
                }));
            }
        }
        if !self.notice.is_empty() {
            col = col.child(muted(self.notice.clone()));
        }
        if !self.action_error.is_empty() {
            col = col.child(error_line(self.action_error.clone()));
        }
        col
    }
}
