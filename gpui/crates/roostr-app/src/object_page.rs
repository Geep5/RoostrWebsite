//! The object page (`routes/app/object/[id]/+page.svelte`) and the space
//! home (`routes/app/+page.svelte`): icon, title, featured properties, notes,
//! the body for the object's kind, and the right "Chat & properties" pane.
//!
//! A page's own state (its title draft, icon picker, cover) lives in the
//! [`Pages`] slot keyed by object id from when the shell first shows it
//! ([`shown`]) until no tab shows it ([`closed`]). Each body keeps its state
//! in its own crate's slot; the page opens and drops it with the page.

mod cover;

use std::collections::HashMap;
use std::sync::Arc;

use roostr_core::nav::Route;
use roostr_core::objects::{is_image_icon, object_icon};
use roostr_core::types::{Object, Value, field_str, is_builtin_tool, is_locked_tool};
use roostr_core::{ObjectSlot, State, StoreEvent, api, cover::CoverAsk};
use roostr_discussion::DiscussionOptions;
use roostr_sets::DataViewKind;
use roostr_ui::overlay::anchored;
use roostr_ui::{
    Button, Glyph, Images, KitSize, Placement, Textarea, TextareaState, Theme, glyph, ty,
};
use rune::{Cx, Drag, El, Tx, col, el, row, text};

/// What the page body shows (decided by the object's type).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Kind {
    Space,
    Chat,
    File,
    Type,
    Relation,
    Query,
    Collection,
    Document { read_only: bool },
}

pub(crate) fn kind_of(object: &Object) -> Kind {
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

/// (`hasDiscussion`, `hasPane`) from the object page's rules. Every page
/// but a chat object (whose page IS its discussion) has the pane with both
/// Chat and Properties - types, properties and templates included.
pub(crate) fn pane_flags(object: &Object) -> (bool, bool) {
    let t = object.type_key.as_str();
    let has_mailbox = object.mailbox.as_ref().is_some_and(|m| !m.is_empty());
    let has_discussion = has_mailbox || t != "chat";
    let has_pane = has_discussion || t != "chat";
    (has_discussion, has_pane)
}

/// Every object page the shell shows, by object id.
#[derive(Default)]
pub struct Pages(HashMap<String, Page>);

/// One object page's own state.
pub(crate) struct Page {
    title: TextareaState,
    /// The stored name the title last took: a refresh brings in a name only
    /// when it changed, never re-imposing one over a draft.
    title_synced: Option<String>,
    /// Typed since the last save: saved once the title loses focus.
    title_dirty: bool,
    show_emoji: bool,
    /// The body whose state was opened, by kind.
    body: Option<Kind>,
    opened: bool,
    cover: cover::CoverUi,
    /// The object state the page last synced with: a reload that emits no
    /// store event (a mutation's refetch) is caught by [`follow`].
    synced: Option<Arc<Object>>,
}

impl Page {
    fn new() -> Self {
        Self {
            title: TextareaState::new().auto_grow(1, 100).submit_on_enter(true),
            title_synced: None,
            title_dirty: false,
            show_emoji: false,
            body: None,
            opened: false,
            cover: cover::CoverUi::default(),
            synced: None,
        }
    }
}

pub(crate) fn page<'a>(s: &'a State, id: &str) -> Option<&'a Page> {
    s.get::<Pages>()?.0.get(id)
}

pub(crate) fn page_mut<'a>(s: &'a mut State, id: &str) -> Option<&'a mut Page> {
    s.slot::<Pages>().0.get_mut(id)
}

/// The listeners object pages follow. Call once.
pub fn register(s: &mut State) {
    s.on_store(on_store);
    s.on_cover_ask(on_cover_ask);
    s.on_poll(follow);
}

fn ids(s: &mut State) -> Vec<String> {
    s.slot::<Pages>().0.keys().cloned().collect()
}

fn on_store(s: &mut State, event: &StoreEvent) {
    for id in ids(s) {
        // A load that failed (not synced yet) retries when the replica learns more.
        let failed = matches!(s.store.object_slot(&id), Some(ObjectSlot::Failed(_)));
        let retry = match event {
            StoreEvent::Committed(committed) => *committed == id,
            StoreEvent::Refreshed | StoreEvent::Reconnected => true,
        };
        if failed && retry {
            s.store.reload_object(&id);
        }
        sync(s, &id);
    }
}

