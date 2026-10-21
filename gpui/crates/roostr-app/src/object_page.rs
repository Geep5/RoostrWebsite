//! The object page (`routes/app/object/[id]/+page.svelte`) and the space
//! home (`routes/app/+page.svelte`): icon, title, featured properties, notes,
//! the body for the object's kind, and the right "Chat & properties" pane.

use std::sync::Arc;

use gpui_kit::component::input::{Enter, InputEvent, Textarea, TextareaState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use roostr_core::layout::is_mobile;
use roostr_core::nav::{NavEvent, Navigator, Route};
use roostr_core::objects::{is_image_icon, object_icon};
use roostr_core::store::Store;
use roostr_core::types::{Object, Value, field_str, is_builtin_tool, is_locked_tool};
use roostr_core::{ObjectSlot, PaneState, StoreEvent, api, theme, ui};
use roostr_discussion::{ConversationDrawer, Discussion, DiscussionOptions};
use roostr_editor::{Editor, FileView};
use roostr_properties::{EmojiPicker, ObjectMeta, PropertyPanel, TypePanel};
use roostr_sets::{DataView, DataViewKind};
use roostr_settings::{InstallComputer, SpaceManage};

/// What the page body shows (decided by the object's type).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Kind {
    Space,
    Chat,
    File,
    Type,
    Relation,
    Query,
    Collection,
    Document { read_only: bool },
}

fn kind_of(object: &Object) -> Kind {
    match object.type_key.as_str() {
        "channel" => Kind::Space,
        "chat" => Kind::Chat,
        "file" => Kind::File,
        "type" => Kind::Type,
        "relation" => Kind::Relation,
        "query" | "set" => Kind::Query,
        "collection" => Kind::Collection,
        _ => Kind::Document {
            read_only: is_locked_tool(&object.fields),
        },
    }
}

enum Body {
    Space(Entity<SpaceManage>),
    Chat(Entity<Discussion>),
    File(Entity<FileView>),
    Type(Entity<TypePanel>, Entity<DataView>),
    Relation(Entity<PropertyPanel>),
    Data(Entity<DataView>),
    Document(Entity<Editor>),
}

pub struct ObjectPage {
    id: String,
    title: Entity<TextareaState>,
    /// The stored name the title last took: a refresh brings in a name only
    /// when it changed, never re-imposing one over a draft.
    title_synced: Option<String>,
    /// From the title's own focus event to its blur, which saves the draft.
    /// Window focus leaves a frame before that blur arrives, so it can't tell.
    title_editing: bool,
    show_emoji: bool,
    meta: Option<Entity<ObjectMeta>>,
    body: Option<(Kind, Body)>,
    pane: Option<(bool, Entity<ConversationDrawer>)>,
    opened: bool,
    /// A title edit that brought in newlines, to rewrite with spaces.
    pending_title_fix: Option<String>,
    /// Whether the title was last set read-only (built-in tools).
    title_read_only: bool,
    _subscriptions: Vec<Subscription>,
}

