//! A credential's Status editor (`CredentialStatus.svelte`), its Reconnect
//! block as a view's popover offers it (`CredentialReconnect.svelte`), and
//! the view cell that opens that popover (`CredentialCell.svelte`).

use std::collections::HashMap;
use std::rc::Rc;

use roostr_core::create::this_machine_id;
use roostr_core::serving::{MachineRow, fetch_machines, machine_name, served_by_machine_id};
use roostr_core::types::{field_str, guest_agents};
use roostr_core::{ObjectSlot, QueryRow, Route, State, api, nav};
use roostr_ui::ty::{self, Weights as _};
use roostr_ui::{Alert, Glyph, KitSize, Label, Theme, radius};
use rune::{Cursor, El, ScrollSpec, col, el, row, text};
use serde_json::json;

use crate::credentials::{
    CredentialRef, CredentialTarget, Progress, ReconnectPhase, action_gate, can_connect, changed,
    check_says, connect_label, connecting_hint, credential_status_text, credentials_for_error,
    linked_credential_ids, run_action, signed_out_names,
};
use crate::widgets::{act_button, backdrop, below, later, tip};

/// A muted note line, 12.5pt at line height 1.45.
fn muted_line(t: &Theme, line: impl Into<String>) -> El {
    text(line.into())
        .font(ty::ui(12.5).lh(12.5 * 1.45))
        .color(t.muted)
        .wrap()
}

/// A Check / sign-in outcome: "ok" a semibold label, "error" the kit
/// destructive alert (`key` keys it), anything else a muted line.
fn tone_line(t: &Theme, key: &str, line: String, tone: &str) -> El {
    match tone {
        "ok" => text(line)
            .font(ty::ui(12.5).semibold())
            .color(t.fg)
            .wrap()
            .key(key),
        "error" => Alert::error(key, line)
            .size(KitSize::XSmall)
            .text_size(12.5)
            .build(t),
        _ => muted_line(t, line).key(key),
    }
}

/// Machines and this computer's id, loaded once per view (the web's onMount).
#[derive(Clone, Default)]
struct Machines {
    machines: Vec<MachineRow>,
    this_machine: String,
    ready: bool,
}

/// Loads the machine roster and this computer's id, then hands them to `set`.
fn load_machines(s: &State, set: impl FnOnce(&mut State, Machines) + 'static) {
    let roster = fetch_machines(s);
    let here = this_machine_id(s);
    s.rt.spawn(async move |cx| {
        let machines = roster.await.map(|(_, m)| m).unwrap_or_default();
        let this_machine = here.await;
        cx.update(|s| {
            set(
                s,
                Machines {
                    machines,
                    this_machine,
                    ready: true,
                },
            )
        });
    })
    .detach();
}

/// Where a credential's serving computer is and what this window may run for it.
struct Place {
    computer: String,
    gate: crate::credentials::Gate,
    hands_to: String,
}

fn place(s: &State, machines: &Machines, fields: &roostr_core::Fields) -> Place {
    let served_by = served_by_machine_id(fields, &machines.machines);
    let computer = if served_by.is_empty() {
        String::new()
    } else {
        machine_name(&machines.machines, &served_by)
    };
    let paired = s.harness.is_some();
    let this_machine = &machines.this_machine;
    let gate = action_gate(paired, this_machine, &served_by, &computer, fields);
    let hands_to = if !this_machine.is_empty() && *this_machine != served_by {
        computer.clone()
    } else {
        String::new()
    };
    Place {
        computer,
        gate,
        hands_to,
    }
}

fn check_tooltip(computer: &str) -> String {
    format!(
        "{} tests the login now",
        if computer.is_empty() {
            "The computer serving it"
        } else {
            computer
        }
    )
}

// ── CredentialStatus ─────────────────────────────────────────────────

/// The Status editor of a credential: the status in words and the actions that change it.
#[derive(Default)]
struct StatusState {
    /// Which open this is (a reopen ignores the loads of the one before).
    open: u64,
    machines: Machines,
    busy: &'static str,
    action_error: String,
    /// This window started the sign-in that is running.
    started_here: bool,
}

#[derive(Default)]
struct Statuses {
    opened: u64,
    by_id: HashMap<String, StatusState>,
}

