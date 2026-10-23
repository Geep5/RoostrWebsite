//! Space settings on a space (`channel`) object's page (`SpaceManage.svelte`):
//! default type, members, join link + requests, key rotation, the bin, and
//! leaving / deleting the space.

use std::cell::Cell;
use std::rc::Rc;

use gpui_kit::component::WindowExt as _;
use gpui_kit::component::dialog::{Dialog, DialogEntrance};
use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use roostr_core::create::NOT_DEFAULTABLE;
use roostr_core::dates::show_date;
use roostr_core::invite::{SpaceJoinLink, join_url};
use roostr_core::objects::{object_icon, type_glyph};
use roostr_core::types::{Space, field_str};
use roostr_core::ui::{CheckboxIcon, tooltip};
use roostr_core::{Navigator, Route, Store, StoreEvent, Value, api, motion, nav, theme};
use serde_json::json;

use crate::local::{JoinRequest, clear_join_request, list_join_requests};
use crate::widgets::{Btn, FieldStyle, Select, Span, field, rich};

/// `binBusy` while the whole bin is being emptied.
const ALL: &str = "__all__";

struct BinRow {
    id: String,
    name: String,
    type_key: String,
    icon: String,
    done: bool,
}

/// The settings section of a space object's page.
pub struct SpaceManage {
    object_id: String,
    /// `None` until the first read of the bin.
    bin: Option<Vec<BinRow>>,
    /// The bin row being restored/deleted, [`ALL`] while emptying, else "".
    bin_busy: String,
    confirm_delete: bool,
    deleting: bool,
    confirm_empty: bool,
    empty_draft: Entity<InputState>,
    delete_draft: Entity<InputState>,
    npub_draft: Entity<InputState>,
    /// First click on Remove arms it, the second rotates the key.
    confirm_remove: String,
    copied_join: bool,
    /// The npub this view last added: its row rises in when it arrives
    /// (rows from sync just appear).
    added_member: Option<String>,
    join_requests: Vec<JoinRequest>,
    identity_error: SharedString,
    _subscriptions: Vec<Subscription>,
}

impl SpaceManage {
    pub fn new(object_id: String, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let store = Store::global(cx);
        store.update(cx, |store, cx| store.ensure_object(&object_id, cx));
        let empty_draft = cx.new(|cx| InputState::new(window, cx).placeholder("delete"));
        let delete_draft = cx.new(|cx| InputState::new(window, cx));
        let npub_draft =
            cx.new(|cx| InputState::new(window, cx).placeholder("npub1… of the person to invite"));
        let subscriptions = vec![
            cx.observe(&store, |_, _, cx| cx.notify()),
            cx.subscribe(&store, |this, _, event: &StoreEvent, cx| {
                // The space's name feeds the delete phrase.
                if matches!(event, StoreEvent::Committed(id) if *id == this.object_id) {
                    this.sync_delete_placeholder(cx);
                }
            }),
            cx.subscribe(&npub_draft, |this, _, event: &InputEvent, cx| {
                if let InputEvent::PressEnter { .. } = event {
                    this.add_member(cx);
                }
            }),
            cx.subscribe_in(
                &empty_draft,
                window,
                |this, _, event: &InputEvent, window, cx| match event {
                    InputEvent::PressEnter { .. } => {
                        if this.empty_armed(cx) && this.bin_busy.is_empty() {
                            this.empty_bin(window, cx);
                        }
                    }
                    InputEvent::Change => cx.notify(),
                    _ => {}
                },
            ),
            cx.subscribe_in(
                &delete_draft,
                window,
                |this, _, event: &InputEvent, window, cx| match event {
                    InputEvent::PressEnter { .. } => {
                        if this.delete_armed(cx) && !this.deleting {
                            this.remove_space(window, cx);
                        }
                    }
                    InputEvent::Change => cx.notify(),
                    _ => {}
                },
            ),
        ];
        let mut this = Self {
            object_id,
            bin: None,
            bin_busy: String::new(),
            confirm_delete: false,
            deleting: false,
            confirm_empty: false,
            empty_draft,
            delete_draft,
            npub_draft,
            confirm_remove: String::new(),
            copied_join: false,
            added_member: None,
            join_requests: Vec::new(),
            identity_error: SharedString::default(),
            _subscriptions: subscriptions,
        };
        this.load_bin(cx);
        this.load_identity(cx);
        this
    }

    // ------------------------------------------------------------- reads