impl ObjectPage {
    pub fn new(id: String, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let store = Store::global(cx);
        store.update(cx, |store, cx| store.ensure_object(&id, cx));
        let title = cx.new(|cx| {
            TextareaState::new(window, cx)
                .auto_grow(1, 100)
                .placeholder("Untitled")
        });
        let subscriptions = vec![
            cx.observe_in(&store, window, |this, _, window, cx| this.sync(window, cx)),
            cx.subscribe_in(
                &store,
                window,
                |this, store, event: &StoreEvent, window, cx| {
                    // A load that failed (not synced yet) retries when the replica learns more.
                    let failed = matches!(
                        store.read(cx).object_slot(&this.id),
                        Some(ObjectSlot::Failed(_))
                    );
                    let retry = match event {
                        StoreEvent::Committed(id) => *id == this.id,
                        StoreEvent::Refreshed | StoreEvent::Reconnected => true,
                    };
                    if failed && retry {
                        let id = this.id.clone();
                        store.update(cx, |store, cx| store.reload_object(&id, cx));
                    }
                    this.sync(window, cx);
                },
            ),
            cx.subscribe(&title, |this, title, event: &InputEvent, cx| match event {
                InputEvent::Change => {
                    // Titles are single logical lines that wrap: pasted newlines collapse to spaces.
                    let value = title.read(cx).value().to_string();
                    if value.contains('\n') {
                        let fixed = value.replace('\n', " ");
                        this.pending_title_fix = Some(fixed);
                        cx.notify();
                    }
                }
                InputEvent::Focus => this.title_editing = true,
                InputEvent::Blur => {
                    this.title_editing = false;
                    this.save_name(cx);
                }
                _ => {}
            }),
            cx.subscribe_in(
                &Navigator::global(cx),
                window,
                |this, nav, event: &NavEvent, window, cx| {
                    if *event == NavEvent::Navigated
                        && *nav.read(cx).route() == Route::Object(this.id.clone())
                    {
                        this.close_sheet_on_phone(window, cx);
                        this.publish_pane(cx);
                    }
                },
            ),
        ];
        let mut this = Self {
            id,
            title,
            title_synced: None,
            title_editing: false,
            show_emoji: false,
            meta: None,
            body: None,
            pane: None,
            opened: false,
            title_read_only: false,
            _subscriptions: subscriptions,
            pending_title_fix: None,
        };
        if this.is_active(cx) {
            this.close_sheet_on_phone(window, cx);
        }
        this.sync(window, cx);
        this
    }

    /// On a phone the pane is a full-screen sheet over the object, so each
    /// object opens on its own page; the header's chat button opens the sheet.
    fn close_sheet_on_phone(&self, window: &Window, cx: &mut App) {
        if is_mobile(window) {
            PaneState::global(cx).update(cx, |pane, cx| pane.set_open(false, cx));
        }
    }

    fn object(&self, cx: &App) -> Option<Arc<Object>> {
        Store::global(cx).read(cx).object(&self.id)
    }

    fn is_active(&self, cx: &App) -> bool {
        *Navigator::global(cx).read(cx).route() == Route::Object(self.id.clone())
    }

    /// Bring the title, body and pane in line with the cached object state.
    fn sync(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(object) = self.object(cx) else {
            return;
        };
        if !self.opened {
            // Anytype's phone idiom: a chat object IS its discussion - open
            // the full-screen chat page in this one's place.
            if object.type_key == "chat" && is_mobile(window) && self.is_active(cx) {
                let chat = Route::Chat(self.id.clone());
                Navigator::global(cx).update(cx, |nav, cx| {
                    if nav.can_go_back() {
                        nav.back(cx);
                    }
                    nav.navigate(chat, cx);
                });
                return;
            }
            self.opened = true;
            roostr_core::recent::mark_opened(&self.id, cx);
        }
        // Title draft: refreshes must not clobber typing, saved or not yet.
        let name = object.name();
        if !self.title_editing && self.title_synced.as_deref() != Some(name) {
            self.title_synced = Some(name.to_string());
            if self.title.read(cx).value().as_ref() != name {
                self.title
                    .update(cx, |t, cx| t.set_value(name.to_string(), window, cx));
            }
        }
        let read_only = is_builtin_tool(&object.fields);
        if self.title_read_only != read_only {
            self.title_read_only = read_only;
            self.title.update(cx, |t, cx| t.set_readonly(read_only, cx));
        }
        if self.meta.is_none() {
            let id = self.id.clone();
            self.meta = Some(cx.new(|cx| ObjectMeta::new(id, window, cx)));
        }
        let kind = kind_of(&object);
        if self.body.as_ref().map(|(k, _)| *k) != Some(kind) {
            let id = self.id.clone();
            let body = match kind {
                Kind::Space => Body::Space(cx.new(|cx| SpaceManage::new(id, window, cx))),
                Kind::Chat => Body::Chat(cx.new(|cx| {
                    Discussion::new(
                        id,
                        DiscussionOptions {
                            full: true,
                            page_mode: false,
                            thread_id: "__discussion__".into(),
                            embedded: false,
                        },
                        window,
                        cx,
                    )
                })),
                Kind::File => Body::File(cx.new(|cx| FileView::new(id, window, cx))),
                Kind::Type => Body::Type(
                    cx.new(|cx| TypePanel::new(id.clone(), window, cx)),
                    cx.new(|cx| DataView::new(id, DataViewKind::Type, window, cx)),
                ),
                Kind::Relation => Body::Relation(cx.new(|cx| PropertyPanel::new(id, window, cx))),
                Kind::Query => {
                    Body::Data(cx.new(|cx| DataView::new(id, DataViewKind::Query, window, cx)))
                }
                Kind::Collection => {
                    Body::Data(cx.new(|cx| DataView::new(id, DataViewKind::Collection, window, cx)))
                }
                Kind::Document { read_only } => {
                    Body::Document(cx.new(|cx| Editor::new(id, read_only, window, cx)))
                }
            };
            self.body = Some((kind, body));
        }
        // The right pane: properties everywhere a property row makes sense, plus the chat.
        let (has_discussion, has_pane) = pane_flags(&object);
        match (&self.pane, has_pane) {
            (Some((chat, _)), true) if *chat == has_discussion => {}
            (_, true) => {
                let id = self.id.clone();
                self.pane = Some((
                    has_discussion,
                    cx.new(|cx| ConversationDrawer::new(id, has_discussion, window, cx)),
                ));
            }
            (_, false) => self.pane = None,
        }
        if self.is_active(cx) {
            self.publish_pane(cx);
        }
        cx.notify();
    }