/// Opens a credential's Status editor afresh (GPUI made a new view per open) and loads
/// the machine roster.
pub(crate) fn open_credential_status(s: &mut State, object_id: &str) {
    let slot = s.slot::<Statuses>();
    slot.opened += 1;
    let open = slot.opened;
    slot.by_id.insert(
        object_id.to_string(),
        StatusState {
            open,
            ..StatusState::default()
        },
    );
    let id = object_id.to_string();
    load_machines(s, move |s, m| {
        if let Some(st) = s.slot::<Statuses>().by_id.get_mut(&id)
            && st.open == open
        {
            st.machines = m;
        }
    });
}

/// Drops a credential's Status editor state.
pub(crate) fn forget(s: &mut State, object_id: &str) {
    s.slot::<Statuses>().by_id.remove(object_id);
}

fn run(s: &mut State, object_id: &str, action: &'static str) {
    let Some(st) = s.slot::<Statuses>().by_id.get_mut(object_id) else {
        return;
    };
    if !st.busy.is_empty() {
        return;
    }
    st.busy = action;
    st.action_error.clear();
    let open = st.open;
    let task = run_action(action, object_id, s);
    let id = object_id.to_string();
    s.rt.spawn(async move |cx| {
        let result = task.await;
        cx.update(|s| {
            changed(&id, s);
            let Some(st) = s.slot::<Statuses>().by_id.get_mut(&id) else {
                return;
            };
            if st.open != open {
                return;
            }
            match &result {
                Ok(r) => {
                    if action == "connect" {
                        st.started_here = r.status == "connecting";
                    }
                }
                Err(error) => {
                    st.action_error = error.to_string();
                    if action == "connect" {
                        st.started_here = false;
                    }
                }
            }
            if action == "disconnect" {
                st.started_here = false;
            }
            st.busy = "";
        });
    })
    .detach();
}

/// A credential's Status editor; `poll_error` is the pane's connect poll giving up.
/// Opens itself on first draw.
pub(crate) fn credential_status(
    s: &State,
    _cx: &mut rune::Cx<State>,
    t: &Theme,
    _layer: &mut Vec<El>,
    object_id: &str,
    poll_error: &str,
) -> El {
    let Some(object) = s.store.object(object_id) else {
        return el();
    };
    let none = StatusState::default();
    let st = match s.get::<Statuses>().and_then(|x| x.by_id.get(object_id)) {
        Some(st) => st,
        None => {
            let id = object_id.to_string();
            later(s, move |s| {
                if !s.slot::<Statuses>().by_id.contains_key(&id) {
                    open_credential_status(s, &id);
                }
            });
            &none
        }
    };
    let fields = &object.fields;
    let is_template = object.type_key == "template";
    let status = match field_str(fields, "status") {
        "" => "missing",
        s => s,
    }
    .to_string();
    let Place {
        computer,
        gate,
        hands_to,
    } = place(s, &st.machines, fields);
    let no_progress = Progress::default();
    let progress = s.get::<Progress>().unwrap_or(&no_progress);
    // A Check another device asked for: follow it here too.
    if !is_template && progress.should_follow(object_id, fields) {
        let id = object_id.to_string();
        let f = fields.clone();
        later(s, move |s| Progress::follow_check(s, &id, &f));
    }
    let waiting_for_check = progress.checking(object_id, fields);
    let says = check_says(progress, object_id, fields, &computer);
    let mut c = col().key("credential-status").gap(8.0).min_w(220.0).child(
        Label::new(credential_status_text(&status))
            .text_size(14.0)
            .weight(ty::SEMIBOLD)
            .color(t.fg)
            .build(t),
    );
    if is_template {
        c = c.child(muted_line(
            t,
            "A template does not connect; each credential made from it does.",
        ));
    } else if st.machines.ready {
        if gate.can_connect || gate.can_check {
            let busy = !st.busy.is_empty();
            let mut actions = row().flex_wrap().gap(6.0);
            if gate.can_connect && can_connect(fields) {
                let id = object_id.to_string();
                actions = actions.child(
                    act_button(t, "connect", connect_label(&status), busy)
                        .on_press(move |s: &mut State| {
                            if !busy {
                                run(s, &id, "connect")
                            }
                        })
                        .build(t),
                );
            }
            if gate.can_check {
                let here = gate.check_here;
                let id = object_id.to_string();
                let f = fields.clone();
                actions = actions.child(
                    act_button(t, "check", "Check now", waiting_for_check)
                        .tooltip(check_tooltip(&computer))
                        .on_press(move |s: &mut State| {
                            if !waiting_for_check {
                                Progress::check(s, &id, &f, here, 0);
                            }
                        })
                        .build(t),
                );
            }
            if gate.can_connect {
                let id = object_id.to_string();
                actions = actions.child(
                    act_button(t, "disconnect", "Disconnect", busy)
                        .tooltip("Stops a sign-in running on this computer. The saved login stays on the credential and keeps working until the site refuses it - clear its Cookie or key properties to remove it.")
                        .on_press(move |s: &mut State| {
                            if !busy {
                                run(s, &id, "disconnect")
                            }
                        })
                        .build(t),
                );
            }
            c = c.child(actions);
        }
        if !gate.reason.is_empty() {
            c = c.child(muted_line(t, gate.reason.clone()));
        }
        if status == "connecting" {
            let line = if st.started_here {
                connecting_hint(fields, "this computer", &hands_to)
            } else {
                let err = field_str(fields, "error");
                if err.is_empty() {
                    connecting_hint(fields, "", "")
                } else {
                    err.to_string()
                }
            };
            c = c.child(muted_line(t, line));
        }
        if let Some((line, tone)) = says {
            c = c.child(tone_line(t, "says", line, tone));
        }
    }
    let error = if st.action_error.is_empty() {
        poll_error.to_string()
    } else {
        st.action_error.clone()
    };
    if !error.is_empty() {
        c = c.child(tone_line(t, "error", error, "error"));
    }
    c
}

