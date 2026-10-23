//! A credential's Status editor (`CredentialStatus.svelte`), its Reconnect
//! block as a view's popover offers it (`CredentialReconnect.svelte`), and
//! the view cell that opens that popover (`CredentialCell.svelte`).

use std::rc::Rc;

use gpui_kit::component::IconName;
use gpui_kit::component::label::Label;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use roostr_core::create::this_machine_id;
use roostr_core::serving::{MachineRow, fetch_machines, machine_name, served_by_machine_id};
use roostr_core::types::{field_str, guest_agents};
use roostr_core::{ObjectSlot, QueryRow, Route, Store, api, nav, theme};
use serde_json::json;

use crate::credentials::{
    CredentialRef, CredentialTarget, Progress, ReconnectPhase, action_gate, can_connect, changed,
    check_says, connect_label, connecting_hint, credential_status_text, credentials_for_error,
    linked_credential_ids, run_action, signed_out_names,
};
use crate::widgets::{act_button, backdrop, below, error_alert, note, tip};

/// A muted note line (kit [`Label`]), 12.5px.
fn muted_line(text: impl Into<SharedString>) -> Label {
    note(text, 12.5).line_height(relative(1.45))
}

/// A Check / sign-in outcome: "ok" a semibold [`Label`], "error" the kit
/// destructive [`Alert`](gpui_kit::component::alert::Alert) (`id` keys it),
/// anything else a muted line.
fn tone_line(id: &'static str, text: String, tone: &str) -> AnyElement {
    match tone {
        "ok" => Label::new(text)
            .text_size(px(12.5))
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(theme::fg())
            .into_any_element(),
        "error" => error_alert(id, text).text_size(px(12.5)).into_any_element(),
        _ => muted_line(text).into_any_element(),
    }
}

/// Machines and this computer's id, loaded once per view (the web's onMount).
struct Machines {
    machines: Vec<MachineRow>,
    this_machine: String,
    ready: bool,
}

fn load_machines<T: 'static>(
    cx: &mut Context<T>,
    set: impl Fn(&mut T, Machines, &mut Context<T>) + 'static,
) {
    let roster = fetch_machines(cx);
    let here = this_machine_id(cx);
    cx.spawn(async move |this, cx| {
        let machines = roster.await.map(|(_, m)| m).unwrap_or_default();
        let this_machine = here.await;
        let _ = this.update(cx, |this, cx| {
            set(
                this,
                Machines {
                    machines,
                    this_machine,
                    ready: true,
                },
                cx,
            );
            cx.notify();
        });
    })
    .detach();
}

// ── CredentialStatus ─────────────────────────────────────────────────

/// The Status editor of a credential: the status in words and the actions that change it.
pub(crate) struct CredentialStatus {
    object_id: String,
    machines: Machines,
    busy: &'static str,
    action_error: String,
    /// This window started the sign-in that is running.
    started_here: bool,
    /// The pane's connect poll gave up.
    pub(crate) poll_error: String,
    _subs: Vec<Subscription>,
}

impl CredentialStatus {
    pub(crate) fn new(object_id: String, cx: &mut Context<Self>) -> Self {
        let subs = vec![
            cx.observe(&Store::global(cx), |_, _, cx| cx.notify()),
            cx.observe(&Progress::global(cx), |_, _, cx| cx.notify()),
        ];
        load_machines(cx, |this: &mut Self, m, _| this.machines = m);
        Self {
            object_id,
            machines: Machines {
                machines: vec![],
                this_machine: String::new(),
                ready: false,
            },
            busy: "",
            action_error: String::new(),
            started_here: false,
            poll_error: String::new(),
            _subs: subs,
        }
    }

