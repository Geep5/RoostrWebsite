//! Finder: a chat that gets what you say to the right page and agent of the space you're in.
//! The app you type in answers it (`finder_answer.rs`): every device runs its own Finder.
//!
//! Desktop: a floating 🧭 bubble, bottom right, opens a chat panel above it. Phone: the 🧭
//! button by Search opens the same chat as a sheet from the bottom. Each opening starts a new,
//! private session - a `finder_session` object outside every space, so only your own devices
//! hold it - for the space you're in. Finder's choices show as buttons; when it hands a
//! request off, the page it went to opens.

use roostr_core::nav::{Route, navigate};
use roostr_core::relations::current_space_id;
use roostr_core::types::{Fields, Value};
use roostr_core::{State, api};
use roostr_discussion::DiscussionOptions;
use roostr_shell::FinderOpen;
use roostr_ui::ty::{self, Weights as _};
use roostr_ui::{Theme, motion};
use rune::{Color, Cursor, Cx, El, Shadow, col, row, text};

use crate::finder_answer::{self, Choice};

/// The session open now, and what of it this window has acted on.
#[derive(Default)]
struct Finder {
    /// The session shown, with the space it was made for.
    session: Option<(String, String)>,
    /// Asked the store for a new session and waiting.
    making: bool,
    /// The last hand-off (its JSON) this window opened.
    opened: String,
    /// Whether the panel was open when last drawn: a fresh opening makes a fresh session.
    was_open: bool,
    /// The buttons (their JSON) a tap already answered: hidden until Finder offers new ones.
    tapped: String,
    /// The person's message answered last ("session:ts"): each is answered once.
    answered: String,
    /// Finder is working on an answer.
    busy: bool,
    /// Old sessions were cleared this run.
    swept: bool,
}

const SESSION_TYPE: &str = roostr_core::finder::SESSION_TYPE;
/// Sessions older than this are removed when Finder first opens.
const SESSION_TTL_MS: i64 = 7 * 86_400_000;
const PANEL_W: f32 = 380.;
const PANEL_H: f32 = 540.;

/// A new session for `space`; this app answers it.
fn start(s: &mut State, space: String) {
    let f = s.slot::<Finder>();
    if f.making {
        return;
    }
    f.making = true;
    f.session = None;
    if !std::mem::replace(&mut f.swept, true) {
        sweep(s);
    }
    let fields: Fields = [("finder_space".to_string(), Value::string(&space))].into();
    s.rt.spawn(async move |cx| {
        let made = cx
            .update(|s| api::note::create("Finder", Some(SESSION_TYPE), Some(fields), s))
            .await;
        cx.update(move |s| {
            let f = s.slot::<Finder>();
            f.making = false;
            match made {
                Ok(id) => {
                    f.session = Some((id.clone(), space));
                    s.store.ensure_object(&id);
                }
                Err(error) => log::warn!("[finder] new session failed: {error:#}"),
            }
        });
    })
    .detach();
}

/// Remove this person's Finder sessions older than a week.
fn sweep(s: &mut State) {
    let rows = api::query_all(serde_json::json!({ "type": SESSION_TYPE }), s);
    s.rt.spawn(async move |cx| {
        let Ok(rows) = rows.await else { return };
        let cutoff = roostr_core::finder::now_ms() - SESSION_TTL_MS;
        for r in rows.into_iter().filter(|r| r.updated_at < cutoff) {
            let params = [("object_id".to_string(), serde_json::json!(r.id))]
                .into_iter()
                .collect();
            let _ = cx.update(|s| api::mutate("delete", params, s)).await;
        }
    })
    .detach();
}

/// Run [`follow`] after every poll.
pub(crate) fn register(s: &mut State) {
    s.on_poll(follow);
}

/// Keep the session in step with the panel: a fresh one each time it opens (or the space
/// changes), and the page of a new hand-off opened. True when it changed what shows.
fn follow(s: &mut State) -> bool {
    let open = s.get::<FinderOpen>().is_some_and(|o| o.0);
    let space = current_space_id(s);
    let f = s.slot::<Finder>();
    let fresh = open && !f.was_open;
    f.was_open = open;
    if !open || space.is_empty() {
        return false;
    }
    let stale = !f.making && f.session.as_ref().is_none_or(|(_, sp)| *sp != space);
    if fresh || stale {
        start(s, space);
        return true;
    }
    let Some((id, space)) = s.get::<Finder>().and_then(|f| f.session.clone()) else {
        return false;
    };
    let answering = answer_newest(s, &id, &space);
    let handoff = s
        .store
        .object(&id)
        .map(|o| o.field_str("finder_handoff").to_string())
        .unwrap_or_default();
    if handoff.is_empty() || s.get::<Finder>().is_some_and(|f| f.opened == handoff) {
        return answering;
    }
    s.slot::<Finder>().opened = handoff.clone();
    if let Some(page) = serde_json::from_str::<serde_json::Value>(&handoff)
        .ok()
        .and_then(|v| v["page"].as_str().map(str::to_string))
    {
        navigate(Route::Object(page), s);
    }
    true
}