fn on_cover_ask(s: &mut State, ask: &CoverAsk) {
    cover::on_ask(s, &ask.object_id);
}

/// Each poll: pages whose object changed without a store event (a
/// mutation refetches its object quietly) sync, and a title typed into and
/// left is saved (Rune has no blur event).
fn follow(s: &mut State) -> bool {
    let changed: Vec<String> = s
        .get::<Pages>()
        .map(|pages| {
            pages
                .0
                .iter()
                .filter(|(id, p)| match (s.store.object(id), &p.synced) {
                    (Some(now), Some(then)) => !Arc::ptr_eq(&now, then),
                    (Some(_), None) => true,
                    (None, _) => false,
                })
                .map(|(id, _)| id.clone())
                .collect()
        })
        .unwrap_or_default();
    for id in &changed {
        sync(s, id);
    }
    let left: Vec<String> = s
        .slot::<Pages>()
        .0
        .iter()
        .filter(|(_, p)| p.title_dirty && !p.title.is_focused())
        .map(|(id, _)| id.clone())
        .collect();
    for id in &left {
        save_name(s, id);
    }
    !changed.is_empty() || !left.is_empty()
}

/// The shell shows `id`'s page: its state exists from now on, the object
/// loads, and the pane is published.
pub(crate) fn shown(s: &mut State, id: &str) {
    if page(s, id).is_none() {
        s.slot::<Pages>().0.insert(id.to_string(), Page::new());
        s.store.ensure_object(id);
    }
    close_sheet_on_phone(s);
    sync(s, id);
    publish_pane(s, id);
}

/// No tab shows `id`'s page any more: drop its state and its body's.
pub(crate) fn closed(s: &mut State, id: &str) {
    let Some(page) = s.slot::<Pages>().0.remove(id) else {
        return;
    };
    if page.title_dirty {
        save_draft(s, id, page.title.value());
    }
    if let Some(kind) = page.body {
        close_body(s, id, kind);
    }
    roostr_properties::forget(s, id);
    roostr_discussion::forget(s, id);
}

/// On a phone the pane is a full-screen sheet over the object, so each
/// object opens on its own page; the header's chat button opens the sheet.
fn close_sheet_on_phone(s: &mut State) {
    if roostr_shell::is_mobile(s) {
        s.pane.set_sheet(false);
    }
}

fn is_active(s: &State, id: &str) -> bool {
    matches!(s.nav.route(), Route::Object(open) if open == id)
}

fn open_body(s: &mut State, id: &str, kind: Kind) {
    match kind {
        Kind::Space => roostr_settings::open_space_manage(s, id),
        // The discussion opens itself when first drawn.
        Kind::Chat => {}
        Kind::File => roostr_editor::open_file(s, id),
        Kind::Type => {
            roostr_properties::open_type_panel(s, id);
            roostr_sets::open(s, id, DataViewKind::Type);
        }
        Kind::Relation => roostr_properties::open_property_panel(s, id),
        Kind::Query => roostr_sets::open(s, id, DataViewKind::Query),
        Kind::Collection => roostr_sets::open(s, id, DataViewKind::Collection),
        Kind::Document { read_only } => roostr_editor::open(s, id, read_only),
    }
}

fn close_body(s: &mut State, id: &str, kind: Kind) {
    match kind {
        Kind::Space | Kind::Chat | Kind::Relation => {}
        Kind::File => roostr_editor::close_file(s, id),
        Kind::Type | Kind::Query | Kind::Collection => roostr_sets::close(s, id),
        Kind::Document { .. } => roostr_editor::close(s, id),
    }
}

