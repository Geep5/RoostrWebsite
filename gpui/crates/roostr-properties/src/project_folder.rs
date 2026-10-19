//! Under an agent's Project folder (`ProjectFolder.svelte`): Choose… opens
//! the macOS folder window through this computer's harness (offered only
//! when this computer is the agent's Served by, since the folder must exist
//! there) and shows the folder as that computer sees it.

use std::rc::Rc;

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use roostr_core::create::this_machine_id;
use roostr_core::harness::harness;
use roostr_core::io;
use roostr_core::types::field_str;
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

fn note(text: &'static str) -> Div {
    div()
        .text_size(px(12.))
        .line_height(relative(1.4))
        .opacity(0.6)
        .child(text)
}

impl Render for ProjectFolder {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(object) = Store::global(cx).read(cx).object(&self.object_id) else {
            return div();
        };
        let path = field_str(&object.fields, "repo_path").trim().to_string();
        let served_by = field_str(&object.fields, "served_by").to_string();
        let local = !self.here.is_empty() && self.here == served_by;
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
                        div()
                            .id("choose")
                            .self_start()
                            .px(px(10.))
                            .py(px(4.))
                            .rounded(theme::radius::sm())
                            .border_1()
                            .border_color(theme::border())
                            .text_size(px(13.))
                            .when(picking, |d| d.opacity(0.6))
                            .when(!picking, |d| d.cursor_pointer())
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if !picking {
                                    this.choose(cx)
                                }
                            }))
                            .child(if picking { "Choose in the window that opened…" } else { "Choose…" }),
                    )
                } else if !served_by.is_empty() {
                    d.child(note("The folder must be on this agent's computer; it checks it there, and a problem shows on its Error."))
                } else {
                    d.child(note("Set Served by first - the folder must be on that computer."))
                }
            })
            .when_some(self.status.clone(), |d, (ok, text)| {
                d.child(
                    div()
                        .text_size(px(12.))
                        .text_color(if ok { rgb(0x4caf50) } else { rgb(0xf55522) })
                        .child(format!("{} {text}", if ok { "✓" } else { "✗" })),
                )
            })
            .child(note("Optional. When set, the agent works there: it's told the folder is its project (and its git repo), reads its AGENTS.md, and its shell starts there."))
    }
}