    /// `discussionUI.available/count/convCount` + the pane view, for the shell.
    fn publish_pane(&mut self, cx: &mut Context<Self>) {
        let Some(object) = self.object(cx) else {
            PaneState::global(cx).update(cx, |p, cx| p.publish(false, 0, 0, cx));
            roostr_shell::set_pane(None, cx);
            return;
        };
        let (has_discussion, has_pane) = pane_flags(&object);
        let (count, conversations) = roostr_discussion::pane_counts(&object);
        let conversations = if has_discussion { conversations } else { 0 };
        PaneState::global(cx).update(cx, |p, cx| p.publish(has_pane, count, conversations, cx));
        roostr_shell::set_pane(self.pane.as_ref().map(|(_, view)| view.clone().into()), cx);
    }

    fn save_name(&mut self, cx: &mut Context<Self>) {
        let Some(object) = self.object(cx) else {
            return;
        };
        let draft = self.title.read(cx).value().replace('\n', " ");
        if draft == object.name() {
            return;
        }
        api::note::set_field(&object.id, "name", Value::string(draft), cx).detach_and_log_err(cx);
    }

    /// Emoji and image icons are mutually exclusive.
    fn set_icon(&mut self, value: &str, cx: &mut Context<Self>) {
        self.show_emoji = false;
        let image = is_image_icon(value);
        let (set, clear) = if image {
            ("iconImage", "iconEmoji")
        } else {
            ("iconEmoji", "iconImage")
        };
        let id = self.id.clone();
        let set_task = api::note::set_field(&id, set, Value::string(value), cx);
        cx.spawn(async move |_, cx| {
            if set_task.await.is_ok() {
                let clear_task = cx.update(|cx| api::note::delete_field(&id, clear, cx));
                let _ = clear_task.await;
            }
        })
        .detach();
        cx.notify();
    }

    fn toggle_done(&mut self, done: bool, cx: &mut Context<Self>) {
        api::note::set_field(&self.id, "done", Value::bool(!done), cx).detach_and_log_err(cx);
    }

    fn focus_body(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some((_, Body::Document(editor))) = &self.body {
            editor.update(cx, |editor, cx| editor.focus_start(window, cx));
        }
    }
}