    fn space_info<'a>(&self, store: &'a Store) -> Option<&'a Space> {
        store.space(&self.object_id)
    }

    /// `SpaceJSON.owner` is "" when this identity administers the space.
    fn is_owner(&self, cx: &App) -> bool {
        self.space_info(Store::global(cx).read(cx))
            .is_none_or(|s| s.owner.is_empty())
    }

    fn space_name(&self, cx: &App) -> String {
        Store::global(cx)
            .read(cx)
            .object(&self.object_id)
            .map(|o| o.field_str("name").trim().to_string())
            .unwrap_or_default()
    }

    /// The space's own name, not the word "delete": the gate should cost a
    /// look at WHICH space this is. A space with no name keeps the action's word.
    fn delete_phrase(&self, cx: &App) -> String {
        let name = self.space_name(cx);
        if !name.is_empty() {
            name
        } else if self.is_owner(cx) {
            "delete".into()
        } else {
            "leave".into()
        }
    }

    fn delete_armed(&self, cx: &App) -> bool {
        self.delete_draft.read(cx).value().trim().to_lowercase()
            == self.delete_phrase(cx).to_lowercase()
    }

    fn empty_armed(&self, cx: &App) -> bool {
        self.empty_draft.read(cx).value().trim().to_lowercase() == "delete"
    }

    fn owner_npub(cx: &App) -> String {
        Store::global(cx)
            .read(cx)
            .settings
            .as_ref()
            .and_then(|s| s.npub.clone())
            .unwrap_or_default()
    }

    fn sync_delete_placeholder(&mut self, cx: &mut Context<Self>) {
        if !self.confirm_delete {
            return;
        }
        let phrase = self.delete_phrase(cx);
        if let Some(window) = cx.windows().first().copied() {
            let state = self.delete_draft.clone();
            let _ = window.update(cx, |_, window, cx| {
                state.update(cx, |s, cx| s.set_placeholder(phrase, window, cx))
            });
        }
    }

    // ------------------------------------------------------------- bin

    /// Deleted objects of this space, read to exhaustion: "Empty bin" vanishes
    /// exactly this list, so a capped read would delete a page and then
    /// report the bin emptied.
    fn load_bin(&mut self, cx: &mut Context<Self>) {
        let task = api::query_all(
            json!({ "includeDeleted": true, "filters": [{ "key": "channel", "condition": "equal", "value": self.object_id }] }),
            cx,
        );
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                match result {
                    Ok(records) => {
                        this.bin = Some(
                            records
                                .into_iter()
                                .filter(|r| r.deleted == Some(true))
                                .map(|r| BinRow {
                                    name: Some(field_str(&r.fields, "name"))
                                        .filter(|s| !s.is_empty())
                                        .unwrap_or("Untitled")
                                        .to_string(),
                                    icon: field_str(&r.fields, "iconEmoji").to_string(),
                                    done: r.fields.get("done").and_then(|v| v.bool_value)
                                        == Some(true),
                                    type_key: r.type_key,
                                    id: r.id,
                                })
                                .collect(),
                        );
                    }
                    Err(error) => log::warn!("bin read failed: {error:#}"),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn restore_object(&mut self, id: String, cx: &mut Context<Self>) {
        self.bin_busy = id.clone();
        cx.notify();
        let task = api::note::restore(&id, cx);
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                if let Err(error) = result {
                    log::warn!("restore failed: {error:#}");
                }
                this.bin_busy.clear();
                this.load_bin(cx);
                Store::global(cx).update(cx, |store, cx| store.refresh_all(cx));
            });
        })
        .detach();
    }

    /// Empty bin: vanish every deleted object in this space at once.
    fn empty_bin(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(ids) = self
            .bin
            .as_ref()
            .map(|b| b.iter().map(|r| r.id.clone()).collect::<Vec<_>>())
        else {
            return;
        };
        if ids.is_empty() {
            return;
        }
        self.bin_busy = ALL.into();
        cx.notify();
        let task = api::note::vanish(&ids, cx);
        let dialog_window = window.window_handle();
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                if let Err(error) = result {
                    log::warn!("empty bin failed: {error:#}");
                }
                this.bin = Some(Vec::new());
                // Escape may already have closed it while the vanish ran.
                if this.confirm_empty {
                    this.confirm_empty = false;
                    let _ = dialog_window.update(cx, |_, window, cx| window.close_dialog(cx));
                }
                this.bin_busy.clear();
                cx.notify();
            });
        })
        .detach();
    }

    fn vanish_object(
        &mut self,
        id: String,
        name: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let answer = window.prompt(
            PromptLevel::Warning,
            &format!("Permanently delete \"{name}\"? This cannot be undone — on any device."),
            None,
            &["OK", "Cancel"],
            cx,
        );
        cx.spawn(async move |this, cx| {
            if answer.await != Ok(0) {
                return;
            }
            let Ok(task) = this.update(cx, |this, cx| {
                this.bin_busy = id.clone();
                cx.notify();
                api::note::vanish(std::slice::from_ref(&id), cx)
            }) else {
                return;
            };
            if let Err(error) = task.await {
                log::warn!("vanish failed: {error:#}");
            }
            let _ = this.update(cx, |this, cx| {
                this.load_bin(cx);
                this.bin_busy.clear();
            });
        })
        .detach();
    }

    // ------------------------------------------------------------- members

    fn load_identity(&mut self, cx: &mut Context<Self>) {
        self.load_join_requests(cx);
    }

    fn load_join_requests(&mut self, cx: &mut Context<Self>) {
        let task = list_join_requests(cx);
        let space = self.object_id.clone();
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                match result {
                    Ok(requests) => {
                        this.join_requests =
                            requests.into_iter().filter(|r| r.space == space).collect();
                        this.identity_error = SharedString::default();
                    }
                    Err(error) => {
                        this.join_requests.clear();
                        this.identity_error = error.to_string().into();
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn copy_join_link(&mut self, cx: &mut Context<Self>) {
        let owner = Self::owner_npub(cx);
        if owner.is_empty() {
            return;
        }
        let store = Store::global(cx).read(cx);
        let name = store
            .object(&self.object_id)
            .map(|o| o.field_str("name").to_string())
            .filter(|n| !n.is_empty());
        let relays = store
            .settings
            .as_ref()
            .map(|s| s.relays.clone())
            .unwrap_or_default();
        let url = join_url(&SpaceJoinLink {
            space: self.object_id.clone(),
            name,
            owner,
            relays,
        });
        cx.write_to_clipboard(ClipboardItem::new_string(url));
        self.copied_join = true;
        cx.notify();
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(std::time::Duration::from_millis(1500))
                .await;
            let _ = this.update(cx, |this, cx| {
                this.copied_join = false;
                cx.notify();
            });
        })
        .detach();
    }

    fn approve_request(&mut self, request: JoinRequest, cx: &mut Context<Self>) {
        let add = api::space::member_add(&self.object_id, &request.requester_npub, None, cx);
        cx.spawn(async move |this, cx| {
            let result = async {
                add.await?;
                this.update(cx, |_, cx| clear_join_request(&request.key, cx))?
                    .await
            }
            .await;
            let _ = this.update(cx, |this, cx| {
                match result {
                    Ok(()) => {
                        this.load_join_requests(cx);
                        Store::global(cx).update(cx, |store, cx| store.refresh_all(cx));
                    }
                    Err(error) => this.identity_error = error.to_string().into(),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn deny_request(&mut self, request: JoinRequest, cx: &mut Context<Self>) {
        let task = clear_join_request(&request.key, cx);
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| match result {
                Ok(()) => this.load_join_requests(cx),
                Err(error) => {
                    this.identity_error = error.to_string().into();
                    cx.notify();
                }
            });
        })
        .detach();
    }

    fn add_member(&mut self, cx: &mut Context<Self>) {
        let npub = self.npub_draft.read(cx).value().trim().to_string();
        if npub.is_empty() {
            return;
        }
        self.added_member = Some(npub.clone());
        let task = api::space::member_add(&self.object_id, &npub, None, cx);
        let draft = self.npub_draft.clone();
        cx.spawn(async move |_, cx| match task.await {
            Ok(_) => {
                cx.update(|cx| {
                    if let Some(window) = cx.active_window() {
                        let _ = window.update(cx, |_, window, cx| {
                            draft.update(cx, |s, cx| s.set_value("", window, cx))
                        });
                    }
                    Store::global(cx).update(cx, |store, cx| store.refresh_all(cx));
                });
            }
            Err(error) => log::warn!("add member failed: {error:#}"),
        })
        .detach();
    }

    fn remove_member(&mut self, npub: String, cx: &mut Context<Self>) {
        if self.confirm_remove != npub {
            // First click arms, second executes: the key rotates and the member loses future access.
            self.confirm_remove = npub;
            cx.notify();
            return;
        }
        self.confirm_remove.clear();
        cx.notify();
        let task = api::space::member_remove(&self.object_id, &npub, cx);
        cx.spawn(async move |_, cx| match task.await {
            Ok(_) => {
                cx.update(|cx| Store::global(cx).update(cx, |store, cx| store.refresh_all(cx)));
            }
            Err(error) => log::warn!("remove member failed: {error:#}"),
        })
        .detach();
    }

    fn rotate_key(&mut self, cx: &mut Context<Self>) {
        let task = api::space::key_rotate(&self.object_id, cx);
        cx.spawn(async move |_, cx| match task.await {
            Ok(_) => {
                cx.update(|cx| Store::global(cx).update(cx, |store, cx| store.refresh_all(cx)));
            }
            Err(error) => log::warn!("key rotate failed: {error:#}"),
        })
        .detach();
    }

    fn set_default_type(&mut self, key: &str, cx: &mut Context<Self>) {
        api::note::set_field(&self.object_id, "default_type", Value::string(key), cx)
            .detach_and_log_err(cx);
    }

    // ------------------------------------------------------------- delete / leave

    fn open_delete(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.confirm_delete = true;
        let phrase = self.delete_phrase(cx);
        self.delete_draft.update(cx, |s, cx| {
            s.set_value("", window, cx);
            s.set_placeholder(phrase, window, cx);
        });
        self.open_confirm(window, cx);
        self.delete_draft.focus_handle(cx).focus(window, cx);
        cx.notify();
    }

    fn open_empty(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.confirm_empty = true;
        self.empty_draft
            .update(cx, |s, cx| s.set_value("", window, cx));
        self.open_confirm(window, cx);
        cx.notify();
    }

    /// Owner: vanish the space and every object in it (the ledger entry rides
    /// sync, so it is gone everywhere and can't resurrect). Member: leave —
    /// only this identity's copies go.
    fn remove_space(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let dialog_window = window.window_handle();
        self.deleting = true;
        cx.notify();
        let id = self.object_id.clone();
        let owner = self.is_owner(cx);
        cx.spawn(async move |this, cx| {
            let result: anyhow::Result<()> = async {
                if owner {
                    // Every object, not a page of them: anything left behind is unreachable.
                    let records = cx
                        .update(|cx| api::query_all(json!({ "filters": [{ "key": "channel", "condition": "equal", "value": id }] }), cx))
                        .await?;
                    let mut ids = vec![id.clone()];
                    ids.extend(records.into_iter().map(|r| r.id));
                    cx.update(|cx| api::note::vanish(&ids, cx)).await?;
                } else {
                    cx.update(|cx| api::space::leave(&id, cx)).await?;
                }
                Ok(())
            }
            .await;
            let _ = this.update(cx, |this, cx| {
                this.deleting = false;
                match result {
                    Ok(()) => {
                        if this.confirm_delete {
                            this.confirm_delete = false;
                            let _ = dialog_window.update(cx, |_, window, cx| window.close_dialog(cx));
                        }
                        let store = Store::global(cx);
                        store.update(cx, |store, cx| store.refresh_all(cx));
                        let navigator = Navigator::global(cx);
                        navigator.update(cx, |nav, cx| nav.forget(&id, cx));
                        // Switching space restores that space's tabs; with none left, home.
                        let next = store.read(cx).channels.iter().find(|s| s.id != id).map(|s| s.id.clone());
                        match next {
                            Some(next) => navigator.update(cx, |nav, cx| nav.use_space(&next, cx)),
                            None => nav::navigate(Route::Home, cx),
                        }
                    }
                    Err(error) => log::warn!("removing the space failed: {error:#}"),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Escape, a backdrop press and Cancel all land here: the empty-bin
    /// confirmation always closes, the delete flow stays put once the vanish
    /// is in flight. Returns whether the dialog may close.
    fn dismiss_confirm(&mut self, cx: &mut Context<Self>) -> bool {
        let close = if self.confirm_empty {
            self.confirm_empty = false;
            true
        } else if self.confirm_delete {
            let close = !self.deleting;
            self.confirm_delete = !close;
            close
        } else {
            true
        };
        cx.notify();
        close
    }

    /// A Cancel button: dismiss, and take the dialog down with it.
    fn cancel_confirm(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.dismiss_confirm(cx) {
            window.close_dialog(cx);
        }
    }

    /// Open the kit dialog for the confirmation just armed. It is rebuilt
    /// from this view every frame, so the confirm button arms as the phrase
    /// is typed.
    fn open_confirm(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let this = cx.weak_entity();
        let gone = Rc::new(Cell::new(false));
        window.open_dialog(cx, move |dialog, window, cx| {
            match this.upgrade() {
                Some(view) => view.read(cx).confirm_dialog(&this, dialog, window, cx),
                None => {
                    // The view went away under its dialog: take the dialog down once.
                    if !gone.replace(true) {
                        window.defer(cx, |window, cx| window.close_dialog(cx));
                    }
                    dialog
                }
            }
        });
    }

    /// The empty-bin or delete/leave confirmation (`.del-modal`): title,
    /// warning, "Type <phrase> to confirm.", the field, Cancel and the red
    /// confirm that stays disabled until the phrase matches.
    fn confirm_dialog(
        &self,
        this: &WeakEntity<Self>,
        dialog: Dialog,
        window: &mut Window,
        cx: &App,
    ) -> Dialog {
        let cancel = |id: &'static str| {
            let this = this.clone();
            Btn::manage(id, "Cancel").on_click(move |_, window, cx| {
                let _ = this.update(cx, |this, cx| this.cancel_confirm(window, cx));
            })
        };
        let (title, text, phrase, draft, confirm) = if self.confirm_empty {
            let n = self.bin.as_ref().map_or(0, Vec::len);
            let armed = self.empty_armed(cx) && self.bin_busy.is_empty();
            let this = this.clone();
            let confirm = Btn::destructive(
                "empty-confirm",
                if self.bin_busy == ALL {
                    "Deleting…"
                } else {
                    "Delete forever"
                },
            )
            .disabled(!armed)
            .on_click(move |_, window, cx| {
                let _ = this.update(cx, |this, cx| this.empty_bin(window, cx));
            });
            (
                "Empty the bin?".to_string(),
                format!(
                    "Permanently deletes {n} object{} — on every device, forever. This cannot be undone.",
                    if n == 1 { "" } else { "s" }
                ),
                "delete".to_string(),
                &self.empty_draft,
                div()
                    .flex()
                    .justify_end()
                    .gap(px(8.))
                    .child(cancel("empty-cancel"))
                    .child(confirm),
            )
        } else {
            let is_owner = self.is_owner(cx);
            let space_name = self.space_name(cx);
            let armed = self.delete_armed(cx) && !self.deleting;
            let shown = if space_name.is_empty() {
                "this space".to_string()
            } else {
                space_name
            };
            let (title, text) = if is_owner {
                (
                    format!("Delete {shown} for everyone?"),
                    "This permanently deletes the space and everything in it — for every member, on every device, forever. This cannot be undone.",
                )
            } else {
                (
                    format!("Leave {shown}?"),
                    "Only your own copies go: the space and everything in it disappear from your devices. The other members keep the space and everything in it, including what you added. This cannot be undone.",
                )
            };
            let confirm_label = match (is_owner, self.deleting) {
                (true, true) => "Deleting…",
                (true, false) => "Delete for everyone",
                (false, true) => "Leaving…",
                (false, false) => "Leave space",
            };
            let this = this.clone();
            let confirm = Btn::destructive("delete-confirm", confirm_label)
                .disabled(!armed)
                .on_click(move |_, window, cx| {
                    let _ = this.update(cx, |this, cx| this.remove_space(window, cx));
                });
            (
                title,
                text.to_string(),
                self.delete_phrase(cx),
                &self.delete_draft,
                div()
                    .flex()
                    .justify_end()
                    .gap(px(8.))
                    .child(cancel("delete-cancel").disabled(self.deleting))
                    .child(confirm),
            )
        };
        let dismiss = this.clone();
        dialog
            .w(px(380.))
            .close_button(false)
            .entrance(DialogEntrance::FadeSlide)
            // Enter belongs to the field, which submits once armed.
            .on_ok(|_, _, _| false)
            .on_cancel(move |_, _, cx| {
                dismiss
                    .update(cx, |this, cx| this.dismiss_confirm(cx))
                    .unwrap_or(true)
            })
            .p(px(18.))
            .gap(px(14.))
            .rounded(px(12.))
            .bg(theme::panel())
            .border_color(theme::border())
            .text_color(theme::fg())
            .title(
                div()
                    .text_size(px(15.))
                    .line_height(px(19.))
                    .font_weight(FontWeight::BOLD)
                    .child(title),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(10.))
                    .child(manage_hint(text))
                    .child(type_to_confirm(&phrase, window))
                    .child(
                        div()
                            .mt(px(8.))
                            .child(field(draft, Self::delete_input_style(), window, cx).w_full()),
                    ),
            )
            .footer(confirm)
    }

    fn delete_input_style() -> FieldStyle {
        let mut ring = theme::red();
        ring.a = 0.25;
        FieldStyle {
            bg: theme::bg(),
            radius: 6.,
            px: 10.,
            py: 7.,
            size: 14.,
            focus: theme::red(),
            ring: Some(ring),
            centered: false,
        }
    }
}

fn h3(text: &'static str) -> Div {
    div()
        .mt(px(14.))
        .mb(px(2.))
        .text_size(px(11.))
        .line_height(px(14.))
        .font_weight(FontWeight::BOLD)
        .text_color(theme::muted())
        .child(text.to_uppercase())
}

fn manage_hint(text: impl Into<SharedString>) -> Div {
    div()
        .max_w(px(560.))
        .text_size(px(12.))
        .line_height(px(15.))
        .text_color(theme::muted())
        .child(text.into())
}

fn short_npub(npub: &str) -> String {
    format!("{}…", npub.chars().take(20).collect::<String>())
}

impl Render for SpaceManage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let store = Store::global(cx);
        let s = store.read(cx);
        let Some(object) = s.object(&self.object_id) else {
            return div().id("space-manage");
        };
        let space = self.space_info(s).cloned();
        let is_owner = space.as_ref().is_none_or(|sp| sp.owner.is_empty());
        let members = space
            .as_ref()
            .map(|sp| sp.members.clone())
            .unwrap_or_default();
        let channel_count = s.channels.len();

        // Default type: what this space makes when Roostr doesn't ask.
        let default_type = Some(object.field_str("default_type"))
            .filter(|k| !k.is_empty())
            .unwrap_or("note")
            .to_string();
        let mut defaultable: Vec<_> = s
            .types
            .iter()
            .filter(|t| t.space == self.object_id && !NOT_DEFAULTABLE.contains(&t.key.as_str()))
            .collect();
        defaultable.sort_by(|a, b| {
            let an = if a.name.is_empty() { &a.key } else { &a.name };
            let bn = if b.name.is_empty() { &b.key } else { &b.name };
            an.to_lowercase().cmp(&bn.to_lowercase())
        });
        let type_options: Vec<(String, SharedString)> = defaultable
            .iter()
            .map(|t| {
                let icon = if t.icon.is_empty() {
                    type_glyph(s, &t.key, &self.object_id)
                } else {
                    t.icon.clone()
                };
                let name = if t.name.is_empty() { &t.key } else { &t.name };
                (t.key.clone(), format!("{icon} {name}").into())
            })
            .collect();
        // (id, name, is task, done, icon) per binned object.
        type BinView = (String, String, bool, bool, String);
        let bin_rows: Option<Vec<BinView>> = self.bin.as_ref().map(|bin| {
            bin.iter()
                .map(|b| {
                    let is_task = s.layout_of(&b.type_key) == "task";
                    (
                        b.id.clone(),
                        b.name.clone(),
                        is_task,
                        b.done,
                        object_icon(s, &b.icon, &b.type_key, &self.object_id),
                    )
                })
                .collect()
        });
        let owner_npub = Self::owner_npub(cx);
        let key_id = space
            .as_ref()
            .map_or("?".to_string(), |sp| sp.key_id.to_string());

        let mut root = div()
            .id("space-manage")
            .flex()
            .flex_col()
            .gap(px(10.))
            .mt(px(8.))
            .mb(px(24.))
            // `.manage`: `margin: 8px 24px 24px 48px`, `8px 16px 24px` at ≤720px.
            .map(|d| {
                if roostr_core::layout::is_mobile(window) {
                    d.mx(px(16.))
                } else {
                    d.mr(px(24.)).ml(px(48.))
                }
            })
            .text_color(theme::fg());

        // ── Default type
        let this = cx.entity();
        root = root
            .child(h3("Default type"))
            .child(manage_hint(
                "What a new object becomes when Roostr doesn't ask: search's \"Create object\", a collection's New, and the first choice in the New menu.",
            ))
            .child(
                div().flex().child(
                    Select::new("default-type", type_options, default_type)
                        .bg(theme::panel())
                        .min_w(px(220.))
                        .on_pick({
                            let this = this.clone();
                            move |key, _, cx| this.update(cx, |this, cx| this.set_default_type(key, cx))
                        }),
                ),
            );

        // ── Members
        root = root.child(h3("Members")).child(manage_hint(
            "Everyone holding this space's key can read and write every object in it. Adding an npub gift-wraps the space key to it over your relays (NIP-59) — nothing to copy, their device just receives the space. Removing a member rotates the key.",
        ));
        for m in &members {
            let npub = m.npub.clone();
            let armed = self.confirm_remove == m.npub;
            let row = div()
                .flex()
                .items_center()
                .gap(px(10.))
                .text_size(px(13.))
                .child(
                    div()
                        .id(SharedString::from(format!("npub-{}", m.npub)))
                        .font_family(theme::fonts::MONO)
                        .tooltip(tooltip(m.npub.clone()))
                        .child(short_npub(&m.npub)),
                )
                .child(
                    div()
                        .text_size(px(12.))
                        .text_color(theme::muted())
                        .child(m.role.clone()),
                )
                .child(
                    Btn::manage(
                        SharedString::from(format!("remove-{}", m.npub)),
                        if armed {
                            "Confirm: rotate key"
                        } else {
                            "Remove"
                        },
                    )
                    .danger_hover()
                    .on_click(
                        cx.listener(move |this, _, _, cx| this.remove_member(npub.clone(), cx)),
                    ),
                );
            // A member you just added rises in (opacity, y 6 → 0, 150ms).
            let row = if self.added_member.as_deref() == Some(m.npub.as_str()) {
                let enter = motion::use_motion(
                    SharedString::from(format!("member-enter-{}", m.npub)),
                    window,
                    cx,
                    motion::MotionState::default,
                );
                let frame = enter.update(cx, |state, cx| {
                    state.frame(
                        motion::Motion::new(motion::Target::default())
                            .initial(motion::Target::default().y(6.).opacity(0.))
                            .transition(motion::CONTROL.transition()),
                        window,
                        cx,
                    )
                });
                frame.apply(row)
            } else {
                row
            };
            root = root.child(row);
        }
        if members.is_empty() {
            root = root.child(manage_hint("No members yet — it's just you."));
        }
        let npub_field = field(
            &self.npub_draft,
            FieldStyle {
                bg: theme::panel(),
                radius: 7.,
                px: 10.,
                py: 5.,
                size: 13.,
                focus: theme::accent(),
                ring: None,
                centered: false,
            },
            window,
            cx,
        )
        .w(px(340.));
        root = root
            .child(
                div()
                    .flex()
                    .gap(px(8.))
                    .child(npub_field)
                    .child(Btn::manage("add-member", "Add member").on_click(cx.listener(|this, _, _, cx| this.add_member(cx))))
                    .child(
                        Btn::manage("copy-join", if self.copied_join { "Copied ✓" } else { "Copy join link" })
                            .disabled(owner_npub.is_empty())
                            .tooltip(if owner_npub.is_empty() {
                                "No key on this device yet"
                            } else {
                                "Copy a public link that lets anyone REQUEST to join — you approve each request here"
                            })
                            .on_click(cx.listener(|this, _, _, cx| this.copy_join_link(cx))),
                    ),
            )
            .child(manage_hint("The join link carries no key — it only lets someone knock. You approve each request below."));
        if !self.identity_error.is_empty() {
            root = root.child(manage_hint(self.identity_error.clone())).child(
                Btn::manage("retry-join", "Retry join requests")
                    .full_width()
                    .on_click(cx.listener(|this, _, _, cx| this.load_identity(cx))),
            );
        }

        // ── Join requests
        if !self.join_requests.is_empty() {
            root = root.child(h3("Join requests"));
            for r in &self.join_requests {
                let avatar = match r.picture.as_deref().filter(|p| !p.is_empty()) {
                    Some(picture) => img(SharedString::from(picture.to_string()))
                        .size(px(28.))
                        .rounded_full()
                        .flex_none()
                        .into_any_element(),
                    None => div()
                        .size(px(28.))
                        .rounded_full()
                        .flex_none()
                        .flex()
                        .items_center()
                        .justify_center()
                        .bg(theme::hover())
                        .text_size(px(14.))
                        .child("👤")
                        .into_any_element(),
                };
                let name = r.name.clone().filter(|n| !n.is_empty());
                let approve = r.clone();
                let deny = r.clone();
                root = root.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(10.))
                        .text_size(px(13.))
                        .child(avatar)
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .flex()
                                .flex_col()
                                .gap(px(1.))
                                .child(
                                    div()
                                        .text_size(px(13.))
                                        .font_weight(FontWeight::MEDIUM)
                                        .overflow_hidden()
                                        .whitespace_nowrap()
                                        .text_ellipsis()
                                        .child(
                                            name.clone()
                                                .unwrap_or_else(|| short_npub(&r.requester_npub)),
                                        ),
                                )
                                .when(name.is_some(), |d| {
                                    d.child(
                                        div()
                                            .id(SharedString::from(format!("req-npub-{}", r.key)))
                                            .font_family(theme::fonts::MONO)
                                            .text_size(px(11.))
                                            .text_color(theme::muted())
                                            .tooltip(tooltip(r.requester_npub.clone()))
                                            .child(short_npub(&r.requester_npub)),
                                    )
                                }),
                        )
                        .child(
                            div()
                                .text_size(px(12.))
                                .text_color(theme::muted())
                                .child(show_date(r.at as i64)),
                        )
                        .child(
                            Btn::manage(
                                SharedString::from(format!("approve-{}", r.key)),
                                "Approve",
                            )
                            .on_click(cx.listener(
                                move |this, _, _, cx| this.approve_request(approve.clone(), cx),
                            )),
                        )
                        .child(
                            Btn::manage(SharedString::from(format!("deny-{}", r.key)), "Deny")
                                .danger_hover()
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.deny_request(deny.clone(), cx)
                                })),
                        ),
                );
            }
        }

        // ── Key
        root = root.child(
            div()
                .flex()
                .items_center()
                .gap(px(10.))
                .child(manage_hint(format!("Space key #{key_id}")))
                .child(
                    Btn::manage("rotate-key", "Rotate key")
                        .on_click(cx.listener(|this, _, _, cx| this.rotate_key(cx))),
                ),
        );

        // ── Bin
        let bin_len = bin_rows.as_ref().map_or(0, Vec::len);
        root = root.child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .child(h3("Bin").mb_0())
                .when(bin_len > 0, |d| {
                    d.child(
                        Btn::manage("empty-bin", "Empty bin…")
                            .danger_hover()
                            .on_click(
                                cx.listener(|this, _, window, cx| this.open_empty(window, cx)),
                            ),
                    )
                }),
        );
        match bin_rows {
            None => root = root.child(manage_hint("Loading…")),
            Some(rows) if rows.is_empty() => root = root.child(manage_hint("Nothing in the bin.")),
            Some(rows) => {
                let mut list = div().flex().flex_col();
                for (id, name, is_task, done, icon) in rows {
                    let busy = self.bin_busy == id;
                    let leading = if is_task {
                        div()
                            .flex_none()
                            .child(CheckboxIcon::new(done).size(px(16.)).color(if done {
                                theme::accent()
                            } else {
                                theme::muted()
                            }))
                    } else {
                        div().flex_none().child(icon)
                    };
                    let (restore_id, vanish_id, vanish_name) =
                        (id.clone(), id.clone(), name.clone());
                    list = list.child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .py(px(4.))
                            .text_size(px(13.))
                            .child(leading)
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .text_ellipsis()
                                    .child(name),
                            )
                            .child(
                                Btn::manage(SharedString::from(format!("restore-{id}")), "Restore")
                                    .style(|b| b.set_text(11., FontWeight::NORMAL))
                                    .disabled(busy)
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.restore_object(restore_id.clone(), cx)
                                    })),
                            )
                            .child(
                                Btn::manage(
                                    SharedString::from(format!("vanish-{id}")),
                                    "Delete forever",
                                )
                                .style(|b| b.set_text(11., FontWeight::NORMAL))
                                .danger_hover()
                                .disabled(busy)
                                .on_click(cx.listener(
                                    move |this, _, window, cx| {
                                        this.vanish_object(
                                            vanish_id.clone(),
                                            vanish_name.clone(),
                                            window,
                                            cx,
                                        )
                                    },
                                )),
                            ),
                    );
                }
                root = root.child(list);
            }
        }

        // ── Danger zone: an owner keeps at least one space; a member may always leave.
        if !is_owner || channel_count > 1 {
            root = root.child(h3("Danger zone")).child(
                Btn::manage(
                    "remove-space",
                    if is_owner {
                        "Delete for everyone…"
                    } else {
                        "Leave space…"
                    },
                )
                .danger_hover()
                .full_width()
                .on_click(cx.listener(|this, _, window, cx| this.open_delete(window, cx))),
            );
        }

        root
    }
}

/// "Type **<phrase>** to confirm."
fn type_to_confirm(phrase: &str, window: &Window) -> Div {
    div().text_size(px(12.)).line_height(px(15.)).child(rich(
        &[
            Span::Plain("Type "),
            Span::Bold(phrase),
            Span::Plain(" to confirm."),
        ],
        theme::muted(),
        window,
    ))
}