    fn run(&mut self, action: &'static str, cx: &mut Context<Self>) {
        if !self.busy.is_empty() {
            return;
        }
        self.busy = action;
        self.action_error.clear();
        let task = run_action(action, &self.object_id, cx);
        let id = self.object_id.clone();
        cx.spawn(async move |this, cx| {
            let result = task.await;
            cx.update(|cx| changed(&id, cx));
            let _ = this.update(cx, |this, cx| {
                match &result {
                    Ok(r) => {
                        if action == "connect" {
                            this.started_here = r.status == "connecting";
                        }
                    }
                    Err(error) => {
                        this.action_error = error.to_string();
                        if action == "connect" {
                            this.started_here = false;
                        }
                    }
                }
                if action == "disconnect" {
                    this.started_here = false;
                }
                this.busy = "";
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }
}

impl Render for CredentialStatus {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(object) = Store::global(cx).read(cx).object(&self.object_id) else {
            return div();
        };
        let fields = &object.fields;
        let is_template = object.type_key == "template";
        let status = match field_str(fields, "status") {
            "" => "missing",
            s => s,
        }
        .to_string();
        let served_by = served_by_machine_id(fields, &self.machines.machines);
        let computer = if served_by.is_empty() {
            String::new()
        } else {
            machine_name(&self.machines.machines, &served_by)
        };
        let paired = roostr_core::harness::harness(cx).is_some();
        let this_machine = self.machines.this_machine.clone();
        let gate = action_gate(paired, &this_machine, &served_by, &computer, fields);
        let hands_to = if !this_machine.is_empty() && this_machine != served_by {
            computer.clone()
        } else {
            String::new()
        };
        // A Check another device asked for: follow it here too.
        if !is_template {
            let id = self.object_id.clone();
            let f = fields.clone();
            Progress::global(cx).update(cx, |p, cx| p.follow_check(&id, &f, cx));
        }
        let progress = Progress::global(cx).read(cx);
        let waiting_for_check = progress.checking(&self.object_id, fields);
        let says = check_says(progress, &self.object_id, fields, &computer);
        let mut col = div().flex().flex_col().gap(px(8.)).min_w(px(220.)).child(
            Label::new(credential_status_text(&status))
                .text_size(px(14.))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(theme::fg()),
        );
        if is_template {
            col = col.child(muted_line(
                "A template does not connect; each credential made from it does.",
            ));
        } else if self.machines.ready {
            if gate.can_connect || gate.can_check {
                let busy = !self.busy.is_empty();
                let mut actions = div().flex().flex_wrap().gap(px(6.));
                if gate.can_connect && can_connect(fields) {
                    actions = actions.child(
                        act_button("connect", connect_label(&status), busy).on_click(cx.listener(
                            move |this, _, _, cx| {
                                if !busy {
                                    this.run("connect", cx)
                                }
                            },
                        )),
                    );
                }
                if gate.can_check {
                    let here = gate.check_here;
                    let id = self.object_id.clone();
                    let f = fields.clone();
                    actions = actions.child(
                        act_button("check", "Check now", waiting_for_check)
                            .tooltip(format!(
                                "{} tests the login now",
                                if computer.is_empty() {
                                    "The computer serving it"
                                } else {
                                    &computer
                                }
                            ))
                            .on_click(move |_, _, cx| {
                                if !waiting_for_check {
                                    Progress::global(cx)
                                        .update(cx, |p, cx| p.check(&id, &f, here, 0, cx));
                                }
                            }),
                    );
                }
                if gate.can_connect {
                    actions = actions.child(
                        act_button("disconnect", "Disconnect", busy)
                            .tooltip("Stops a sign-in running on this computer. The saved login stays on the credential and keeps working until the site refuses it - clear its Cookie or key properties to remove it.")
                            .on_click(cx.listener(move |this, _, _, cx| if !busy { this.run("disconnect", cx) })),
                    );
                }
                col = col.child(actions);
            }
            if !gate.reason.is_empty() {
                col = col.child(muted_line(gate.reason.clone()));
            }
            if status == "connecting" {
                let text = if self.started_here {
                    connecting_hint(fields, "this computer", &hands_to)
                } else {
                    let err = field_str(fields, "error");
                    if err.is_empty() {
                        connecting_hint(fields, "", "")
                    } else {
                        err.to_string()
                    }
                };
                col = col.child(muted_line(text));
            }
            if let Some((text, tone)) = says {
                col = col.child(tone_line("says", text, tone));
            }
        }
        let error = if self.action_error.is_empty() {
            self.poll_error.clone()
        } else {
            self.action_error.clone()
        };
        if !error.is_empty() {
            col = col.child(tone_line("error", error, "error"));
        }
        col
    }
}

// ── CredentialReconnect ──────────────────────────────────────────────

/// One credential's Reconnect, as the views' popover offers it.
pub(crate) struct CredentialReconnect {
    id: String,
    machines: Machines,
    /// This popover pressed Reconnect (the sign-in window is on this computer).
    pressed: bool,
    started: bool,
    _subs: Vec<Subscription>,
}

impl CredentialReconnect {
    pub(crate) fn new(id: String, cx: &mut Context<Self>) -> Self {
        let store = Store::global(cx);
        store.update(cx, |s, cx| {
            if s.object(&id).is_some() {
                s.reload_object(&id, cx);
            } else {
                s.ensure_object(&id, cx);
            }
        });
        let subs = vec![
            cx.observe(&store, |_, _, cx| cx.notify()),
            cx.observe(&Progress::global(cx), |_, _, cx| cx.notify()),
        ];
        load_machines(cx, |this: &mut Self, m, _| this.machines = m);
        Self {
            id,
            machines: Machines {
                machines: vec![],
                this_machine: String::new(),
                ready: false,
            },
            pressed: false,
            started: false,
            _subs: subs,
        }
    }
}

impl Render for CredentialReconnect {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let store = Store::global(cx).read(cx);
        let slot = store.object_slot(&self.id).cloned();
        let open_id = self.id.clone();
        let open = act_button("open", "Open credential", false)
            .on_click(move |_, _, cx| nav::navigate(Route::Object(open_id.clone()), cx));
        let mut col = div().flex().flex_col().items_start().gap(px(6.));
        let object = match slot {
            Some(ObjectSlot::Failed(error)) => {
                return col
                    .child(tone_line("load-error", error.to_string(), "error"))
                    .child(open);
            }
            Some(ObjectSlot::Ready(o)) if self.machines.ready => o,
            _ => return col.child(muted_line("Loading…")).child(open),
        };
        let fields = &object.fields;
        if !self.started {
            // A sign-in already under way, or a Check another device asked for: follow it here too.
            self.started = true;
            let id = self.id.clone();
            let f = fields.clone();
            let connecting = field_str(fields, "status") == "connecting";
            Progress::global(cx).update(cx, |p, cx| {
                if connecting {
                    p.reconnect(&id, true, cx);
                }
                p.follow_check(&id, &f, cx);
            });
        }
        let name = match field_str(fields, "name") {
            "" => "this credential",
            n => n,
        }
        .to_string();
        let status = match field_str(fields, "status") {
            "" => "missing",
            s => s,
        }
        .to_string();
        let served_by = served_by_machine_id(fields, &self.machines.machines);
        let computer = if served_by.is_empty() {
            String::new()
        } else {
            machine_name(&self.machines.machines, &served_by)
        };
        let paired = roostr_core::harness::harness(cx).is_some();
        let this_machine = self.machines.this_machine.clone();
        let gate = action_gate(paired, &this_machine, &served_by, &computer, fields);
        let hands_to = if !this_machine.is_empty() && this_machine != served_by {
            computer.clone()
        } else {
            String::new()
        };
        let progress = Progress::global(cx).read(cx);
        let state = progress.reconnect_state(&self.id).cloned();
        let running = state
            .as_ref()
            .is_some_and(|s| matches!(s.phase, ReconnectPhase::Starting | ReconnectPhase::Waiting));
        let waiting_for_check = progress.checking(&self.id, fields);
        let says = check_says(progress, &self.id, fields, &computer);
        col = col.child(
            div()
                .flex()
                .child(
                    Label::new(name.clone())
                        .text_size(px(13.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(theme::fg()),
                )
                .child(
                    Label::new(format!(" · {}", credential_status_text(&status)))
                        .text_size(px(13.))
                        .text_color(theme::fg()),
                ),
        );
        let connectable = can_connect(fields);
        if (connectable && gate.can_connect) || gate.can_check {
            let mut actions = div().flex().flex_wrap().gap(px(6.));
            if connectable && gate.can_connect {
                let id = self.id.clone();
                actions = actions.child(
                    // Busy while the sign-in starts or waits: the kit loading
                    // state (spinner, no clicks) on the accent-bordered button.
                    act_button("run", format!("{} {name}", connect_label(&status)), false)
                        .loading(running)
                        .when(running, |b| b.icon(IconName::Loader))
                        .border_color(theme::accent())
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if !running {
                                this.pressed = true;
                                Progress::global(cx)
                                    .update(cx, |p, cx| p.reconnect(&id, false, cx));
                            }
                        })),
                );
            }
            if gate.can_check {
                let here = gate.check_here;
                let id = self.id.clone();
                let f = fields.clone();
                actions = actions.child(
                    act_button("check", "Check now", waiting_for_check)
                        .tooltip(format!(
                            "{} tests the login now",
                            if computer.is_empty() {
                                "The computer serving it"
                            } else {
                                &computer
                            }
                        ))
                        .on_click(move |_, _, cx| {
                            if !waiting_for_check {
                                Progress::global(cx)
                                    .update(cx, |p, cx| p.check(&id, &f, here, 0, cx));
                            }
                        }),
                );
            }
            col = col.child(actions);
        }
        if !connectable {
            col = col.child(muted_line(format!(
                "{name} signs in with pasted keys - open it to replace them."
            )));
        } else if !gate.can_connect && !gate.reason.is_empty() {
            col = col.child(muted_line(gate.reason.clone()));
        }
        match state.as_ref().map(|s| s.phase) {
            Some(ReconnectPhase::Starting) => {
                col = col.child(muted_line("Opening the sign-in on this computer…"))
            }
            p if p == Some(ReconnectPhase::Waiting) || (p.is_none() && status == "connecting") => {
                let detail = if self.pressed {
                    connecting_hint(fields, "this computer", &hands_to)
                } else {
                    let from_progress = state
                        .as_ref()
                        .map(|s| field_str(&s.fields, "error").to_string())
                        .unwrap_or_default();
                    let own = field_str(fields, "error").to_string();
                    if !from_progress.is_empty() {
                        from_progress
                    } else if !own.is_empty() {
                        own
                    } else {
                        connecting_hint(fields, "", "")
                    }
                };
                col = col.child(muted_line(format!("Waiting for sign-in… {detail}")));
            }
            Some(ReconnectPhase::Connected) => {
                col = col.child(tone_line("connected", "Connected".into(), "ok"))
            }
            Some(ReconnectPhase::Failed) => {
                col = col.child(tone_line(
                    "failed",
                    state.map(|s| s.error).unwrap_or_default(),
                    "error",
                ))
            }
            _ => {}
        }
        if let Some((text, tone)) = says {
            col = col.child(tone_line("says", text, tone));
        }
        col.child(open)
    }
}

// ── CredentialCell ───────────────────────────────────────────────────

/// Credential status cell trigger for set views (children = trigger content):
/// opens a popover with each credential's Reconnect. Clicks never reach the
/// row or card under it.
#[derive(IntoElement)]
pub struct CredentialCell {
    id: ElementId,
    row: QueryRow,
    target: CredentialTarget,
    label: SharedString,
    children: Vec<AnyElement>,
}

impl CredentialCell {
    /// `target` from [`crate::reconnect_target`]; `label` is the trigger's tooltip.
    pub fn new(
        id: impl Into<ElementId>,
        row: QueryRow,
        target: CredentialTarget,
        label: SharedString,
    ) -> Self {
        Self {
            id: id.into(),
            row,
            target,
            label,
            children: Vec::new(),
        }
    }
}

impl ParentElement for CredentialCell {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.children.extend(elements);
    }
}