/// (`hasDiscussion`, `hasPane`) from the object page's rules.
fn pane_flags(object: &Object) -> (bool, bool) {
    let t = object.type_key.as_str();
    let has_mailbox = object.mailbox.as_ref().is_some_and(|m| !m.is_empty());
    let has_discussion = has_mailbox || !matches!(t, "chat" | "type" | "template" | "relation");
    let has_pane = has_discussion || !matches!(t, "chat" | "type" | "relation");
    (has_discussion, has_pane)
}

/// `.tpl-note`: 12px muted, margin 0 0 8 48 (16 on a phone).
fn note(text: impl Into<SharedString>, mobile: bool) -> Div {
    div()
        .ml(px(if mobile { 16. } else { 48. }))
        .mb(px(8.))
        .text_size(px(12.))
        .text_color(theme::muted())
        .child(text.into())
}

impl Render for ObjectPage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if let Some(fixed) = self.pending_title_fix.take() {
            self.title
                .update(cx, |t, cx| t.set_value(fixed, window, cx));
        }
        let Some(object) = self.object(cx) else {
            let error = match Store::global(cx).read(cx).object_slot(&self.id) {
                Some(ObjectSlot::Failed(message)) if message.contains("Storage") => {
                    Some(format!("Cannot open this object: {message}"))
                }
                _ => None,
            };
            return div()
                .text_size(px(12.))
                .text_color(theme::muted())
                .child(error.unwrap_or_else(|| "Loading…".into()))
                .into_any_element();
        };
        let store = Store::global(cx).read(cx);
        let active = Navigator::global(cx).read(cx).space().to_string();
        let target_type = (object.type_key == "template")
            .then(|| {
                store
                    .types
                    .iter()
                    .find(|t| t.id == field_str(&object.fields, "target_type"))
                    .cloned()
            })
            .flatten();
        let effective = if object.type_key == "template" {
            target_type
                .as_ref()
                .map(|t| t.key.clone())
                .unwrap_or_default()
        } else {
            object.type_key.clone()
        };
        let task = store.layout_of(&effective) == "task";
        let done = object.field("done").and_then(|v| v.bool_value) == Some(true);
        let builtin_tool = is_builtin_tool(&object.fields);
        let locked_tool = is_locked_tool(&object.fields);
        let emoji = field_str(&object.fields, "iconEmoji").to_string();
        let image = field_str(&object.fields, "iconImage").to_string();
        let icon = object_icon(store, &emoji, &object.type_key, &active);
        // Phones drop the 48px rail: one 16px gutter for icon, title, notes
        // and body, a tighter rhythm, smaller icon and title (@media 720).
        let mobile = is_mobile(window);

        let mut article = div().flex().flex_col();
        if !task {
            let picker = self.show_emoji.then(|| {
                let page = cx.entity().downgrade();
                let close = cx.entity().downgrade();
                deferred(
                    anchored().snap_to_window_with_margin(px(8.)).child(
                        div().mt(px(6.)).child(
                            EmojiPicker::new("object-emoji")
                                .with_image(true)
                                .on_pick(move |value, _, cx| {
                                    let value = value.to_string();
                                    let _ = page.update(cx, |page, cx| page.set_icon(&value, cx));
                                })
                                .on_close(move |_, cx| {
                                    let _ = close.update(cx, |page, cx| {
                                        page.show_emoji = false;
                                        cx.notify();
                                    });
                                }),
                        ),
                    ),
                )
                .with_priority(5)
            });
            let button = div()
                .id("obj-emoji")
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(14.))
                .map(|d| {
                    if !image.is_empty() {
                        let size = px(if mobile { 72. } else { 96. });
                        d.size(size).child(
                            img(SharedString::from(image.clone()))
                                .size(size)
                                .rounded(px(18.))
                                .object_fit(ObjectFit::Cover),
                        )
                    } else if emoji.is_empty() {
                        d.size(px(if mobile { 56. } else { 80. }))
                            .text_size(px(32.))
                            .line_height(px(32.))
                            .text_color(theme::muted())
                            .opacity(0.5)
                            .child(icon)
                    } else if mobile {
                        d.size(px(56.))
                            .text_size(px(42.))
                            .line_height(px(42.))
                            .child(ui::emoji_or_image_icon(&icon, px(47.25)))
                    } else {
                        d.size(px(80.))
                            .text_size(px(64.))
                            .line_height(px(64.))
                            .child(ui::emoji_or_image_icon(&icon, px(72.)))
                    }
                })
                .when(!builtin_tool, |d| {
                    d.cursor_pointer()
                        .hover(|s| s.bg(theme::hover()))
                        .tooltip(ui::tooltip("Set icon"))
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.show_emoji = !this.show_emoji;
                            cx.notify();
                        }))
                });
            article = article.child(
                div()
                    .relative()
                    .map(|d| {
                        if mobile {
                            d.mt(px(8.)).mb(px(2.)).ml(px(16.))
                        } else {
                            d.mt(px(16.)).mb(px(4.)).ml(px(44.))
                        }
                    })
                    .flex()
                    .flex_col()
                    .child(button)
                    .children(picker),
            );
        }

        let title = div()
            .id("title")
            .debug_selector(|| "object-title".into())
            .flex_1()
            .min_w_0()
            .map(|d| {
                if mobile {
                    d.pt(px(4.)).pb(px(6.)).text_size(px(32.))
                } else {
                    d.pt(px(8.)).pb(px(16.)).text_size(px(34.))
                }
            })
            .font_weight(FontWeight(750.))
            .line_height(relative(1.25))
            .text_color(theme::fg())
            .capture_action(cx.listener(|this, _: &Enter, window, cx| {
                cx.stop_propagation();
                window.blur(cx);
                this.save_name(cx);
                this.focus_body(window, cx);
            }))
            .child(
                Textarea::new(&self.title)
                    .appearance(false)
                    .readonly(builtin_tool)
                    .text_size(px(if mobile { 32. } else { 34. }))
                    .font_weight(FontWeight(750.))
                    .line_height(relative(1.25))
                    .p_0(),
            );
        article = article.child(
            div()
                .flex()
                .items_start()
                .when(!task, |d| d.pl(px(if mobile { 16. } else { 48. })))
                .when(task, |d| {
                    d.child(
                        div()
                            .id("done-check")
                            .flex_none()
                            .size(px(20.))
                            .ml(px(14.))
                            .mr(px(6.))
                            .mt(px(if mobile { 14. } else { 19. }))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(4.))
                            .border_1()
                            .text_size(px(12.))
                            .text_color(theme::white())
                            .cursor_pointer()
                            .map(|d| {
                                if done {
                                    d.bg(theme::accent()).border_color(theme::accent())
                                } else {
                                    d.bg(theme::panel())
                                        .border_color(theme::border())
                                        .hover(|s| s.border_color(theme::accent()))
                                }
                            })
                            .when(done, |d| d.child("✓"))
                            .tooltip(ui::tooltip(if done { "Done" } else { "Mark done" }))
                            .on_click(
                                cx.listener(move |this, _, _, cx| this.toggle_done(done, cx)),
                            ),
                    )
                })
                .child(title),
        );
        if let Some(meta) = &self.meta {
            article = article.child(meta.clone());
        }
        if object.type_key == "template" {
            let of = target_type
                .map(|t| t.name)
                .filter(|n| !n.is_empty())
                .map(|n| format!(" of {n}"))
                .unwrap_or_default();
            article = article.child(note(format!(
                "Template{of} — new objects copy these blocks, and its properties fill in any the new object doesn't already have."
            ), mobile));
        }
        if locked_tool {
            article = article.child(note("Built-in tool, written by an older Roostr that runs it from its own code — this page doesn't change what runs, so it is read-only here. A computer with a current Roostr turns it into the shipped code, which you can then edit.", mobile));
        } else if builtin_tool {
            article = article.child(note("Built-in tool, shipped with Roostr — the code below is what runs, so editing it changes this tool for every agent in the space. If an edit breaks it, each computer keeps running the last version that worked there and says so under Error.", mobile));
            if object.field("tool_update_available").is_some() {
                article = article.child(note(
                    "Roostr now ships a different version of this tool. It was edited here, so your version was kept.",
                    mobile,
                ));
            }
        }
        let dataview = |view: AnyView| div().when(!mobile, |d| d.ml(px(48.))).child(view);
        if let Some((_, body)) = &self.body {
            article = match body {
                Body::Space(v) => article.child(v.clone()),
                Body::Chat(v) => article.child(v.clone()),
                Body::File(v) => article.child(v.clone()),
                Body::Type(panel, data) => article
                    .child(panel.clone())
                    .child(dataview(data.clone().into())),
                Body::Relation(v) => article.child(v.clone()),
                Body::Data(v) => article.child(dataview(v.clone().into())),
                Body::Document(v) => article.child(v.clone()),
            };
        }
        // The table's rubber band arms anywhere on the page (web selection provider).
        let data_view = match &self.body {
            Some((_, Body::Type(_, view))) | Some((_, Body::Data(view))) => Some(view.clone()),
            _ => None,
        };
        if let Some(view) = data_view {
            article = article.on_mouse_down(MouseButton::Left, move |e, window, cx| {
                view.update(cx, |v, cx| v.page_mouse_down(e, window, cx));
            });
        }
        article.into_any_element()
    }
}

