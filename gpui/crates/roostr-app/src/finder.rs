//! Finder: a chat that gets what you say to the right page and agent of the space you're in
//! (glonOdin `harness/src/finder.ts` answers it).
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
}

const SESSION_TYPE: &str = "finder_session";
const PANEL_W: f32 = 380.;
const PANEL_H: f32 = 540.;
const BUBBLE: f32 = 52.;

/// A new session for `space`, served by the computer this app talks to (its harness answers
/// it as the space's Finder); a browser with no computer leaves it to the Finder's own.
fn start(s: &mut State, space: String) {
    let f = s.slot::<Finder>();
    if f.making {
        return;
    }
    f.making = true;
    f.session = None;
    let machine = roostr_core::create::this_machine_id(s);
    let session_space = space.clone();
    s.rt.spawn(async move |cx| {
        let machine = machine.await;
        let mut fields: Fields =
            [("finder_space".to_string(), Value::string(&session_space))].into();
        if !machine.is_empty() {
            fields.insert("served_by".into(), Value::string(machine));
        }
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
    let Some((id, _)) = s.get::<Finder>().and_then(|f| f.session.clone()) else {
        return false;
    };
    let handoff = s
        .store
        .object(&id)
        .map(|o| o.field_str("finder_handoff").to_string())
        .unwrap_or_default();
    if handoff.is_empty() || s.get::<Finder>().is_some_and(|f| f.opened == handoff) {
        return false;
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

/// A button Finder offered (glonOdin finder.ts `FinderOption`).
#[derive(serde::Deserialize)]
struct Choice {
    id: String,
    name: String,
    kind: String,
    #[serde(default, rename = "type")]
    type_key: String,
    #[serde(default)]
    icon: String,
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
            "page" | "new" => {
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

/// Draw the bubble (desktop) and, while open, the Finder chat.
pub(crate) fn view(s: &State, cx: &mut Cx<State>, t: &Theme, layer: &mut Vec<El>) {
    let [w, h] = crate::object_page::window_size(s);
    if w <= 0. {
        return;
    }
    let mobile = roostr_shell::is_mobile(s);
    let open = s.get::<FinderOpen>().is_some_and(|o| o.0);
    if !mobile {
        layer.push(
            motion::pressable(row())
                .key("finder-bubble")
                .role("button")
                .label("Finder")
                .absolute(w - BUBBLE - 20., h - BUBBLE - 20.)
                .size(BUBBLE, BUBBLE)
                .radius(BUBBLE / 2.)
                .center()
                .fill(t.panel)
                .border(1., t.border)
                .border_layout()
                .box_shadow(Shadow {
                    x: 0.,
                    y: 4.,
                    blur: 16.,
                    spread: 0.,
                    color: Color::BLACK.alpha(0.25),
                    inset: false,
                })
                .cursor(Cursor::Pointer)
                .child(
                    text(if open { "✕" } else { "🧭" })
                        .font(ty::ui(22.))
                        .color(t.fg),
                )
                .on_press(roostr_shell::toggle_finder),
        );
    }
    if !open {
        return;
    }
    let (px, py, pw, ph) = if mobile {
        let ph = (h * 0.72).min(h - 40.);
        (0., h - ph, w, ph)
    } else {
        let ph = PANEL_H.min(h - BUBBLE - 60.);
        (w - PANEL_W - 20., h - BUBBLE - 32. - ph, PANEL_W, ph)
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