/// The person's newest message in the session, unanswered: Finder answers it here. True when
/// an answer started.
fn answer_newest(s: &mut State, id: &str, space: &str) -> bool {
    let Some(session) = s.store.object(id) else {
        return false;
    };
    let finder = roostr_core::finder::finder_id(space);
    let Some(newest) = finder_answer::lines(&session).pop() else {
        return false;
    };
    let key = format!("{id}:{}", newest.ts);
    let f = s.slot::<Finder>();
    if newest.author == finder || f.busy || f.answered == key {
        return false;
    }
    f.answered = key;
    f.busy = true;
    let (id, space) = (id.to_string(), space.to_string());
    s.rt.spawn(async move |cx| {
        finder_answer::answer(cx.clone(), id, space).await;
        cx.update(|s| s.slot::<Finder>().busy = false);
    })
    .detach();
    true
}

/// Finder's buttons, drawn between its messages and the composer: page cards (icon, name, type)
/// and the confirm's buttons. A tap posts the button's name once; they hide until Finder
/// offers new ones.
fn choices(s: &State, t: &Theme, id: &str) -> El {
    let raw = s
        .store
        .object(id)
        .map(|o| o.field_str("finder_options").to_string())
        .unwrap_or_default();
    if s.get::<Finder>().is_some_and(|f| f.busy) {
        return col()
            .key("finder-thinking")
            .px(14.)
            .pt(4.)
            .pb(8.)
            .child(text("🧭 Finding…").font(ty::ui(13.)).color(t.muted));
    }
    let list: Vec<Choice> = serde_json::from_str(&raw).unwrap_or_default();
    if list.is_empty() || s.get::<Finder>().is_some_and(|f| f.tapped == raw) {
        return col();
    }
    let mut out = col().key("finder-choices").gap(6.).px(14.).pt(4.).pb(8.);
    for c in list {
        let (sid, label, answered) = (id.to_string(), c.name.clone(), raw.clone());
        let press = move |s: &mut State| {
            s.slot::<Finder>().tapped = answered.clone();
            api::chat::post(&sid, &label, "", "", &[], s).detach_and_log_err(&s.rt);
        };
        let button = motion::pressable(row())
            .key(format!("finder-choice-{}", c.id))
            .role("button")
            .label(c.name.clone())
            .items_center()
            .gap(8.)
            .radius(10.)
            .cursor(Cursor::Pointer)
            .hover(t.hover)
            .on_press(press);
        let el = match c.kind.as_str() {
            "page" | "new" | "open" => {
                let icon = if c.icon.is_empty() {
                    if c.kind == "new" { "＋" } else { "📄" }.to_string()
                } else {
                    c.icon.clone()
                };
                let kind = match c.type_key.as_str() {
                    "" => String::new(),
                    "query" => "Query".into(),
                    "collection" => "Collection".into(),
                    k => {
                        let mut k = k.replace('_', " ");
                        if let Some(f) = k.get_mut(0..1) {
                            f.make_ascii_uppercase();
                        }
                        k
                    }
                };
                button
                    .w_full()
                    .px(12.)
                    .py(9.)
                    .fill(t.panel)
                    .border(1., t.border)
                    .border_layout()
                    .child(text(icon).font(ty::ui(16.)))
                    .child(
                        text(c.name)
                            .font(ty::ui(14.).semibold())
                            .color(t.fg)
                            .truncate()
                            .grow(1.)
                            .min_w(0.),
                    )
                    .child(text(kind).font(ty::ui(12.)).color(t.muted))
            }
            "do" => button.self_start().px(16.).py(8.).fill(t.accent).child(
                text(c.name)
                    .font(ty::ui(14.).semibold())
                    .color(Color::BLACK),
            ),
            _ => button
                .self_start()
                .px(14.)
                .py(8.)
                .fill(t.panel)
                .border(1., t.border)
                .border_layout()
                .child(if c.icon.is_empty() {
                    text("")
                } else {
                    text(c.icon).font(ty::ui(14.))
                })
                .child(text(c.name).font(ty::ui(14.)).color(t.fg)),
        };
        out = out.child(el);
    }
    out
}