/// Bring the title, body and pane in line with the cached object state.
fn sync(s: &mut State, id: &str) {
    let Some(object) = s.store.object(id) else {
        return;
    };
    let Some(p) = page_mut(s, id) else {
        return;
    };
    p.synced = Some(object.clone());
    if !p.opened {
        // Anytype's phone idiom: a chat object IS its discussion - open
        // the full-screen chat page in this one's place.
        if object.type_key == "chat" && roostr_shell::is_mobile(s) && is_active(s, id) {
            if s.nav.can_go_back() {
                s.nav.back(&mut s.ui);
            }
            roostr_core::nav::navigate(Route::Chat(id.to_string()), s);
            return;
        }
        page_mut(s, id).expect("checked above").opened = true;
        roostr_core::recent::mark_opened(id, s);
    }
    let p = page_mut(s, id).expect("checked above");
    // Title draft: refreshes must not clobber typing, saved or not yet.
    let name = object.name();
    if !p.title.is_focused() && !p.title_dirty && p.title_synced.as_deref() != Some(name) {
        p.title_synced = Some(name.to_string());
        if p.title.value() != name {
            p.title.set_value(name);
        }
    }
    let kind = kind_of(&object);
    let previous = p.body.replace(kind);
    if previous != Some(kind) {
        if let Some(previous) = previous {
            close_body(s, id, previous);
        }
        open_body(s, id, kind);
    }
    cover::sync(s, id, &object);
    if is_active(s, id) {
        publish_pane(s, id);
    }
}

/// `discussionUI.available/count/convCount` for the shell's pane.
fn publish_pane(s: &mut State, id: &str) {
    let Some(object) = s.store.object(id) else {
        s.pane.publish(false, false, 0, 0);
        return;
    };
    let (has_discussion, has_pane) = pane_flags(&object);
    let (count, conversations) = roostr_discussion::pane_counts(&object);
    let conversations = if has_discussion { conversations } else { 0 };
    s.pane
        .publish(has_pane, has_discussion, count, conversations);
}

/// The pane body the shell asks for: one desktop column or the phone sheet.
pub(crate) fn pane(
    s: &State,
    cx: &mut Cx<State>,
    t: &'static Theme,
    layer: &mut Vec<El>,
    id: &str,
    part: roostr_shell::PanePart,
) -> El {
    let window = window_size(s);
    match part {
        roostr_shell::PanePart::Column(tab) => {
            roostr_discussion::pane_column(s, cx, t, layer, id, s.pane.chat, tab, window)
        }
        roostr_shell::PanePart::Sheet => {
            roostr_discussion::pane_sheet(s, cx, t, layer, id, s.pane.chat, window)
        }
    }
}

/// The window's logical size, as the shell's root last laid out.
pub(crate) fn window_size(s: &State) -> [f32; 2] {
    [roostr_shell::window_width(s), roostr_shell::window_height(s)]
}

fn save_name(s: &mut State, id: &str) {
    let Some(p) = page_mut(s, id) else {
        return;
    };
    p.title_dirty = false;
    let draft = p.title.value().to_string();
    save_draft(s, id, &draft);
}

fn save_draft(s: &mut State, id: &str, draft: &str) {
    let Some(object) = s.store.object(id) else {
        return;
    };
    let draft = draft.replace('\n', " ");
    if draft == object.name() {
        return;
    }
    api::note::set_field(id, "name", Value::string(draft), s).detach_and_log_err(&s.rt);
}

/// Titles are single logical lines that wrap: pasted newlines collapse to spaces.
fn title_changed(s: &mut State, id: &str) {
    if let Some(p) = page_mut(s, id) {
        p.title_dirty = true;
        if p.title.value().contains('\n') {
            let fixed = p.title.value().replace('\n', " ");
            let caret = fixed.len();
            p.title.set_value(fixed);
            p.title.set_caret(caret);
        }
    }
}

/// Enter in the title: leave it, save, and start typing the body.
fn title_entered(s: &mut State, id: &str, tx: &mut Tx) {
    tx.blur();
    save_name(s, id);
    if matches!(
        page(s, id).and_then(|p| p.body),
        Some(Kind::Document { .. })
    ) {
        roostr_editor::focus_start(s, id);
    }
}

/// Emoji and image icons are mutually exclusive.
fn set_icon(s: &mut State, id: &str, value: &str) {
    if let Some(p) = page_mut(s, id) {
        p.show_emoji = false;
    }
    let (set, clear) = if is_image_icon(value) {
        ("iconImage", "iconEmoji")
    } else {
        ("iconEmoji", "iconImage")
    };
    let set_task = api::note::set_field(id, set, Value::string(value), s);
    let id = id.to_string();
    s.rt.spawn(async move |cx| {
        if set_task.await.is_ok() {
            let clear_task = cx.update(|s| api::note::delete_field(&id, clear, s));
            let _ = clear_task.await;
        }
    })
    .detach();
}

