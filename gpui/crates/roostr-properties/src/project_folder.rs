//! Under an object's Project folder (`ProjectFolder.svelte`): Choose… opens
//! the macOS folder window through this computer's harness - offered only
//! where the folder must exist: an agent's Served by, or the computer of an
//! agent on the object (any computer when none is listed) - and shows the
//! folder as that computer sees it.

use std::rc::Rc;

use gpui_kit::component::Sizable as _;
use gpui_kit::component::alert::Alert;
use gpui_kit::component::label::Label;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use roostr_core::create::this_machine_id;
use roostr_core::harness::harness;
use roostr_core::io;
use roostr_core::types::field_str;
use roostr_core::ui::{disabled, outline_button};
use roostr_core::{Store, theme};
use serde_json::{Value as Json, json};

type SaveFn = Rc<dyn Fn(String, &mut App)>;

pub(crate) struct ProjectFolder {
    object_id: String,
    on_save: SaveFn,
    /// This window's computer.
    here: String,
    picking: bool,
    /// (ok, text)
    status: Option<(bool, String)>,
    /// The path the status is for.
    checked: String,
    _subs: Vec<Subscription>,
}

impl ProjectFolder {
    pub(crate) fn new(
        object_id: String,
        on_save: impl Fn(String, &mut App) + 'static,
        cx: &mut Context<Self>,
    ) -> Self {
        let here = this_machine_id(cx);
        cx.spawn(async move |this, cx| {
            let id = here.await;
            let _ = this.update(cx, |this, cx| {
                this.here = id;
                cx.notify();
            });
        })
        .detach();
        Self {
            object_id,
            on_save: Rc::new(on_save),
            here: String::new(),
            picking: false,
            status: None,
            checked: String::new(),
            _subs: vec![cx.observe(&Store::global(cx), |_, _, cx| cx.notify())],
        }
    }

    fn check(&mut self, path: String, cx: &mut Context<Self>) {
        self.checked = path.clone();
        self.status = None;
        let Some(h) = harness(cx) else { return };
        let task = io::spawn(cx, async move {
            h.post_json("/workspace/check", &json!({ "path": path }))
                .await
        });
        let path = self.checked.clone();
        cx.spawn(async move |this, cx| {
            let Ok(r) = task.await else { return };
            let _ = this.update(cx, |this, cx| {
                if this.checked != path {
                    return;
                }
                let ok = r.get("ok").and_then(Json::as_bool) == Some(true);
                let text = if ok {
                    r.get("summary")
                        .and_then(Json::as_str)
                        .unwrap_or("")
                        .to_string()
                } else {
                    r.get("error")
                        .and_then(Json::as_str)
                        .unwrap_or("Not found")
                        .to_string()
                };
                this.status = Some((ok, text));
                cx.notify();
            });
        })
        .detach();
    }

    fn choose(&mut self, cx: &mut Context<Self>) {
        let Some(h) = harness(cx) else { return };
        self.picking = true;
        let task = io::spawn(cx, async move {
            h.post_json("/workspace/pick", &json!({})).await
        });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                this.picking = false;
                match result {
                    Ok(r) => {
                        if let Some(path) = r.get("path").and_then(Json::as_str) {
                            (this.on_save)(path.to_string(), cx);
                        } else if let Some(e) = r.get("error").and_then(Json::as_str) {
                            this.status = Some((false, e.to_string()));
                        }
                    }
                    Err(e) => this.status = Some((false, e.to_string())),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }
}

/// A muted 12px explanatory note.
fn folder_note(text: &'static str) -> Label {
    crate::widgets::note(text, 12.).line_height(relative(1.4))
}

impl Render for ProjectFolder {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let store = Store::global(cx).read(cx);
        let Some(object) = store.object(&self.object_id) else {
            return div();
        };
        let path = field_str(&object.fields, "repo_path").trim().to_string();
        let is_agent = object.type_key == "agent";
        // The computers the folder must be on: an agent's own, or those of
        // the agents on this object (its Agent property). None listed: any
        // agent tagged here works on its own computer, so this one may pick.
        let computers: Vec<String> = if is_agent {
            vec![field_str(&object.fields, "served_by").to_string()]
        } else {
            roostr_core::types::guest_agents(&object.fields)
                .iter()
                .filter_map(|id| store.object(id))
                .map(|agent| field_str(&agent.fields, "served_by").to_string())
                .collect()
        };
        let local = !self.here.is_empty()
            && (computers.is_empty() || computers.iter().any(|c| *c == self.here));
        if !local || path.is_empty() {
            if !self.checked.is_empty() || self.status.is_some() {
                self.checked.clear();
                self.status = None;
            }
        } else if self.checked != path {
            self.check(path, cx);
        }
        let picking = self.picking;
        div()
            .flex()
            .flex_col()
            .gap(px(6.))
            .mt(px(8.))
            .map(|d| {
                if local {
                    d.child(
                        // Greyed and inert while the macOS folder window is open.
                        disabled(
                            outline_button(
                                "choose",
                                if picking { "Choose in the window that opened…" } else { "Choose…" },
                            )
                            .self_start()
                            .rounded(theme::radius::sm())
                            .px(px(10.))
                            .py(px(4.)),
                            picking,
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if !picking {
                                this.choose(cx)
                            }
                        })),
                    )
                } else if is_agent && computers.iter().all(String::is_empty) {
                    d.child(folder_note("Set Served by first - the folder must be on that computer."))
                } else if is_agent {
                    d.child(folder_note("The folder must be on this agent's computer; it checks it there, and a problem shows on its Error."))
                } else {
                    d.child(folder_note("The folder must be on the computer its agents run on - choose it there. An agent that can't find it says so in the chat."))
                }
            })
            .when_some(self.status.clone(), |d, (ok, text)| {
                d.child(
                    if ok {
                        Alert::success("folder-status", text)
                    } else {
                        Alert::error("folder-status", text)
                    }
                    .xsmall()
                    .text_size(px(12.)),
                )
            })
            .child(if is_agent {
                folder_note("Optional, a fallback: the agent works here on its own page and where no folder is set. Better: set the folder on the task or Project the work is for.")
            } else {
                folder_note("Optional. Agents working here use it: they're told it's the project (and its git repo), read its AGENTS.md, and their shell starts there. Tasks linked to a Project with a folder use the Project's.")
            })
    }
}