/// While open, the Finder chat: on the desktop beside its 🧭 tile in the rail (roostr-shell
/// draws the tile), its bottom level with the tile's; on the phone a sheet from the bottom.
pub(crate) fn view(s: &State, cx: &mut Cx<State>, t: &Theme, layer: &mut Vec<El>) {
    let [w, h] = crate::object_page::window_size(s);
    if w <= 0. {
        return;
    }
    let mobile = roostr_shell::is_mobile(s);
    let open = s.get::<FinderOpen>().is_some_and(|o| o.0);
    if !open {
        return;
    }
    let (px, py, pw, ph) = if mobile {
        let ph = (h * 0.72).min(h - 40.);
        (0., h - ph, w, ph)
    } else {
        let ph = PANEL_H.min(h - 24.);
        // The tile's box [x, y, w, h]; before the rail is laid out, the bottom-left corner.
        let tile = cx
            .layout_with("rail-finder", |r| r)
            .unwrap_or([12., h - 56., 44., 44.]);
        let x = (tile[0] + tile[2] + 10.).min(w - PANEL_W - 12.).max(12.);
        let y = (tile[1] + tile[3] - ph).max(12.);
        (x, y, PANEL_W, ph)
    };
    let space = s
        .store
        .space(&current_space_id(s))
        .map(|sp| sp.name.clone())
        .unwrap_or_default();
    let header = row()
        .key("finder-head")
        .items_center()
        .gap(8.)
        .px(14.)
        .py(10.)
        .border_sides([0., 0., 1., 0.], t.border)
        .border_layout()
        .child(text("🧭").font(ty::ui(16.)))
        .child(
            text(if space.is_empty() {
                "Finder".to_string()
            } else {
                format!("Finder · {space}")
            })
            .font(ty::ui(14.).semibold())
            .color(t.fg)
            .truncate()
            .grow(1.)
            .min_w(0.),
        )
        .child(
            motion::pressable(row())
                .key("finder-new")
                .role("button")
                .label("New Finder chat")
                .px(8.)
                .py(4.)
                .radius(6.)
                .hover(t.hover)
                .cursor(Cursor::Pointer)
                .child(text("New").font(ty::ui(13.)).color(t.muted))
                .on_press(|s: &mut State| start(s, current_space_id(s))),
        )
        .child(
            motion::pressable(row())
                .key("finder-close")
                .role("button")
                .label("Close Finder")
                .size(28., 28.)
                .radius(14.)
                .center()
                .hover(t.hover)
                .cursor(Cursor::Pointer)
                .child(text("✕").font(ty::ui(14.)).color(t.muted))
                .on_press(roostr_shell::toggle_finder),
        );
    let mut body = col().key("finder-body").grow(1.).min_h(0.);
    match s.get::<Finder>().and_then(|f| f.session.clone()) {
        Some((id, _)) => {
            let opts = DiscussionOptions {
                full: true,
                embedded: true,
                above_composer: Some(choices),
                ..DiscussionOptions::default()
            };
            body = body.child(
                col()
                    .key(format!("finder-chat-{id}"))
                    .grow(1.)
                    .min_h(0.)
                    .child(roostr_discussion::discussion(
                        s,
                        cx,
                        t,
                        layer,
                        &id,
                        &opts,
                        [pw, ph],
                    )),
            );
        }
        None => {
            body = body.child(
                text("Starting…")
                    .font(ty::ui(13.))
                    .color(t.muted)
                    .px(14.)
                    .py(12.),
            )
        }
    }
    layer.push(
        col()
            .key("finder-panel")
            .absolute(px, py)
            .size(pw, ph)
            .fill(t.bg)
            .border(1., t.border)
            .border_layout()
            .radius(if mobile { 14. } else { 12. })
            .clip_rounded(if mobile { 14. } else { 12. })
            .box_shadow(Shadow {
                x: 0.,
                y: 8.,
                blur: 32.,
                spread: 0.,
                color: Color::BLACK.alpha(0.3),
                inset: false,
            })
            .child(header)
            .child(body),
    );
}