/// `/app`: the space home — the Install-a-computer card, then a note.
pub struct HomePage {
    install: Entity<InstallComputer>,
}

impl HomePage {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        cx.observe(&Store::global(cx), |_, _, cx| cx.notify())
            .detach();
        Self {
            install: cx.new(|cx| InstallComputer::new(window, cx)),
        }
    }
}

impl Render for HomePage {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let loaded = Store::global(cx).read(cx).loaded;
        div().flex().flex_col().child(self.install.clone()).when(loaded, |d| {
            d.child(div().my(px(24.)).text_size(px(14.)).text_color(theme::muted()).child(
                "Nothing open in this space yet. Pick something in the sidebar, find anything with search, or make something new from the New menu.",
            ))
        })
    }
}

#[cfg(all(test, not(target_family = "wasm")))]
mod tests {
    // Explicit imports: the kit's glob would shadow `#[test]` with GPUI's.
    use std::collections::HashMap;
    use std::path::Path;
    use std::sync::Arc;

    use gpui::{Modifiers, TestAppContext, VisualTestContext};
    use gpui_kit::component::input::TextareaState;
    use gpui_kit::{
        AppContext as _, Context, Entity, FocusHandle, Focusable as _, InteractiveElement as _,
        IntoElement, ParentElement as _, Render, Styled as _, Window, div, px, size,
    };
    use parking_lot::Mutex;
    use roostr_core::io::{BoxFuture, BoxStream};
    use roostr_core::store::Store;
    use roostr_core::types::{
        Object, ObjectSummary, QueryResult, RelationDef, Settings, Space, Value,
    };
    use roostr_core::{Backend, BackendEvent, JsonMap};
    use serde_json::Value as Json;