// ── CredentialReconnect ──────────────────────────────────────────────

/// One credential's Reconnect, as the views' popover offers it.
#[derive(Clone)]
struct ReconnectBlock {
    id: String,
    machines: Machines,
    /// This popover pressed Reconnect (the sign-in window is on this computer).
    pressed: bool,
    started: bool,
}

/// The block for credential `id` in the cell `key`'s current open, if still there.
fn block_mut<'a>(
    s: &'a mut State,
    key: &str,
    open: u64,
    id: &str,
) -> Option<&'a mut ReconnectBlock> {
    let cell = s.slot::<Cells>().0.get_mut(key)?;
    if cell.opened != open {
        return None;
    }
    cell.blocks.iter_mut().find(|b| b.id == id)
}

/// Starts a block: reload the credential and load the machine roster.
fn new_block(s: &mut State, key: &str, open: u64, id: &str) -> ReconnectBlock {
    if s.store.object(id).is_some() {
        s.store.reload_object(id);
    } else {
        s.store.ensure_object(id);
    }
    let (key, bid) = (key.to_string(), id.to_string());
    load_machines(s, move |s, m| {
        if let Some(b) = block_mut(s, &key, open, &bid) {
            b.machines = m;
        }
    });
    ReconnectBlock {
        id: id.to_string(),
        machines: Machines::default(),
        pressed: false,
        started: false,
    }
}

