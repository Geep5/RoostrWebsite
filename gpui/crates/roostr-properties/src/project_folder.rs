//! Under an agent's Project folder (`ProjectFolder.svelte`): Choose… opens
//! the macOS folder window through this computer's harness (offered only
//! when this computer is the agent's Served by, since the folder must exist
//! there) and shows the folder as that computer sees it.

use std::collections::HashMap;
use std::rc::Rc;

use roostr_core::State;
use roostr_core::create::this_machine_id;
use roostr_core::types::field_str;
use roostr_ui::{Alert, KitSize, Theme, outline_button, radius, ty};
use rune::{Cx, El, col, text};
use serde_json::{Value as Json, json};

use crate::widgets::later;

/// One object's Project folder controls (GPUI's `ProjectFolder` entity).
#[derive(Default)]
struct FolderState {
    /// This window's computer.
    here: String,
    picking: bool,
    /// (ok, text)
    status: Option<(bool, String)>,
    /// The path the status is for.
    checked: String,
}

#[derive(Default)]
struct Folders(HashMap<String, FolderState>);

/// Opens `object_id`'s Project folder controls afresh (as GPUI made a new
/// view per open): learns this computer's machine id.
pub(crate) fn open_project_folder(s: &mut State, object_id: &str) {
    s.slot::<Folders>()
        .0
        .insert(object_id.to_string(), FolderState::default());
    let here = this_machine_id(s);
    let id = object_id.to_string();
    s.rt.spawn(async move |cx| {
        let here = here.await;
        cx.update(|s| {
            if let Some(f) = s.slot::<Folders>().0.get_mut(&id) {
                f.here = here;
            }
        });
    })
    .detach();
}

/// Drops `object_id`'s Project folder state.
pub(crate) fn forget(s: &mut State, object_id: &str) {
    s.slot::<Folders>().0.remove(object_id);
}

/// Asks this computer's harness how it sees `path`; a later path wins.
fn check(s: &mut State, id: &str, path: String) {
    let Some(f) = s.slot::<Folders>().0.get_mut(id) else {
        return;
    };
    if f.checked == path {
        return;
    }
    f.checked = path.clone();
    f.status = None;
    let Some(h) = s.harness.clone() else { return };
    let body = json!({ "path": path });
    let task =
        s.rt.background(async move { h.post_json("/workspace/check", &body).await });
    let id = id.to_string();
    s.rt.spawn(async move |cx| {
        let Ok(r) = task.await else { return };
        cx.update(|s| {
            let Some(f) = s.slot::<Folders>().0.get_mut(&id) else {
                return;
            };
            if f.checked != path {
                return;
            }
            let ok = r.get("ok").and_then(Json::as_bool) == Some(true);
            let text = if ok {
                r.get("summary").and_then(Json::as_str).unwrap_or("")
            } else {
                r.get("error").and_then(Json::as_str).unwrap_or("Not found")
            };
            f.status = Some((ok, text.to_string()));
        });
    })
    .detach();
}

/// Forgets the status when the folder isn't checked here.
fn uncheck(s: &mut State, id: &str) {
    if let Some(f) = s.slot::<Folders>().0.get_mut(id) {
        f.checked.clear();
        f.status = None;
    }
}

/// Opens the macOS folder window through the harness; the picked path goes to `on_save`.
fn choose(s: &mut State, id: &str, on_save: Rc<dyn Fn(&mut State, String)>) {
    let Some(h) = s.harness.clone() else { return };
    let Some(f) = s.slot::<Folders>().0.get_mut(id) else {
        return;
    };
    f.picking = true;
    let task =
        s.rt.background(async move { h.post_json("/workspace/pick", &json!({})).await });
    let id = id.to_string();
    s.rt.spawn(async move |cx| {
        let result = task.await;
        cx.update(|s| {
            let Some(f) = s.slot::<Folders>().0.get_mut(&id) else {
                return;
            };
            f.picking = false;
            let status = match result {
                Ok(r) => match r.get("path").and_then(Json::as_str) {
                    Some(path) => {
                        on_save(s, path.to_string());
                        return;
                    }
                    None => r
                        .get("error")
                        .and_then(Json::as_str)
                        .map(|e| (false, e.to_string())),
                },
                Err(e) => Some((false, e.to_string())),
            };
            if let Some(status) = status {
                f.status = Some(status);
            }
        });
    })
    .detach();
}