    use super::ObjectPage;

    const PAGE: &str = "page-1";
    const QUERY: &str = "query-1";

    /// A vault of an untitled page and an untitled query that keeps every
    /// mutation it is sent.
    #[derive(Default)]
    struct Vault {
        /// Saved names by object id.
        names: Mutex<HashMap<String, String>>,
        mutations: Mutex<Vec<(String, JsonMap)>>,
    }

    fn ready<T: Send + 'static>(value: T) -> BoxFuture<T> {
        Box::pin(std::future::ready(value))
    }

    impl Backend for Vault {
        fn describe(&self) -> String {
            "a page and a query".into()
        }
        fn settings(&self) -> BoxFuture<anyhow::Result<Settings>> {
            ready(Ok(Settings {
                has_key: true,
                ..Settings::default()
            }))
        }
        fn objects(&self) -> BoxFuture<anyhow::Result<Vec<ObjectSummary>>> {
            ready(Ok(Vec::new()))
        }
        fn object(&self, id: &str) -> BoxFuture<anyhow::Result<Object>> {
            let type_key = match id {
                PAGE => "page",
                QUERY => "query",
                _ => return ready(Err(anyhow::anyhow!("unknown object {id}"))),
            };
            let name = self.names.lock().get(id).cloned().unwrap_or_default();
            ready(Ok(Object {
                id: id.into(),
                type_key: type_key.into(),
                fields: [("name".to_string(), Value::string(name))].into(),
                ..Object::default()
            }))
        }
        fn channels(&self) -> BoxFuture<anyhow::Result<Vec<Space>>> {
            ready(Ok(Vec::new()))
        }
        fn relations(&self) -> BoxFuture<anyhow::Result<Vec<RelationDef>>> {
            ready(Ok(Vec::new()))
        }
        fn query(&self, _: &Json) -> BoxFuture<anyhow::Result<QueryResult>> {
            ready(Ok(QueryResult::default()))
        }
        fn mutate(&self, action: &str, params: JsonMap) -> BoxFuture<anyhow::Result<JsonMap>> {
            if action == "set_field" && params["key"] == "name" {
                let id = params["object_id"].as_str().unwrap_or("");
                let name = params["value"]["stringValue"].as_str().unwrap_or("");
                self.names.lock().insert(id.to_string(), name.to_string());
            }
            self.mutations.lock().push((action.to_string(), params));
            ready(Ok(JsonMap::new()))
        }
        fn serving(&self, _: &[String]) -> BoxFuture<anyhow::Result<JsonMap>> {
            ready(Ok(JsonMap::new()))
        }
        fn sync_digest(&self) -> BoxFuture<anyhow::Result<JsonMap>> {
            ready(Ok(JsonMap::new()))
        }
        fn events(&self) -> BoxStream<anyhow::Result<BackendEvent>> {
            Box::pin(futures::stream::pending())
        }
    }

    /// The page with somewhere else to click (its body, another field).
    struct Harness {
        page: Entity<ObjectPage>,
        elsewhere: FocusHandle,
    }

    impl Render for Harness {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().size_full().child(self.page.clone()).child(
                div()
                    .id("elsewhere")
                    .track_focus(&self.elsewhere)
                    .h(px(40.)),
            )
        }
    }

    fn text(title: &Entity<TextareaState>, cx: &mut VisualTestContext) -> String {
        cx.update(|_, cx| title.read(cx).value().to_string())
    }

    fn title_of(harness: &Entity<Harness>, cx: &mut VisualTestContext) -> Entity<TextareaState> {
        cx.update(|_, cx| harness.read(cx).page.read(cx).title.clone())
    }

    fn focus_elsewhere(harness: &Entity<Harness>, cx: &mut VisualTestContext) {
        cx.update(|window, cx| {
            let elsewhere = harness.read(cx).elsewhere.clone();
            window.focus(&elsewhere, cx);
        });
    }

    /// Whether `object`'s name was ever saved as `name`.
    fn saved(backend: &Vault, object: &str, name: &str) -> bool {
        backend.mutations.lock().iter().any(|(action, params)| {
            action == "set_field"
                && params["object_id"] == object
                && params["key"] == "name"
                && params["value"]["stringValue"] == name
        })
    }

    /// Start the app on `backend`, its device-local UI state in `dir`, and
    /// open `id`'s page in an active window.
    fn open<'a>(
        id: &str,
        backend: &Arc<Vault>,
        dir: &Path,
        cx: &'a mut TestAppContext,
    ) -> (Entity<Harness>, &'a mut VisualTestContext) {
        // The environment is the process's: tests set it and start their
        // app (which reads it) one at a time.
        static START: Mutex<()> = Mutex::new(());
        {
            let _one_at_a_time = START.lock();
            // SAFETY: the only place these tests touch the environment, and
            // no other thread of theirs reads it while the lock is held.
            unsafe { std::env::set_var("ROOSTR_UI_STATE", dir.join("ui-state.json")) };
            cx.update(|cx| {
                gpui_kit::init(cx);
                roostr_core::init(backend.clone(), None, cx);
                crate::init(cx);
            });
        }
        let id = id.to_string();
        let (harness, cx) = cx.add_window_view(|window, cx| Harness {
            page: cx.new(|cx| ObjectPage::new(id, window, cx)),
            elsewhere: cx.focus_handle(),
        });
        // Focus events need the window active, as a window being typed in is.
        cx.update(|window, _| window.activate_window());
        cx.run_until_parked();
        (harness, cx)
    }

    /// A title typed into a new page and left at once is saved. Focus leaves
    /// the title a frame before the title hears its blur (which saves); a
    /// store refresh landing in between (a fresh vault's first sync brings
    /// many) must not put the stored, still empty, name back over the draft.
    #[gpui::test]
    fn title_left_mid_refresh_is_saved(cx: &mut TestAppContext) {
        let ui_state = tempfile::tempdir().unwrap();
        let backend = Arc::new(Vault::default());
        let (harness, cx) = open(PAGE, &backend, ui_state.path(), cx);
        let title = title_of(&harness, cx);

        cx.update(|window, cx| title.update(cx, |title, cx| title.focus(window, cx)));
        cx.run_until_parked();
        cx.simulate_input("E2E Page");
        assert_eq!(text(&title, cx), "E2E Page");

        cx.update(|window, cx| {
            let elsewhere = harness.read(cx).elsewhere.clone();
            window.focus(&elsewhere, cx);
            Store::global(cx).update(cx, |_, cx| cx.notify());
        });
        assert_eq!(text(&title, cx), "E2E Page", "the refresh took the draft");
        cx.run_until_parked();

        assert!(
            saved(&backend, PAGE, "E2E Page"),
            "the title was never saved: {:?}",
            backend.mutations.lock()
        );
        assert_eq!(text(&title, cx), "E2E Page");
    }

    /// A tap on a new query's title edits it, on a phone and on the desktop.
    /// A press anywhere on a query's page arms its table's rubber band, which
    /// focuses the view; a press the title took is the title's (the web's
    /// selection provider leaves inputs alone) and keeps the focus there.
    #[gpui::test]
    fn query_title_takes_a_tap(cx: &mut TestAppContext) {
        let ui_state = tempfile::tempdir().unwrap();
        let backend = Arc::new(Vault::default());
        let (harness, cx) = open(QUERY, &backend, ui_state.path(), cx);
        let title = title_of(&harness, cx);
        let title_focused = |cx: &mut VisualTestContext| {
            cx.update(|window, cx| title.read(cx).focus_handle(cx).is_focused(window))
        };

        // iPhone 13 Pro Max portrait: the mobile layout.
        cx.simulate_resize(size(px(428.), px(926.)));
        cx.run_until_parked();
        let at = cx.debug_bounds("object-title").expect("the title").center();
        cx.simulate_click(at, Modifiers::none());
        cx.run_until_parked();
        assert!(title_focused(cx), "the tap left the title unfocused");
        cx.simulate_input("Reading list");
        assert_eq!(text(&title, cx), "Reading list");
        focus_elsewhere(&harness, cx);
        cx.run_until_parked();
        assert!(
            saved(&backend, QUERY, "Reading list"),
            "the title was never saved: {:?}",
            backend.mutations.lock()
        );

        cx.simulate_resize(size(px(1280.), px(800.)));
        cx.run_until_parked();
        let at = cx.debug_bounds("object-title").expect("the title").center();
        cx.simulate_click(at, Modifiers::none());
        cx.run_until_parked();
        assert!(title_focused(cx), "the click left the title unfocused");
    }
}