struct CellState {
    open: bool,
    /// Credentials to offer (None while resolving names).
    ids: Option<Vec<String>>,
    missing: Vec<String>,
    find_error: String,
    blocks: Vec<Entity<CredentialReconnect>>,
}

impl CellState {
    fn open(&mut self, row: &QueryRow, target: &CredentialTarget, cx: &mut Context<Self>) {
        self.open = true;
        self.missing.clear();
        self.find_error.clear();
        self.blocks.clear();
        match target {
            CredentialTarget::Credential(id) => self.set_ids(vec![id.clone()], cx),
            CredentialTarget::Error(error) => {
                self.ids = None;
                let error = error.to_string();
                let agents = if row.type_key == "agent" {
                    vec![]
                } else {
                    guest_agents(&row.fields)
                };
                let row_fields = row.fields.clone();
                let all = api::query_all(
                    json!({ "filters": [{ "key": "typeKey", "condition": "equal", "value": "credential" }] }),
                    cx,
                );
                let agent_rows = (!agents.is_empty()).then(|| {
                    api::query_all(
                        json!({ "filters": [{ "key": "id", "condition": "in", "value": agents }] }),
                        cx,
                    )
                });
                cx.spawn(async move |this, cx| {
                    let result: anyhow::Result<(Vec<QueryRow>, Vec<QueryRow>)> = async {
                        let all = all.await?;
                        let agents = match agent_rows {
                            Some(t) => t.await?,
                            None => vec![],
                        };
                        Ok((all, agents))
                    }
                    .await;
                    let _ = this.update(cx, |this, cx| match result {
                        Ok((all, agent_rows)) => {
                            let mut linked = linked_credential_ids(&row_fields);
                            for a in &agent_rows {
                                linked.extend(linked_credential_ids(&a.fields));
                            }
                            let refs: Vec<CredentialRef> = all
                                .iter()
                                .map(|c| CredentialRef {
                                    id: c.id.clone(),
                                    name: field_str(&c.fields, "name").to_string(),
                                })
                                .collect();
                            let mine: Vec<CredentialRef> = refs
                                .iter()
                                .filter(|c| linked.contains(&c.id))
                                .cloned()
                                .collect();
                            let found = credentials_for_error(&error, &mine, &refs);
                            this.missing = signed_out_names(&error)
                                .into_iter()
                                .filter(|n| !found.iter().any(|c| &c.name == n))
                                .collect();
                            this.set_ids(found.into_iter().map(|c| c.id).collect(), cx);
                        }
                        Err(e) => {
                            this.find_error = e.to_string();
                            this.set_ids(vec![], cx);
                        }
                    });
                })
                .detach();
            }
        }
        cx.notify();
    }