fn toggle_done(s: &mut State, id: &str, done: bool) {
    api::note::set_field(id, "done", Value::bool(!done), s).detach_and_log_err(&s.rt);
}

/// `.tpl-note` as a quiet kit info alert: no fill or border, 12px muted,
/// margin 0 0 8 0 (16 on a phone).
fn note(key: &str, line: String, mobile: bool, t: &Theme) -> El {
    row()
        .key(key)
        .items_start()
        .gap(6.)
        .ml(if mobile { 16. } else { 0. })
        .mb(8.)
        .child(glyph(Glyph::Info, 12.).color(t.muted).mt(2.))
        .child(text(line).font(ty::ui(12.)).color(t.muted).wrap().shrink())
}

/// The page for `Route::Object(id)`.
pub(crate) fn view(
    s: &State,
    cx: &mut Cx<State>,
    t: &'static Theme,
    layer: &mut Vec<El>,
    id: &str,
) -> El {
    let (Some(object), Some(p)) = (s.store.object(id), page(s, id)) else {
        let error = match s.store.object_slot(id) {
            Some(ObjectSlot::Failed(message)) if message.contains("Storage") => {
                Some(format!("Cannot open this object: {message}"))
            }
            _ => None,
        };
        return text(error.unwrap_or_else(|| "Loading…".into()))
            .font(ty::ui(12.))
            .color(t.muted);
    };
    let store = &s.store;
    let active = s.nav.space().to_string();
    let target_type = (object.type_key == "template")
        .then(|| {
            store
                .types
                .iter()
                .find(|ty| ty.id == field_str(&object.fields, "target_type"))
                .cloned()
        })
        .flatten();
    let effective = if object.type_key == "template" {
        target_type
            .as_ref()
            .map(|ty| ty.key.clone())
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
    let mobile = roostr_shell::is_mobile(s);
    // On desktop every page runs flush with the header's back/forward
    // arrows: icon, title, badges and content start under them. A
    // document's block handles hang in the margin to the left instead
    // of taking a 48px rail inside the page.
    let flush = !mobile;
    let images = s.get::<Images>();

    let mut c = cover::Ctx {
        s,
        cx,
        t,
        id,
        object: &object,
        mobile,
    };
    // The cover spans the page's column, above the icon and title.
    let banner = cover::banner(&mut c);
    let covered = banner.is_some();
    let add_cover = cover::add_cover_button(&c);
    let cards = cover::cards(&mut c, layer);

    // Desktop: the header's 16px side padding, so the page's content
    // starts right under the back arrow.
    let mut article = col();
    if flush {
        article = article.px(16.);
    }
    // The icon and title: hovering them offers Add cover (desktop).
    let hover = id.to_string();
    let mut head =
        col()
            .key("page-head")
            .on_hover_change(move |s: &mut State, inside: bool, _: &mut Tx| {
                if let Some(p) = page_mut(s, &hover) {
                    p.cover.head_hovered = inside;
                }
            });
    if covered && task {
        head = head.mt(12.);
    }
    if !task {
        let content = {
            let frame = row().center();
            if !image.is_empty() {
                let size = if mobile { 72. } else { 96. };
                let mut frame = frame.size(size, size).radius(18.).clip_rounded(18.);
                if let Some(image) = images.and_then(|i| i.image(&image, size)) {
                    frame = frame.pixels_fit(image.pixels(), rune::SurfaceFit::Fill);
                }
                frame
            } else if emoji.is_empty() {
                let size = if mobile { 56. } else { 80. };
                frame
                    .size(size, size)
                    .opacity(0.5)
                    .child(text(icon.clone()).font(ty::ui(32.).lh(32.)).color(t.muted))
            } else {
                let (size, icon_size) = if mobile { (56., 47.25) } else { (80., 72.) };
                let shown = match images {
                    Some(images) => roostr_ui::emoji_or_image_icon(&icon, icon_size, images),
                    None => text(icon.clone()).font(ty::ui(icon_size * 0.9).lh(icon_size)),
                };
                frame.size(size, size).child(shown)
            }
        };
        // Built-in tools show their icon without any affordance; everything
        // else gets a kit ghost button that toggles the emoji picker. Its
        // box never shrinks: squeezed (a phone in landscape), the button's
        // clip cut the emoji in half.
        let button = if builtin_tool {
            el().key("obj-emoji").radius(14.).child(content)
        } else {
            let toggle = id.to_string();
            Button::new("obj-emoji")
                .ghost()
                .size(KitSize::Px(0.))
                .p(0.)
                .h_auto()
                .radius(14.)
                .child(content)
                .tooltip("Set icon")
                .on_press(move |s: &mut State| {
                    if let Some(p) = page_mut(s, &toggle) {
                        p.show_emoji = !p.show_emoji;
                    }
                })
                .build(t)
        };
        if p.show_emoji {
            let (pick, close) = (id.to_string(), id.to_string());
            let picker = roostr_properties::EmojiPicker::new("object-emoji")
                .with_image(true)
                .on_pick(move |s: &mut State, value: &str| set_icon(s, &pick, value))
                .on_close(move |s: &mut State| {
                    if let Some(p) = page_mut(s, &close) {
                        p.show_emoji = false;
                    }
                })
                .build(s, cx, t, layer);
            layer.push(anchored(picker, "object-icon", Placement::BottomStart, 6.));
        }
        // Under a cover the icon overlaps its bottom edge.
        let mut holder = col().key("object-icon").items_start().child(button);
        holder = if mobile {
            holder
                .mt(if covered {
                    -cover::ICON_OVERLAP_PHONE
                } else {
                    8.
                })
                .mb(2.)
                .ml(16.)
        } else {
            holder
                .mt(if covered { -cover::ICON_OVERLAP } else { 16. })
                .mb(4.)
                .ml(if flush { 0. } else { 44. })
        };
        head = head.child(holder);
    }

    let size = if mobile { 32. } else { 34. };
    let (edit, change, enter) = (id.to_string(), id.to_string(), id.to_string());
    let title = el()
        .key("title")
        .grow(1.)
        .min_w(0.)
        .pt(if mobile { 4. } else { 8. })
        .pb(if mobile { 6. } else { 16. })
        .child(
            Textarea::new("object-title", &p.title)
                .appearance(false)
                .readonly(builtin_tool)
                .placeholder("Untitled")
                .text_size(size)
                .weight(750.)
                .line_height(size * 1.25)
                .text_color(t.fg)
                .on_edit(move |s: &mut State| {
                    &mut page_mut(s, &edit).expect("a shown page's title").title
                })
                .on_change(move |s: &mut State, _: &mut Tx| title_changed(s, &change))
                .on_submit(move |s: &mut State, tx: &mut Tx| title_entered(s, &enter, tx))
                .build(t),
        );
    let mut title_row = row().items_start();
    if task {
        // Centred on the title's capitals (their middle is 0.87em below
        // the top of its line - below the line box's own centre), not on
        // the line box, which left it high.
        let pad = if mobile { 4. } else { 8. };
        let toggle = id.to_string();
        title_row = title_row.child(
            el().ml(15.).mr(7.).mt(pad + size * 0.87 - 8.).child(
                roostr_ui::checkbox("done-check", done)
                    .tooltip(if done { "Done" } else { "Mark done" })
                    .on_change(move |s: &mut State, _: bool| toggle_done(s, &toggle, done))
                    .build(t),
            ),
        );
    } else {
        title_row = title_row.pl(if mobile {
            16.
        } else if flush {
            0.
        } else {
            48.
        });
    }
    head = head.child(title_row.child(title));
    if let Some(add_cover) = add_cover {
        head = head.child(add_cover);
    }
    article = article
        .child(head)
        .child(cards)
        .child(roostr_properties::object_meta(s, cx, t, layer, id, flush));
    if object.type_key == "template" {
        let of = target_type
            .map(|ty| ty.name)
            .filter(|n| !n.is_empty())
            .map(|n| format!(" of {n}"))
            .unwrap_or_default();
        article = article.child(note(
            "tpl-note",
            format!(
                "Template{of} — new objects copy these blocks, and its properties fill in any the new object doesn't already have."
            ),
            mobile,
            t,
        ));
    }
    if locked_tool {
        article = article.child(note("tool-note", "Built-in tool, written by an older Roostr that runs it from its own code — this page doesn't change what runs, so it is read-only here. A computer with a current Roostr turns it into the shipped code, which you can then edit.".into(), mobile, t));
    } else if builtin_tool {
        article = article.child(note("tool-note", "Built-in tool, shipped with Roostr — the code below is what runs, so editing it changes this tool for every agent in the space. If an edit breaks it, each computer keeps running the last version that worked there and says so under Error.".into(), mobile, t));
        if object.field("tool_update_available").is_some() {
            article = article.child(note(
                "tool-update-note",
                "Roostr now ships a different version of this tool. It was edited here, so your version was kept.".into(),
                mobile,
                t,
            ));
        }
    }
    let kind = kind_of(&object);
    let body = match kind {
        Kind::Space => roostr_settings::space_manage(s, cx, t, layer, id),
        Kind::Chat => roostr_discussion::discussion(
            s,
            cx,
            t,
            layer,
            id,
            &DiscussionOptions {
                full: true,
                ..DiscussionOptions::default()
            },
            window_size(s),
        ),
        Kind::File => roostr_editor::file_view(s, cx, t, layer, id),
        Kind::Type => col()
            .child(roostr_properties::type_panel(s, cx, t, layer, id))
            .child(roostr_sets::view(s, cx, t, layer, id, DataViewKind::Type)),
        Kind::Relation => roostr_properties::property_panel(s, cx, t, layer, id),
        Kind::Query => roostr_sets::view(s, cx, t, layer, id, DataViewKind::Query),
        Kind::Collection => roostr_sets::view(s, cx, t, layer, id, DataViewKind::Collection),
        Kind::Document { .. } => {
            let editor = el().child(roostr_editor::view(s, cx, t, layer, id));
            if flush {
                editor.ml(-roostr_editor::GUTTER_W)
            } else {
                editor
            }
        }
    };
    article = article.child(body);
    let mut page = match banner {
        Some(banner) => col().child(banner).child(article),
        None => article,
    };
    // The table's rubber band arms anywhere on the page (web selection provider).
    if matches!(kind, Kind::Type | Kind::Query | Kind::Collection) {
        let drag = id.to_string();
        page = page.on_drag(move |s: &mut State, d: &mut Drag, tx: &mut Tx| {
            roostr_sets::page_drag(s, &drag, d, tx)
        });
    }
    page.key("object-page")
}

/// `/app`: the space home — the Install-a-computer card, then the space's
/// own settings (what its object page shows), never an empty note.
pub(crate) fn home(s: &State, cx: &mut Cx<State>, t: &'static Theme, layer: &mut Vec<El>) -> El {
    let space = s.nav.space();
    let known = !space.is_empty() && s.store.space(space).is_some();
    let mut home = col().child(roostr_settings::install_computer(s, cx, t));
    if known {
        home = home.child(roostr_settings::space_manage(s, cx, t, layer, space));
    }
    home
}

/// Escape nobody else took on an object page: the data view's menus and
/// selection first, then the pane.
pub(crate) fn on_escape(s: &mut State) -> bool {
    let Route::Object(id) = s.nav.route().clone() else {
        return false;
    };
    roostr_sets::dismiss(s, &id) || roostr_discussion::on_escape(s, &id, roostr_shell::is_mobile(s))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn object(type_key: &str) -> Object {
        Object {
            type_key: type_key.into(),
            ..Object::default()
        }
    }

    #[test]
    fn kinds_follow_the_type() {
        assert_eq!(kind_of(&object("channel")), Kind::Space);
        assert_eq!(kind_of(&object("set")), Kind::Query);
        assert_eq!(kind_of(&object("query")), Kind::Query);
        assert_eq!(
            kind_of(&object("page")),
            Kind::Document { read_only: false }
        );
    }

    #[test]
    fn only_a_chat_without_mailbox_has_no_pane() {
        assert_eq!(pane_flags(&object("page")), (true, true));
        assert_eq!(pane_flags(&object("chat")), (false, false));
        assert_eq!(pane_flags(&object("type")), (true, true));
    }
}