fn reconnect_block(s: &State, t: &Theme, cell_key: &str, open: u64, b: &ReconnectBlock) -> El {
    let open_id = b.id.clone();
    let open_button = act_button(t, "open", "Open credential", false)
        .on_press(move |s: &mut State| nav::navigate(Route::Object(open_id.clone()), s))
        .build(t);
    let c = col()
        .key(format!("cred-block-{}", b.id))
        .items_start()
        .gap(6.0);
    let object = match s.store.object_slot(&b.id) {
        Some(ObjectSlot::Failed(error)) => {
            return c
                .child(tone_line(t, "load-error", error.clone(), "error"))
                .child(open_button);
        }
        Some(ObjectSlot::Ready(o)) if b.machines.ready => o.clone(),
        _ => return c.child(muted_line(t, "Loading…")).child(open_button),
    };
    let fields = &object.fields;
    if !b.started {
        // A sign-in already under way, or a Check another device asked for: follow it here too.
        let (key, id) = (cell_key.to_string(), b.id.clone());
        let f = fields.clone();
        let connecting = field_str(fields, "status") == "connecting";
        later(s, move |s| {
            let Some(b) = block_mut(s, &key, open, &id) else {
                return;
            };
            if b.started {
                return;
            }
            b.started = true;
            if connecting {
                Progress::reconnect(s, &id, true);
            }
            Progress::follow_check(s, &id, &f);
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
    let Place {
        computer,
        gate,
        hands_to,
    } = place(s, &b.machines, fields);
    let no_progress = Progress::default();
    let progress = s.get::<Progress>().unwrap_or(&no_progress);
    let state = progress.reconnect_state(&b.id).cloned();
    let running = state
        .as_ref()
        .is_some_and(|s| matches!(s.phase, ReconnectPhase::Starting | ReconnectPhase::Waiting));
    let waiting_for_check = progress.checking(&b.id, fields);
    let says = check_says(progress, &b.id, fields, &computer);
    let mut c = c.child(
        row()
            .child(
                Label::new(name.clone())
                    .text_size(13.0)
                    .weight(ty::SEMIBOLD)
                    .color(t.fg)
                    .build(t),
            )
            .child(
                Label::new(format!(" · {}", credential_status_text(&status)))
                    .text_size(13.0)
                    .color(t.fg)
                    .build(t),
            ),
    );
    let connectable = can_connect(fields);
    if (connectable && gate.can_connect) || gate.can_check {
        let mut actions = row().flex_wrap().gap(6.0);
        if connectable && gate.can_connect {
            let (key, id) = (cell_key.to_string(), b.id.clone());
            // Busy while the sign-in starts or waits: the kit loading state
            // (spinner, no presses) on the accent-bordered button.
            let mut run = act_button(
                t,
                "run",
                format!("{} {name}", connect_label(&status)),
                false,
            )
            .loading(running)
            .border_color(t.accent);
            if running {
                run = run.icon(Glyph::Loader);
            }
            actions = actions.child(
                run.on_press(move |s: &mut State| {
                    if !running {
                        if let Some(b) = block_mut(s, &key, open, &id) {
                            b.pressed = true;
                        }
                        Progress::reconnect(s, &id, false);
                    }
                })
                .build(t),
            );
        }
        if gate.can_check {
            let here = gate.check_here;
            let id = b.id.clone();
            let f = fields.clone();
            actions = actions.child(
                act_button(t, "check", "Check now", waiting_for_check)
                    .tooltip(check_tooltip(&computer))
                    .on_press(move |s: &mut State| {
                        if !waiting_for_check {
                            Progress::check(s, &id, &f, here, 0);
                        }
                    })
                    .build(t),
            );
        }
        c = c.child(actions);
    }
    if !connectable {
        c = c.child(muted_line(
            t,
            format!("{name} signs in with pasted keys - open it to replace them."),
        ));
    } else if !gate.can_connect && !gate.reason.is_empty() {
        c = c.child(muted_line(t, gate.reason.clone()));
    }
    match state.as_ref().map(|s| s.phase) {
        Some(ReconnectPhase::Starting) => {
            c = c.child(muted_line(t, "Opening the sign-in on this computer…"))
        }
        p if p == Some(ReconnectPhase::Waiting) || (p.is_none() && status == "connecting") => {
            let detail = if b.pressed {
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
            c = c.child(muted_line(t, format!("Waiting for sign-in… {detail}")));
        }
        Some(ReconnectPhase::Connected) => {
            c = c.child(tone_line(t, "connected", "Connected".into(), "ok"))
        }
        Some(ReconnectPhase::Failed) => {
            c = c.child(tone_line(
                t,
                "failed",
                state.map(|s| s.error).unwrap_or_default(),
                "error",
            ))
        }
        _ => {}
    }
    if let Some((line, tone)) = says {
        c = c.child(tone_line(t, "says", line, tone));
    }
    c.child(open_button)
}

// ── CredentialCell ───────────────────────────────────────────────────

/// A cell's popover: open or not, and the credentials it offers.
#[derive(Default)]
struct CellState {
    open: bool,
    /// Which open this is (a reopen ignores the lookups of the one before).
    opened: u64,
    /// Credentials to offer (None while resolving names).
    ids: Option<Vec<String>>,
    missing: Vec<String>,
    find_error: String,
    blocks: Vec<ReconnectBlock>,
}

/// Every credential cell's popover state, keyed by the cell's key.
#[derive(Default)]
struct Cells(HashMap<String, CellState>);

fn set_ids(s: &mut State, key: &str, open: u64, ids: Vec<String>) {
    let blocks = ids.iter().map(|id| new_block(s, key, open, id)).collect();
    if let Some(cell) = s.slot::<Cells>().0.get_mut(key)
        && cell.opened == open
    {
        cell.blocks = blocks;
        cell.ids = Some(ids);
    }
}

fn open_cell(s: &mut State, key: &str, row: &QueryRow, target: &CredentialTarget) {
    let cell = s.slot::<Cells>().0.entry(key.to_string()).or_default();
    cell.open = true;
    cell.opened += 1;
    let open = cell.opened;
    cell.missing.clear();
    cell.find_error.clear();
    cell.blocks.clear();
    match target {
        CredentialTarget::Credential(id) => set_ids(s, key, open, vec![id.clone()]),
        CredentialTarget::Error(error) => {
            cell.ids = None;
            let error = error.clone();
            let agents = if row.type_key == "agent" {
                vec![]
            } else {
                guest_agents(&row.fields)
            };
            let row_fields = row.fields.clone();
            let all = api::query_all(
                json!({ "filters": [{ "key": "typeKey", "condition": "equal", "value": "credential" }] }),
                s,
            );
            let agent_rows = (!agents.is_empty()).then(|| {
                api::query_all(
                    json!({ "filters": [{ "key": "id", "condition": "in", "value": agents }] }),
                    s,
                )
            });
            let key = key.to_string();
            s.rt.spawn(async move |cx| {
                let result: anyhow::Result<(Vec<QueryRow>, Vec<QueryRow>)> = async {
                    let all = all.await?;
                    let agents = match agent_rows {
                        Some(t) => t.await?,
                        None => vec![],
                    };
                    Ok((all, agents))
                }
                .await;
                cx.update(|s| {
                    if s.slot::<Cells>()
                        .0
                        .get(&key)
                        .is_none_or(|c| c.opened != open)
                    {
                        return;
                    }
                    match result {
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
                            let missing = signed_out_names(&error)
                                .into_iter()
                                .filter(|n| !found.iter().any(|c| &c.name == n))
                                .collect();
                            if let Some(cell) = s.slot::<Cells>().0.get_mut(&key) {
                                cell.missing = missing;
                            }
                            set_ids(s, &key, open, found.into_iter().map(|c| c.id).collect());
                        }
                        Err(e) => {
                            if let Some(cell) = s.slot::<Cells>().0.get_mut(&key) {
                                cell.find_error = e.to_string();
                            }
                            set_ids(s, &key, open, vec![]);
                        }
                    }
                });
            })
            .detach();
        }
    }
}

fn close_cell(s: &mut State, key: &str) {
    if let Some(cell) = s.slot::<Cells>().0.get_mut(key) {
        cell.open = false;
        cell.blocks.clear();
    }
}

/// Credential status cell trigger for set views (children = trigger content):
/// opens a popover with each credential's Reconnect. Presses never reach the
/// row or card under it.
pub struct CredentialCell {
    key: String,
    row: QueryRow,
    target: CredentialTarget,
    label: String,
    children: Vec<El>,
}

impl CredentialCell {
    /// `target` from [`crate::reconnect_target`]; `label` is the trigger's tooltip.
    pub fn new(
        key: impl Into<String>,
        row: QueryRow,
        target: CredentialTarget,
        label: impl Into<String>,
    ) -> Self {
        Self {
            key: key.into(),
            row,
            target,
            label: label.into(),
            children: Vec::new(),
        }
    }

    /// Trigger content.
    pub fn child(mut self, child: El) -> Self {
        self.children.push(child);
        self
    }

    /// The trigger keyed `key`; while open, its popover (`cred-pop`) and the outside-press
    /// backdrop (`cred-backdrop`) go into `layer`.
    pub fn build(self, s: &State, _cx: &mut rune::Cx<State>, t: &Theme, layer: &mut Vec<El>) -> El {
        let key = self.key;
        let cell = s.get::<Cells>().and_then(|c| c.0.get(&key));
        let row = Rc::new(self.row);
        let target = Rc::new(self.target);
        // Kept as a plain element: it wraps the caller's arbitrary cell content
        // (ellipsised, full width), which a kit Button's content row would re-lay out.
        let trigger = tip(
            el().key(key.clone())
                .hover_group()
                .min_w(0.0)
                .radius(radius::SM)
                .clip_rounded(radius::SM)
                .cursor(Cursor::Pointer)
                .on_press({
                    let (key, row, target) = (key.clone(), row.clone(), target.clone());
                    move |s: &mut State| {
                        let is_open = s.slot::<Cells>().0.get(&key).is_some_and(|c| c.open);
                        if is_open {
                            close_cell(s, &key)
                        } else {
                            open_cell(s, &key, &row, &target)
                        }
                    }
                })
                .children(self.children)
                .child(
                    el().pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0))
                        .radius(radius::SM)
                        .border(1.0, t.accent)
                        .group_opacity(0.0, 1.0),
                ),
            self.label,
        );
        if let Some(cell) = cell.filter(|c| c.open) {
            let names = match &*target {
                CredentialTarget::Error(e) => signed_out_names(e).join(", "),
                CredentialTarget::Credential(_) => String::new(),
            };
            let mut body = col().gap(12.0).px(12.0).py(10.0);
            match &cell.ids {
                None => body = body.child(muted_line(t, format!("Finding {names}…"))),
                Some(_) => {
                    for b in &cell.blocks {
                        body = body.child(reconnect_block(s, t, &key, cell.opened, b));
                    }
                    for name in &cell.missing {
                        body = body.child(muted_line(t, format!("No credential named {name}.")));
                    }
                    if !cell.find_error.is_empty() {
                        body = body.child(
                            Alert::error("cred-find-error", cell.find_error.clone())
                                .size(KitSize::XSmall)
                                .text_size(12.5)
                                .build(t),
                        );
                    }
                }
            }
            // At most as tall as the window leaves it (anchored_fit); it scrolls inside.
            let close_key = key.clone();
            let pop = el()
                .key("cred-pop")
                .w(300.0)
                .min_h(0.0)
                .fill(t.panel)
                .border(1.0, t.border)
                .radius(radius::MENU)
                .clip_rounded(radius::MENU)
                .cursor(Cursor::Default)
                .scroll(ScrollSpec {
                    stick_bottom: false,
                })
                .modal(true, move |s: &mut State| close_cell(s, &close_key))
                .child(body);
            let close_key = key.clone();
            backdrop(layer, "cred-backdrop", move |s| close_cell(s, &close_key));
            below(layer, &key, 4.0, pop);
        }
        trigger
    }
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
        let body = credential_status(s, cx, t, &mut layer, "c1", "Gave up waiting");
        roostr_ui::root(body, layer)
    }

    #[test]
    fn the_status_view_says_the_status_its_progress_and_the_poll_error() {
        let vault = Vault::new(
            vec![object(
                "c1",
                "credential",
                &[
                    ("name", Value::string("Gmail")),
                    ("status", Value::string("connecting")),
                    ("service", Value::string("google-account")),
                    ("account", Value::string("me@example.com")),
                ],
            )],
            vec![],
        );
        let mut app = App::new(vault, view);
        app.s().store.ensure_object("c1");
        app.until("the editor's machines", |s| {
            s.get::<Statuses>()
                .and_then(|x| x.by_id.get("c1"))
                .is_some_and(|st| st.machines.ready)
        });
        let texts: Vec<String> = app
            .rt
            .elements()
            .into_iter()
            .filter_map(|e| e.text)
            .collect();
        for want in [
            "Waiting for sign-in",
            "Set Served by to a computer first.",
            "Sign in to me@example.com in the browser window gws opens on its computer.",
            "Gave up waiting",
        ] {
            assert!(texts.iter().any(|t| t == want), "{want} in {texts:?}");
        }
    }
}