    fn set_ids(&mut self, ids: Vec<String>, cx: &mut Context<Self>) {
        self.blocks = ids
            .iter()
            .map(|id| cx.new(|cx| CredentialReconnect::new(id.clone(), cx)))
            .collect();
        self.ids = Some(ids);
        cx.notify();
    }

    fn close(&mut self, cx: &mut Context<Self>) {
        self.open = false;
        self.blocks.clear();
        cx.notify();
    }
}

impl RenderOnce for CredentialCell {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let state = window.use_keyed_state(self.id.clone(), cx, |_, _| CellState {
            open: false,
            ids: None,
            missing: vec![],
            find_error: String::new(),
            blocks: vec![],
        });
        let s = state.read(cx);
        let open = s.open;
        let row = Rc::new(self.row);
        let target = Rc::new(self.target);
        let mut root = div().relative().max_w_full().flex();
        let trigger_state = state.clone();
        // Kept as a plain div: it wraps the caller's arbitrary cell content
        // (ellipsised, full width), which the kit Button's fixed-height,
        // centred, padded content row would re-lay out.
        let trigger = div()
            .id(self.id.clone())
            .group("cred-cell")
            .relative()
            .max_w_full()
            .rounded(theme::radius::sm())
            .overflow_hidden()
            .whitespace_nowrap()
            .text_ellipsis()
            .cursor_pointer()
            .tooltip(tip(self.label.clone()))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click({
                let row = row.clone();
                let target = target.clone();
                move |_, _, cx| {
                    cx.stop_propagation();
                    trigger_state.update(cx, |s, cx| {
                        if s.open {
                            s.close(cx)
                        } else {
                            s.open(&row, &target, cx)
                        }
                    });
                }
            })
            .children(self.children)
            .child(
                div()
                    .absolute()
                    .inset_0()
                    .rounded(theme::radius::sm())
                    .border_1()
                    .border_color(transparent_black())
                    .group_hover("cred-cell", |s| s.border_color(theme::accent())),
            );
        root = root.child(trigger);
        if open {
            let names = match &*target {
                CredentialTarget::Error(e) => signed_out_names(e).join(", "),
                CredentialTarget::Credential(_) => String::new(),
            };
            let mut pop = div()
                .id("cred-pop")
                .w(px(300.))
                // `max-height: calc(100vh - 16px)`; it scrolls inside.
                .max_h(window.viewport_size().height - px(16.))
                .overflow_y_scroll()
                .flex()
                .flex_col()
                .gap(px(12.))
                .bg(theme::panel())
                .border_1()
                .border_color(theme::border())
                .rounded(theme::radius::menu())
                .px(px(12.))
                .py(px(10.))
                .text_size(px(13.))
                .text_color(theme::fg())
                .whitespace_normal()
                .cursor_default()
                .occlude()
                .on_click(|_, _, cx| cx.stop_propagation())
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_key_down({
                    let state = state.clone();
                    move |ev: &KeyDownEvent, _, cx| {
                        cx.stop_propagation();
                        if ev.keystroke.key == "escape" {
                            state.update(cx, |s, cx| s.close(cx));
                        }
                    }
                });
            let s = state.read(cx);
            match &s.ids {
                None => pop = pop.child(muted_line(format!("Finding {names}…"))),
                Some(_) => {
                    for block in &s.blocks {
                        pop = pop.child(block.clone());
                    }
                    for name in &s.missing {
                        pop = pop.child(muted_line(format!("No credential named {name}.")));
                    }
                    if !s.find_error.is_empty() {
                        pop = pop.child(
                            error_alert("cred-find-error", s.find_error.clone())
                                .text_size(px(12.5)),
                        );
                    }
                }
            }
            let close_state = state.clone();
            root = root
                .child(backdrop("cred-backdrop", move |_, cx| {
                    close_state.update(cx, |s, cx| s.close(cx))
                }))
                .child(below("cred-pop", 4., pop));
        }
        root
    }
}