/// A muted 12px explanatory note.
fn folder_note(t: &Theme, line: &'static str) -> El {
    text(line)
        .font(ty::ui(12.0).lh(12.0 * 1.4))
        .color(t.muted)
        .wrap()
}

/// The controls under `object_id`'s Project folder value; a picked folder goes to `on_save`.
pub(crate) fn project_folder(
    s: &State,
    _cx: &mut Cx<State>,
    t: &Theme,
    _layer: &mut Vec<El>,
    object_id: &str,
    on_save: Rc<dyn Fn(&mut State, String)>,
) -> El {
    let Some(object) = s.store.object(object_id) else {
        return col();
    };
    let id = object_id.to_string();
    let Some(me) = s.get::<Folders>().and_then(|f| f.0.get(object_id)) else {
        later(s, move |s| {
            if !s.slot::<Folders>().0.contains_key(&id) {
                open_project_folder(s, &id);
            }
        });
        return col();
    };
    // The computer the folder must be on: the agent's own, or for any
    // other object the computer its agents run on.
    let served_by = folder_computer(&object, |a| s.store.agent(a).map(|a| a.served_by.clone()));
    let is_agent = object.type_key == "agent";
    let path = field_str(&object.fields, "repo_path").trim().to_string();
    let local = !me.here.is_empty() && me.here == served_by;
    if !local || path.is_empty() {
        if !me.checked.is_empty() || me.status.is_some() {
            later(s, move |s| uncheck(s, &id));
        }
    } else if me.checked != path {
        later(s, move |s| check(s, &id, path));
    }
    let picking = me.picking;
    let mut root = col().gap(6.0).mt(8.0);
    root = root.child(if local {
        // Greyed and inert while the macOS folder window is open.
        let id = object_id.to_string();
        roostr_ui::disabled(
            outline_button(
                "choose",
                if picking {
                    "Choose in the window that opened…"
                } else {
                    "Choose…"
                },
            )
            .radius(radius::SM)
            .px(10.0)
            .py(4.0),
            picking,
        )
        .on_press(move |s: &mut State| {
            if !picking {
                choose(s, &id, on_save.clone())
            }
        })
        .build(t)
        .self_start()
    } else if !served_by.is_empty() {
        folder_note(
            t,
            if is_agent {
                "The folder must be on this agent's computer; it checks it there, and a problem shows on its Error."
            } else {
                "The folder must be on the computer this object's agents run on; it's checked there, and a problem shows on this object's Error."
            },
        )
    } else if is_agent {
        folder_note(t, "Set Served by first - the folder must be on that computer.")
    } else {
        folder_note(
            t,
            "Its agents need a Served by first - the folder must be on that computer.",
        )
    });
    if let Some((ok, line)) = me.status.clone() {
        root = root.child(
            if ok {
                Alert::success("folder-status", line)
            } else {
                Alert::error("folder-status", line)
            }
            .size(KitSize::XSmall)
            .build(t),
        );
    }
    root.child(folder_note(
        t,
        if is_agent {
            "Optional. When set, the agent works there: it's told the folder is its project (and its git repo), reads its AGENTS.md, and its shell starts there."
        } else {
            "Optional. When set, this object's agents work there for it, instead of their own Project folder: they're told it's the project (and its git repo), read its AGENTS.md, and their shell starts there."
        },
    ))
}

/// The computer an object's Project folder must be on: its own Served by,
/// else (an object agents work on) the first of its agents that has one.
pub(crate) fn folder_computer(
    object: &roostr_core::types::Object,
    agent_served_by: impl Fn(&str) -> Option<String>,
) -> String {
    let own = field_str(&object.fields, "served_by");
    if !own.is_empty() || object.type_key == "agent" {
        return own.to_string();
    }
    roostr_core::types::guest_agents(&object.fields)
        .iter()
        .filter_map(|id| agent_served_by(id))
        .find(|s| !s.is_empty())
        .unwrap_or_default()
}
