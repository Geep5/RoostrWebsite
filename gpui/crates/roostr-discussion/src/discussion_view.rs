//! Drawing a discussion instance ([`crate::discussion::Disc`]): the opener, the message
//! list with its bubbles, the footer row (empty note, an agent typing, its problem), notices,
//! the reply bar, the audience picker and the composer with its `@` / `!` menus.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};

use roostr_core::State;
use roostr_core::nav::{self, Route};
use roostr_core::objects::{is_image_icon, object_icon};
use roostr_ui::color::{Bubble, bubble};
use roostr_ui::layout::is_mobile;
use roostr_ui::tags::tint_paint;
use roostr_ui::ty::Weights as _;
use roostr_ui::{
    Alert, AlertVariant, Button, ButtonCustomVariant, Checkbox, DropdownMenu, Glyph, Images,
    KitSize, Placement, PopupMenu, PopupMenuItem, ScrollArea, ScrollbarMode, Spinner, Tag, Theme,
    glyph, motion, overlay, radius, rems, ty,
};
use rune::{
    Color, Cursor, Cx, Cycle, El, GrowthSpec, PillStyle, PointerPress, ScrollSpec, Side,
    SurfaceFit, Transition, Tx, col, el, memo, row, text, text_input,
};

use crate::chat::{ChatMessage, Reaction};
use crate::discussion::{
    AGENT_TOKEN, AgentPresence, Disc, DiscussionOptions, OBJECT_GREEN, OBJECT_TOKEN, Place, REVEAL,
    ScrollAsk, agent_tag_paint, composer_edit, get, instance_key, origin_label, request, with,
};
use crate::live::{Entry, LiveRuns, Run, Thought, clock, now_ms};
use crate::md::{MdEnv, MdTheme, render_markdown};
use crate::reactions::{BarAt, quick_emojis, touch_screen};
use crate::threads::ObjectAgentOption;

/// The instance of `object_id`'s discussion shown with `options`, asking for it when it
/// doesn't exist yet.
pub(crate) fn instance<'a>(
    s: &'a State,
    object_id: &str,
    options: &DiscussionOptions,
) -> Option<&'a Disc> {
    let key = instance_key(object_id, Place::of(options));
    let found = get(s, &key);
    if found.is_none() {
        request(s, &key, object_id, options);
    }
    found
}

/// An object's discussion (see the crate docs for the places it is shown).
pub fn discussion(
    s: &State,
    cx: &mut Cx<State>,
    t: &Theme,
    layer: &mut Vec<El>,
    object_id: &str,
    options: &DiscussionOptions,
    window: [f32; 2],
) -> El {
    match instance(s, object_id, options) {
        Some(d) => view(d, s, cx, t, layer, window),
        None => col(),
    }
}

/// How a round avatar is drawn in one place: message rows and the typing row, or the
/// mention menu.
#[derive(Clone, Copy)]
struct AvatarLook {
    size: f32,
    /// The initials' text size.
    letters: f32,
    /// An emoji's text size.
    emoji: f32,
    /// An emoji sits on a light disc (the mention menu shows it bare).
    disc: bool,
    /// Initials in the monospace face.
    mono: bool,
}

const ROW_AVATAR: AvatarLook = AvatarLook {
    size: 36.,
    letters: 12.,
    emoji: 21.,
    disc: true,
    mono: true,
};

const MENU_AVATAR: AvatarLook = AvatarLook {
    size: 20.,
    letters: 9.,
    emoji: 13.,
    disc: false,
    mono: false,
};

/// An avatar's corner: a squircle-like rounded square (40% of the side, the app-icon look).
fn avatar_radius(size: f32) -> f32 {
    size * 0.4
}

/// Stable avatar hue from the author id.
fn hue(author: &str) -> u32 {
    author
        .encode_utf16()
        .fold(0u32, |h, c| (h * 31 + c as u32) % 360)
}

fn avatar_color(author: &str) -> Color {
    roostr_ui::color::hsla(hue(author) as f32 / 360., 0.45, 0.35, 1.)
}

/// An author's name in their colour (their chat colour, else their avatar's hue), lightened
/// or darkened to read on the theme, so each group's start is easy to spot.
fn name_color(t: &Theme, base: Color) -> Color {
    let [h, s, _, _] = roostr_ui::color::to_hsla(base);
    let l = if t.dark { 0.74 } else { 0.36 };
    roostr_ui::color::hsla(h, s.max(0.45), l, 1.)
}

/// An author's name colour ([`name_color`]): their chat colour (yours on your own lines),
/// else their avatar's hue. Their links and their row's hover take it too.
fn author_ink(d: &Disc, store: &roostr_core::Store, t: &Theme, author: &str) -> Color {
    let chat = if author == d.me {
        store.chat_color.clone()
    } else {
        store
            .agent(author)
            .map(|a| a.chat_color.clone())
            .unwrap_or_default()
    };
    let base = if chat.is_empty() {
        avatar_color(author)
    } else {
        let colors: Bubble = bubble(&chat);
        colors.bg
    };
    name_color(t, base)
}

/// The key of message `key_id`'s row in instance `key`, which the reaction bar anchors to
/// (an anchor is found by its own key, so it carries the instance).
fn row_anchor(key: &str, key_id: &str) -> String {
    format!("{key}-row-{key_id}")
}

/// An avatar: an image icon (a web address or `data:image/` URL) is its picture; else the
/// emoji, else the initials on the author's stable hue.
fn avatar(
    t: &Theme,
    images: Option<&Images>,
    icon: &str,
    initials: String,
    hue_of: &str,
    look: AvatarLook,
) -> El {
    let r = avatar_radius(look.size);
    let frame = row().size(look.size, look.size).center().radius(r);
    if is_image_icon(icon) {
        let picture = images.and_then(|i| i.image(icon, look.size));
        return match picture {
            Some(image) => frame
                .fill(t.hl_light)
                .clip_rounded(r)
                .pixels_fit(image.pixels(), rune::SurfaceFit::Fill),
            None => frame,
        };
    }
    if icon.is_empty() {
        let font = if look.mono {
            ty::mono(look.letters)
        } else {
            ty::ui(look.letters)
        };
        return frame
            .fill(avatar_color(hue_of))
            .child(text(initials).font(font).color(t.white));
    }
    let frame = if look.disc {
        frame.fill(t.hl_light)
    } else {
        frame
    };
    frame.child(roostr_ui::emoji_text(icon, ty::ui(look.emoji).lh(look.size)).color(t.white))
}

/// `HH:MM AM`, with the date before it when not today.
fn when(ts: i64) -> String {
    let Some(d) = chrono::DateTime::from_timestamp_millis(ts) else {
        return String::new();
    };
    let d = d.with_timezone(&chrono::Local);
    let time = d.format("%I:%M %p").to_string();
    if d.date_naive() == chrono::Local::now().date_naive() {
        time
    } else {
        format!("{}, {time}", d.format("%b %-d"))
    }
}

/// Three pulsing dots (`.dots`): the agent composing. Each dot is faint and resting, lit at
/// 30% of its 1.2s cycle, a fifth of a second after the one before.
fn typing_dots(t: &Theme) -> El {
    const STOPS: &[(f32, f32)] = &[(0.0, 0.25), (0.3, 1.0), (0.6, 0.25), (1.0, 0.25)];
    row().gap(3.).center_y().children((0..3).map(|i| {
        el().size(5., 5.).radius(2.5).fill(t.muted).cycle(Cycle {
            period: 1.2,
            phase: i as f32 * (0.2 / 1.2),
            stops: STOPS,
            still: 0.25,
            fps: 30.0,
        })
    }))
}

/// The key path of an element inside instance `key`.
fn path(key: &str, el: &str) -> String {
    format!("{key}/{el}")
}

/// Runs `f` on instance `key` from a handler.
fn act(key: &str, f: impl Fn(&mut Disc, &mut State) + 'static) -> impl Fn(&mut State) + 'static {
    let key = key.to_string();
    move |s: &mut State| {
        with(s, &key, |d, s| f(d, s));
    }
}

/// What one message row reads from the store, hashed into its memo version with the
/// message's own fingerprint.
fn store_sig(s: &State) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    for a in &s.store.agents {
        (&a.id, &a.name, &a.icon, &a.chat_color, &a.channel).hash(&mut h);
    }
    s.store.chat_color.hash(&mut h);
    s.store.summaries.len().hash(&mut h);
    s.store
        .summaries
        .iter()
        .map(|o| o.updated_at)
        .max()
        .hash(&mut h);
    s.nav.space().hash(&mut h);
    h.finish()
}

/// The discussion instance `d`.
pub(crate) fn view(
    d: &Disc,
    s: &State,
    cx: &mut Cx<State>,
    t: &Theme,
    layer: &mut Vec<El>,
    window: [f32; 2],
) -> El {
    let key = d.key.as_str();
    let mobile = is_mobile(window[0]);
    let mut root = col().key(key.to_string());
    // Phones: no 48px rail margin (it skewed the centred opener), 16px sides.
    root = if d.embedded {
        root.grow(1.).min_h(0.).px(14.).pb(12.)
    } else if d.page_mode {
        root.grow(1.).min_h(0.).mt(8.)
    } else if d.full {
        root.mt(8.).grow(1.).min_h(window[1] * 0.6)
    } else if mobile {
        root.mt(32.)
    } else {
        root.mt(32.).ml(48.)
    };
    if mobile && !d.embedded {
        root = root.px(16.);
    }
    if d.object.is_none() {
        return root;
    }

    if !d.is_open() {
        return root.child(
            row()
                .justify_center()
                .pt(4.)
                .pb(12.)
                .child(opener(d, t, mobile)),
        );
    }

    if !d.full {
        // A short rule, then the 600-weight title.
        let title = if d.is_exchange {
            d.exchange_title.to_uppercase()
        } else {
            "DISCUSSION".into()
        };
        root = root
            .child(el().w_full().h(1.).fill(t.border).mb(16.))
            .child(
                row()
                    .center_y()
                    .justify_between()
                    .mb(8.)
                    .child(text(title).font(ty::ui(11.).lh(11. * 1.618)).color(t.muted))
                    .child(
                        Button::new("collapse")
                            .ghost()
                            .xsmall()
                            .text_color(t.muted)
                            .tooltip("Collapse")
                            .on_press(act(key, |d, s| {
                                d.open = false;
                                d.derive(s);
                            }))
                            .child(text("×").font(ty::ui(14.)).color(t.muted))
                            .build(t),
                    ),
            );
    }

    // Notices: warnings in orange, the rest muted.
    let notice = |id: &str, message: String, error: bool| {
        Alert::new(id.to_string(), message)
            .variant(if error {
                AlertVariant::Warning
            } else {
                AlertVariant::Default
            })
            .icon(if error {
                Glyph::TriangleAlert
            } else {
                Glyph::Info
            })
            .size(roostr_ui::KitSize::XSmall)
            .text_size(12.)
            .text_color(if error { t.orange } else { t.muted })
            .build(t)
            .my(12.)
    };
    if !d.identity_error.is_empty() {
        root = root
            .child(notice(
                "identity-error",
                format!("Cannot load your discussion identity: {}", d.identity_error),
                true,
            ))
            .child(
                Button::new("identity-retry")
                    .label("Retry discussion identity")
                    .outline()
                    .xsmall()
                    .on_press(act(key, |d, s| d.load_identity(s)))
                    .build(t),
            );
    }
    if !d.is_exchange && s.harness.is_some() && !d.presence_error.is_empty() {
        root = root.child(notice(
            "presence-error",
            format!(
                "{} Check that the paired native app and harness are running. Discussion remains available.",
                d.presence_error
            ),
            true,
        ));
    }
    if d.is_exchange && !d.members.is_empty() {
        let links = d.members.iter().enumerate().map(|(i, member)| {
            let target = member.object_id.clone();
            roostr_ui::Link::new(format!("member-{i}"))
                .label(d.member_name(member, &s.store))
                .text_size(11.)
                .color(t.fg)
                .plain()
                .on_press(move |s: &mut State| nav::navigate(Route::Object(target.clone()), s))
                .build(t)
        });
        root = root.child(
            row()
                .flex_wrap()
                .gaps(5., 8.)
                .py(8.)
                .child(
                    text("Participants")
                        .font(ty::ui(11.).lh(16.5))
                        .color(t.muted),
                )
                .children(links),
        );
    } else if d.is_exchange
        && let Some(conversation) = &d.conversation
        && !conversation.participants.is_empty()
    {
        let names = conversation
            .participants
            .iter()
            .map(|p| d.who(p, &s.store))
            .collect::<Vec<_>>()
            .join(", ");
        root = root.child(notice(
            "participants",
            format!("Participants: {names}"),
            false,
        ));
    }
    if d.legacy {
        root = root.child(notice(
            "legacy",
            "This shared exchange is read-only, awaiting migration to object inboxes.".into(),
            false,
        ));
    }
    if d.is_exchange && d.conversation.as_ref().is_some_and(|c| c.closed) {
        root = root.child(notice("closed", "This exchange is closed.".into(), false));
    }
    if !d.retry_error.is_empty() {
        root = root.child(notice(
            "retry-error",
            format!("Retry failed: {}", d.retry_error),
            true,
        ));
    }

    // Messages: a growth stack, so a new message eases its height open (a sent one in one
    // quick glide, an arriving reply at the streaming pace while its words fade in) and the
    // rows below or the pinned end follow. Rows replaced wholesale land at once: the stack
    // takes a new key with each `rows_epoch`.
    let rows = message_rows(d, s, t);
    let rows_key = format!("chat-rows-{}", d.rows_epoch);
    if d.own_scroller() {
        let vp = path(key, "chat-messages");
        let (k1, k2, k3) = (key.to_string(), key.to_string(), key.to_string());
        let vp_follow = vp.clone();
        let content = col()
            .key(rows_key)
            .w_full()
            .growth(GrowthSpec::default())
            .selection_group(true)
            .drawn_rect_into(&d.list_rect)
            .children(rows)
            .on_layout_tx(move |s: &mut State, _: [f32; 4], tx: &mut Tx| {
                with(s, &k1, |d, _| match d.scroll_ask.take() {
                    Some(ScrollAsk::Land) => {
                        tx.scroll_to_bottom_instant(vp_follow.clone());
                        d.tall_armed = false;
                    }
                    Some(ScrollAsk::Follow) => {
                        tx.scroll_to_bottom(vp_follow.clone());
                        d.tall_armed = true;
                    }
                    None => {}
                });
                tx.live_only();
            });
        root = root.child(
            col()
                .key("chat-messages")
                .grow(1.)
                .min_h(0.)
                .scroll(ScrollSpec { stick_bottom: true })
                .on_at_bottom(move |s: &mut State, at: bool| {
                    with(s, &k2, |d, _| d.at_end = at);
                })
                .on_layout_tx(move |s: &mut State, r: [f32; 4], tx: &mut Tx| {
                    with(s, &k3, |d, _| d.viewport_h = r[3]);
                    tx.live_only();
                })
                .child(content),
        );
    } else {
        // `.discussion.full .messages { flex: 1 }`: the composer sits at the bottom of the
        // 60vh-tall section, not right under the messages.
        let mut list = col()
            .key(rows_key)
            .growth(GrowthSpec::default())
            .selection_group(true)
            .drawn_rect_into(&d.list_rect)
            .children(rows);
        if d.full {
            list = list.grow(1.);
        }
        root = root.child(list);
    }

    if !d.read_only
        && !d.reply_to.is_empty()
        && let Some(target) = d.message(&d.reply_to)
    {
        let small = ty::ui(11.);
        root = root.child(
            row()
                .items_end()
                .gap(8.)
                .border_sides([0., 0., 0., 2.], t.accent)
                .px(8.)
                .py(2.)
                .mt(8.)
                .mb(4.)
                .child(
                    text(format!("Replying to {}", d.who(&target.author, &s.store)))
                        .font(small.semibold())
                        .color(t.muted),
                )
                .child(
                    text(target.text.chars().take(60).collect::<String>())
                        .font(small)
                        .color(t.muted)
                        .truncate()
                        .shrink(),
                )
                .child(el().grow(1.))
                .child(
                    Button::new("cancel-reply")
                        .ghost()
                        .xsmall()
                        .tooltip("Cancel reply")
                        .on_press(act(key, |d, _| {
                            d.reply_to.clear();
                            d.private_recipient.clear();
                        }))
                        .child(text("×").font(small).color(t.muted))
                        .build(t),
                ),
        );
    }
    if !d.send_error.is_empty() {
        root = root.child(notice("send-error", d.send_error.clone(), true));
    }
    if !d.read_only {
        if d.is_exchange {
            root = root.child(audience(d, s, t, layer));
        }
        root = root.child(composer(d, s, cx, t, layer));
    }
    react_bar(d, s, cx, t, layer, window);
    root
}

/// The collapsed discussion's "N comments" pill (a round chat button on a phone).
fn opener(d: &Disc, t: &Theme, mobile: bool) -> El {
    let count = d.messages.len();
    let label = if count > 0 {
        format!("{count} comment{}", if count == 1 { "" } else { "s" })
    } else {
        "Start a discussion".into()
    };
    let fill = Color::WHITE.alpha(0x0f as f32 / 255.0);
    let button = Button::new("opener")
        .custom(
            ButtonCustomVariant::new(t)
                .color(fill)
                .foreground(t.muted)
                .hover(t.hover)
                .active(t.hover),
        )
        .xsmall()
        .bg(fill)
        .weight(ty::MEDIUM);
    let button = if mobile {
        // A centred round chat button.
        button
            .radius(24.)
            .square(48.)
            .p(0.)
            .child(text("💬").font(ty::ui(20.)))
    } else {
        button
            .radius(18.)
            .h_auto()
            .pl(10.)
            .pr(12.)
            .py(7.)
            .child(text("💬").font(ty::ui(12.)))
            .child(text(label).font(ty::ui(12.).medium()).color(t.muted))
    };
    let (key, object_id) = (d.key.clone(), d.object_id.clone());
    button
        .on_press(move |s: &mut State| {
            // Anytype's phone idiom: the discussion opens as its own page.
            if mobile {
                nav::navigate(Route::Chat(object_id.clone()), s);
                return;
            }
            with(s, &key, |d, s| {
                d.open = true;
                d.derive(s);
                d.edit.focused = true;
            });
        })
        .build(t)
}

/// One row per message, then the trailing footer row.
fn message_rows(d: &Disc, s: &State, t: &Theme) -> Vec<El> {
    let store = &s.store;
    let space = s.nav.space().to_string();
    let images = s.get::<Images>();
    let sig = store_sig(s);
    // Rows on a touch screen hear the finger (holding opens the reaction bar).
    let touch = touch_screen(s);
    let n = d.messages.len();
    let mention_names: Vec<(String, String)> = store
        .agents
        .iter()
        .filter(|a| !a.name.is_empty())
        .map(|a| (a.name.clone(), a.icon.clone()))
        .collect();
    let mut rows: Vec<El> = Vec::with_capacity(n + 1);
    for (ix, m) in d.messages.iter().enumerate() {
        let sending = d.outgoing.as_ref().is_some_and(|o| o.id == m.id);
        let retrying: Vec<&String> = d.retrying.iter().filter(|k| k.starts_with(&m.id)).collect();
        let last = ix + 1 == n;
        // Someone else's reply that just arrived: its words fade in, and its row streams
        // until they have.
        let revealing = d.arrived.as_deref() == Some(m.id.as_str());
        let streaming = revealing && d.arriving;
        let grouped = ix > 0 && follows(&d.messages[ix - 1], m);
        let version = (
            grouped,
            d.fingerprints.get(ix).copied().unwrap_or_default(),
            sig,
            sending,
            retrying,
            ix == 0,
            last,
            t.dark,
            when(m.ts),
            revealing,
            files_sig(d, images, m),
            (d.thought_open.contains(&m.id), touch),
        );
        // A sent line that landed keeps its placeholder's keys.
        let key_id = d.row_ids.get(&m.id).unwrap_or(&m.id);
        let row_key = format!("msg-{key_id}");
        let built = memo(row_key.clone(), version, || {
            message_row(
                d,
                s,
                t,
                ix,
                grouped,
                m,
                key_id,
                revealing,
                &space,
                images,
                &mention_names,
            )
        });
        // Every other row arrives whole (a sent line, history): one quick glide open, and a
        // fade, which is all Reduce Motion keeps.
        let built = if streaming {
            built.streaming(true)
        } else if revealing {
            built
        } else {
            built.arrives_whole().transition(Transition::opacity())
        };
        let built = if last && d.own_scroller() {
            // A reply taller than the pane opens at its first line, not its last.
            let (key, id, row_key) = (d.key.clone(), m.id.clone(), row_key);
            built.on_layout_tx(move |s: &mut State, r: [f32; 4], tx: &mut Tx| {
                with(s, &key, |d, _| {
                    if d.tall_armed && d.tall_check.as_deref() == Some(id.as_str()) {
                        d.tall_check = None;
                        if r[3] > d.viewport_h * 0.8 {
                            tx.scroll_center_in(path(&d.key, "chat-messages"), row_key.clone());
                        }
                    }
                });
                tx.live_only();
            })
        } else {
            built
        };
        rows.push(built);
    }
    rows.push(footer(d, s, t, images));
    rows
}

/// Messages this close after the one before, by the same author, join its group: no
/// avatar or name of their own (Discord's grouping).
const GROUP_WINDOW_MS: i64 = 5 * 60 * 1000;

/// Whether `m` continues `prev`'s group.
fn follows(prev: &ChatMessage, m: &ChatMessage) -> bool {
    prev.author == m.author && prev.thought.is_none() && (m.ts - prev.ts).abs() < GROUP_WINDOW_MS
}

/// Message `m`'s row, Discord style: every message on the left, the avatar and a header
/// line (name, time) leading each author's group; `grouped` rows continue the group above.
/// The author's chat colour tints a card behind the words, with a bar in it on the left.
/// `key_id` scopes its element keys (the message's id, or the placeholder
/// a sent line landed in) and `revealing` fades its words in.
#[allow(clippy::too_many_arguments)]
fn message_row(
    d: &Disc,
    s: &State,
    t: &Theme,
    ix: usize,
    grouped: bool,
    m: &ChatMessage,
    key_id: &str,
    revealing: bool,
    space: &str,
    images: Option<&Images>,
    mention_names: &[(String, String)],
) -> El {
    let store = &s.store;
    let own = m.author == d.me;

    // Header: author and time.
    let sending = d.outgoing.as_ref().is_some_and(|o| o.id == m.id);
    let time = text(if sending {
        "Sending…".to_string()
    } else {
        when(m.ts)
    })
    .font(ty::ui(11.).lh(11. * 1.618))
    .color(t.muted);
    let name = d.who(&m.author, store);
    let name_ink = author_ink(d, store, t, &m.author);
    let name_el = if d.author_links(&m.author, store) {
        let target = m.author.clone();
        roostr_ui::Link::new(format!("author-{ix}"))
            .label(name)
            .text_size(14.)
            .weight(ty::SEMIBOLD)
            .color(name_ink)
            .plain()
            .on_press(move |s: &mut State| nav::navigate(Route::Object(target.clone()), s))
            .build(t)
    } else {
        text(name)
            .font(ty::ui(14.).semibold().lh(14. * 1.4))
            .color(name_ink)
    };
    let meta = (!grouped || sending).then(|| row().items_end().gap(8.).child(name_el).child(time));

    // The words on the theme's ink; their links in the author's colour.
    let md_theme = MdTheme {
        ink: t.fg,
        link: name_ink,
        mention_names: mention_names.to_vec(),
    };
    // The object it came from leads the body as that object's tag; a deleted one, none.
    let origin = origin_label(&m.origin, &d.object_id, store);
    let env = MdEnv {
        t,
        store,
        space,
        reveal: revealing.then_some(REVEAL),
    };
    let body = (!m.text.trim().is_empty() || origin.is_some()).then(|| {
        render_markdown(
            &format!("md-{key_id}"),
            &m.text,
            origin.as_deref().map(|name| (m.origin.as_str(), name)),
            &md_theme,
            &env,
        )
    });
    let files = message_files(d, s, t, images, m, key_id, md_theme.ink);
    // An agent's record of the work behind the reply, folded to one line above it.
    let thought = m
        .thought
        .as_ref()
        .map(|thought| thought_record(d, t, m, key_id, thought));
    let card = col()
        .key(format!("msg-text-{key_id}"))
        .w_full()
        .gap(6.)
        .min_w(0.)
        .children(body)
        .children(files);
    let chips = (!m.reactions.is_empty()).then(|| reaction_chips(d, s, t, m, key_id));

    // Delivery / processing status: quiet when all is well.
    let status_line = m
        .mailbox
        .as_ref()
        .and_then(|entry| status(d, s, t, ix, m, entry, own));

    // Hover actions, at the row's top right over the card (Discord's).
    let reacts = d.can_react() && !sending;
    let actions = (!d.read_only).then(|| {
        let exchange = d.is_exchange;
        let (id_reply, id_private) = (m.id.clone(), m.id.clone());
        let mut a = row()
            .gap(2.)
            .px(2.)
            .py(2.)
            .radius(6.)
            .fill(t.panel)
            .border(1., t.border)
            .group_opacity(0., 1.)
            .hover_motion(motion::HOVER_REVEAL.anim(), motion::HOVER_REVEAL.anim());
        if reacts {
            let id = m.id.clone();
            a = a.child(
                Button::new(format!("react-{ix}"))
                    .ghost()
                    .xsmall()
                    .label("🙂")
                    .tooltip("Add reaction")
                    .on_press(act(&d.key, move |d, s| {
                        d.open_react_bar(&id, BarAt::Button, s)
                    }))
                    .build(t),
            );
        }
        a = a.child(
            Button::new(format!("reply-{ix}"))
                .ghost()
                .xsmall()
                .label("↩")
                .tooltip(if exchange { "Reply to all" } else { "Reply" })
                .on_press(act(&d.key, move |d, _| d.start_reply(&id_reply, false)))
                .build(t),
        );
        if m.mailbox
            .as_ref()
            .is_some_and(|e| e.message.sender.object_id != d.object_id)
        {
            a = a.child(
                Button::new(format!("private-{ix}"))
                    .ghost()
                    .xsmall()
                    .label("Private")
                    .text_color(t.muted)
                    .tooltip("Reply privately in a separate exchange")
                    .on_press(act(&d.key, move |d, _| d.start_reply(&id_private, true)))
                    .build(t),
            );
        }
        a.pin(None, Some(-10.), Some(12.), None)
    });

    let column = col()
        .grow(1.)
        .min_w(0.)
        .gap(2.)
        .children(meta)
        .children(thought)
        .child(card)
        .children(chips)
        .children(status_line);
    // The avatar leads a group; a row continuing it keeps the avatar's width.
    let lead = if grouped {
        el().w(ROW_AVATAR.size).h(1.)
    } else {
        row_avatar(d, s, t, images, &m.author, ix)
    };
    // Hovered, the row takes the author's colour, faintly.
    let mut message = row()
        .key(row_anchor(&d.key, key_id))
        .w_full()
        .items_start()
        .gap(12.)
        .px(8.)
        .py(2.)
        .radius(6.)
        .hover_group()
        .hover(name_ink.alpha(if t.dark { 0.08 } else { 0.10 }))
        .child(lead)
        .child(column)
        .children(actions);
    if reacts {
        // A right click opens the reaction bar where it landed; on a touch screen, holding
        // the message does (the finger is the only hover there, so hovering is holding).
        let (k, id) = (d.key.clone(), m.id.clone());
        message = message.on_secondary_press(move |s: &mut State, p: PointerPress, _: &mut Tx| {
            with(s, &k, |d, s| {
                d.open_react_bar(&id, BarAt::Point(p.x, p.y), s)
            });
        });
        if touch_screen(s) {
            let (k, id) = (d.key.clone(), m.id.clone());
            message = message.on_hover_change(move |s: &mut State, inside: bool, _: &mut Tx| {
                with(s, &k, |d, s| {
                    if inside {
                        d.hold_start(&id, s);
                    } else {
                        d.hold_end(&id);
                    }
                });
            });
        }
    }
    // Room above each author's group, outside the row's hover.
    col()
        .w_full()
        .pt(if grouped { 0. } else { 18. })
        .child(message)
}

/// A message's reactions as chips under it (Discord's): emoji and count, yours accent-tinted.
/// Pressing one puts yours on or takes it back; its tooltip names who reacted.
fn reaction_chips(d: &Disc, s: &State, t: &Theme, m: &ChatMessage, key_id: &str) -> El {
    let chips = m
        .reactions
        .iter()
        .enumerate()
        .map(|(i, r)| reaction_chip(d, s, t, m, key_id, i, r));
    row().flex_wrap().gaps(4., 4.).pt(2.).children(chips)
}

fn reaction_chip(
    d: &Disc,
    s: &State,
    t: &Theme,
    m: &ChatMessage,
    key_id: &str,
    i: usize,
    r: &Reaction,
) -> El {
    let mine = r.authors.contains(&d.me);
    let mut chip = row()
        .key(format!("reaction-{key_id}-{i}"))
        .center_y()
        .gap(4.)
        .h(24.)
        .px(7.)
        .radius(12.)
        .fill(if mine { t.accent.alpha(0.16) } else { t.hover })
        .border(1., if mine { t.accent.alpha(0.7) } else { t.border })
        .child(roostr_ui::emoji_text(&r.emoji, ty::ui(14.).lh(16.)))
        .child(
            text(r.authors.len().to_string())
                .font(ty::ui(12.).semibold())
                .color(if mine { t.accent } else { t.muted }),
        );
    if d.can_react() {
        let (id, emoji) = (m.id.clone(), r.emoji.clone());
        chip = chip
            .role("button")
            .cursor(Cursor::Pointer)
            .hover(if mine { t.accent.alpha(0.26) } else { t.border })
            .on_press(act(&d.key, move |d, s| d.toggle_reaction(&id, &emoji, s)));
    }
    let names: Vec<String> = r.authors.iter().map(|a| d.who(a, &s.store)).collect();
    roostr_ui::tooltip(
        chip,
        format!("{} reacted with {}", names.join(", "), r.emoji),
    )
}

/// The reaction bar's width: the quick emoji and ＋ in a row.
const BAR_W: f32 = 7. * 36. + 6. * 2. + 8.;

/// The reaction bar open on a message, into `layer`: the quick emoji (yours marked) and ＋
/// for the full picker, then Reply and Copy text. It opens above the held row, by the row's
/// 🙂 button, or where a right click landed; a press outside or Escape closes it.
fn react_bar(
    d: &Disc,
    s: &State,
    cx: &mut Cx<State>,
    t: &Theme,
    layer: &mut Vec<El>,
    window: [f32; 2],
) {
    let Some(bar) = &d.react_bar else {
        return;
    };
    let Some(m) = d.message(&bar.message_id).filter(|_| d.can_react()) else {
        return;
    };
    let key = d.key.as_str();
    let key_id = d.row_ids.get(&m.id).unwrap_or(&m.id);
    let point = format!("{key}-react-point");
    let (anchor, placement) = match bar.at {
        BarAt::Row => (row_anchor(key, key_id), Placement::TopStart),
        BarAt::Button => (row_anchor(key, key_id), Placement::TopEnd),
        BarAt::Point(..) => (point.clone(), Placement::BottomStart),
    };
    // A right click's point, kept in from the window's right edge so the bar fits.
    let point_el = match bar.at {
        BarAt::Point(x, y) => Some(
            el().key(point)
                .absolute(x.min(window[0] - BAR_W - 8.).max(8.), y)
                .size(0., 0.),
        ),
        _ => None,
    };

    if bar.picker {
        let (k_pick, k_close, id) = (key.to_string(), key.to_string(), m.id.clone());
        let picker = roostr_properties::EmojiPicker::new(format!("{key}-react-emoji"))
            .on_pick(move |s: &mut State, emoji: &str| {
                with(s, &k_pick, |d, s| {
                    d.close_react_bar();
                    d.toggle_reaction(&id, emoji, s);
                });
            })
            .on_close(move |s: &mut State| {
                with(s, &k_close, |d, _| d.close_react_bar());
            })
            .build(s, cx, t, layer);
        layer.extend(point_el);
        layer.push(overlay::anchored(picker, &anchor, placement, 6.));
        return;
    }

    let cell = |k: String| {
        row()
            .key(k)
            .size(36., 36.)
            .center()
            .radius(8.)
            .hover(t.hover)
            .press(t.hover)
            .cursor(Cursor::Pointer)
    };
    let quick = quick_emojis(s).into_iter().enumerate().map(|(i, emoji)| {
        let on = d.reacted(&m.id, &emoji);
        let tip = if on {
            format!("Remove {emoji}")
        } else {
            format!("React with {emoji}")
        };
        let shown = roostr_ui::emoji_text(&emoji, ty::ui(20.).lh(24.));
        let id = m.id.clone();
        let mut c = cell(format!("react-pick-{i}"))
            .help(tip.clone())
            .label(tip)
            .on_press(act(key, move |d, s| {
                d.close_react_bar();
                d.toggle_reaction(&id, &emoji, s);
            }))
            .child(shown);
        if on {
            c = c.fill(t.accent.alpha(0.18));
        }
        c
    });
    let more = cell("react-more".into())
        .help("More emoji")
        .label("More emoji")
        .on_press(act(key, |d, _| {
            if let Some(bar) = &mut d.react_bar {
                bar.picker = true;
            }
        }))
        .child(glyph(Glyph::Plus, 16.).color(t.muted));
    let item = |k: &str, icon: El, label: &str| {
        row()
            .key(k.to_string())
            .role("menuitem")
            .w_full()
            .center_y()
            .gap(10.)
            .px(10.)
            .py(7.)
            .radius(6.)
            .hover(t.hover)
            .cursor(Cursor::Pointer)
            .child(row().w(16.).center().child(icon))
            .child(text(label.to_string()).font(ty::ui(13.)).color(t.fg))
    };
    let id = m.id.clone();
    let reply = item(
        "react-reply",
        text("↩").font(ty::ui(14.)).color(t.muted),
        "Reply",
    )
    .on_press(act(key, move |d, _| {
        d.close_react_bar();
        d.start_reply(&id, false);
    }));
    let copy = (!m.text.trim().is_empty()).then(|| {
        let (k, words) = (key.to_string(), m.text.clone());
        item(
            "react-copy",
            glyph(Glyph::Copy, 14.).color(t.muted),
            "Copy text",
        )
        .on_press_tx(move |s: &mut State, tx: &mut Tx| {
            tx.copy(words.clone());
            with(s, &k, |d, _| d.close_react_bar());
        })
    });
    let mut surface = col()
        .key(format!("{key}-react-bar"))
        .role("menu")
        .w(BAR_W)
        .max_w(window[0] - 16.)
        .padding(4.)
        .gap(4.)
        .fill(t.kit.popover)
        .radius(radius::KIT_LG)
        .child(row().flex_wrap().gap(2.).children(quick).child(more))
        .child(el().w_full().h(1.).fill(t.border))
        .child(reply)
        .children(copy);
    for sh in roostr_ui::shadow::kit_popover(t) {
        surface = surface.box_shadow(sh);
    }
    let close = || act(key, |d, _| d.close_react_bar());
    let right = close();
    let backdrop = overlay::scrim(format!("{key}-react-backdrop"), close())
        .on_secondary_press(move |s: &mut State, _: PointerPress, _: &mut Tx| right(s));
    layer.push(
        el().key(format!("{key}-react"))
            .absolute(0., 0.)
            .w_full()
            .h_full()
            .child(backdrop)
            .children(point_el)
            .child(overlay::anchored(surface, &anchor, placement, 6.))
            .modal(false, close()),
    );
}

/// The progress text's line height: the box shows [`PROGRESS_LINES`] of them.
const PROGRESS_LINE: f32 = 16.5;
const PROGRESS_LINES: f32 = 6.;

/// An agent's thinking and tool steps in a box of a fixed height with its own scroll bar,
/// under `header`; `live` follows the newest line unless the reader scrolled up.
fn progress_box(
    t: &Theme,
    key: String,
    header: Option<String>,
    entries: &[Entry],
    live: bool,
) -> El {
    let font = ty::ui(11.).lh(PROGRESS_LINE);
    let lines = entries.iter().filter_map(|entry| match entry {
        Entry::Thinking(prose) => {
            let prose = prose.trim();
            (!prose.is_empty()).then(|| text(prose.to_string()).font(font).color(t.muted).wrap())
        }
        Entry::Step(step) => Some(
            row()
                .gap(4.)
                .items_start()
                .child(roostr_ui::emoji_text("🔧", font))
                .child(
                    text(step.clone())
                        .font(ty::mono(11.).lh(PROGRESS_LINE))
                        .color(t.muted)
                        .wrap()
                        .shrink(),
                ),
        ),
    });
    let content = col().w_full().gap(4.).pr(14.).children(lines);
    let area = ScrollArea::vertical(format!("{key}-scroll"), content)
        .h(PROGRESS_LINE * PROGRESS_LINES)
        .w_full()
        .mode(ScrollbarMode::Always);
    let area = if live { area.stick_bottom() } else { area };
    col()
        .key(key)
        .w_full()
        .gap(4.)
        .px(10.)
        .py(6.)
        .radius(8.)
        .border(1., t.border)
        .children(header.map(|h| {
            text(h)
                .font(ty::ui(11.).medium().lh(PROGRESS_LINE))
                .color(t.muted)
        }))
        .child(area.build(t))
}

/// A reply's "Thought for 1m 20s · 6 steps ▸" line; a click opens its record below it.
fn thought_record(d: &Disc, t: &Theme, m: &ChatMessage, key_id: &str, thought: &Thought) -> El {
    let open = d.thought_open.contains(&m.id);
    let label = format!("{} {}", thought.summary(), if open { "▾" } else { "▸" });
    let id = m.id.clone();
    let toggle = row()
        .key(format!("thought-{key_id}"))
        .cursor(rune::Cursor::Pointer)
        .py(2.)
        .child(
            text(label)
                .font(ty::ui(11.).lh(PROGRESS_LINE))
                .color(t.muted),
        )
        .on_press(act(&d.key, move |d, _| {
            if !d.thought_open.remove(&id) {
                d.thought_open.insert(id.clone());
            }
        }));
    let mut record = col().w_full().items_start().gap(4.).pb(4.).child(toggle);
    if open {
        record = record.child(progress_box(
            t,
            format!("thought-box-{key_id}"),
            None,
            &thought.entries,
            false,
        ));
    }
    record
}

/// An agent at work: its name, then its run's thinking and steps live under a running clock.
fn live_row(d: &Disc, s: &State, t: &Theme, images: Option<&Images>, run: &Run) -> El {
    let store = &s.store;
    let elapsed = clock(now_ms() - run.started);
    row()
        .key(format!("live-{}", run.id))
        .w_full()
        .items_start()
        .gap(12.)
        .px(8.)
        .pt(12.)
        .pb(6.)
        .child(avatar(
            t,
            images,
            &d.avatar_emoji(&run.agent, store),
            d.initials(&run.agent, store),
            &run.agent,
            ROW_AVATAR,
        ))
        .child(
            col()
                .grow(1.)
                .min_w(0.)
                .gap(4.)
                .child(
                    text(d.who(&run.agent, store))
                        .font(ty::ui(14.).semibold().lh(14. * 1.4))
                        .color(author_ink(d, store, t, &run.agent)),
                )
                .child(progress_box(
                    t,
                    format!("live-box-{}", run.id),
                    Some(format!("Thinking… {elapsed}")),
                    &run.entries,
                    true,
                )),
        )
}

/// The widest a message's image draws; a tall one stops at [`THUMB_MAX_H`].
const THUMB_W: f32 = 240.;
const THUMB_MAX_H: f32 = 320.;

/// What message `m`'s Files draw from, hashed into its row's memo version: each one's
/// loaded state and its decoded image's size.
fn files_sig(d: &Disc, images: Option<&Images>, m: &ChatMessage) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    for id in &m.files {
        let file = d.chat_files.get(id);
        file.hash(&mut h);
        file.and_then(|f| f.image.as_deref())
            .and_then(|uri| images?.image(uri, THUMB_W))
            .and_then(|image| image.size())
            .hash(&mut h);
    }
    h.finish()
}

/// Message `m`'s Files in its bubble: an image as its thumbnail (its own shape, [`THUMB_W`]
/// wide at most), any other File as a chip with its name. Each opens its File.
#[allow(clippy::too_many_arguments)]
fn message_files(
    d: &Disc,
    s: &State,
    t: &Theme,
    images: Option<&Images>,
    m: &ChatMessage,
    key_id: &str,
    ink: Color,
) -> Option<El> {
    if m.files.is_empty() {
        return None;
    }
    let tiles = m.files.iter().enumerate().map(|(i, id)| {
        let file = d.chat_files.get(id);
        let name = s
            .store
            .summary(id)
            .map(|o| o.name.clone())
            .filter(|n| !n.is_empty())
            .or_else(|| file.map(|f| f.name.clone()).filter(|n| !n.is_empty()))
            .unwrap_or_else(|| "File".into());
        let image = file
            .and_then(|f| f.image.as_deref())
            .and_then(|uri| images?.image(uri, THUMB_W));
        let chip = |icon: El, label: String| {
            row()
                .key(format!("chat-file-{key_id}-{i}"))
                .items_center()
                .gap(6.)
                .max_w(THUMB_W)
                .px(8.)
                .py(4.)
                .radius(6.)
                .border(1., ink.alpha(0.25))
                .child(icon)
                .child(
                    text(label)
                        .font(ty::ui(12.).lh(12. * 1.45))
                        .color(ink)
                        .truncate()
                        .shrink(),
                )
        };
        let tile = match (image, file) {
            (Some(image), _) => {
                // Its own shape; a pending decode holds a landscape box.
                let (w, h) = image
                    .size()
                    .map_or((THUMB_W, THUMB_W * 0.6), |[w, h]| (w as f32, h as f32));
                let mut dw = w.min(THUMB_W);
                let mut dh = dw * h / w.max(1.);
                if dh > THUMB_MAX_H {
                    dh = THUMB_MAX_H;
                    dw = dh * w / h.max(1.);
                }
                el().key(format!("chat-image-{key_id}-{i}"))
                    .size(dw, dh)
                    .radius(8.)
                    .pixels_fit(image.pixels(), SurfaceFit::Fit)
            }
            (None, Some(f)) if !f.loading && !f.error.is_empty() => col()
                .gap(2.)
                .child(chip(glyph(Glyph::FileText, 14.).color(ink), name.clone()))
                .child(
                    text(format!("Couldn't load it: {}", f.error))
                        .font(ty::ui(11.).lh(11. * 1.45))
                        .color(t.muted)
                        .wrap(),
                ),
            (None, Some(f)) if !f.loading => {
                chip(glyph(Glyph::FileText, 14.).color(ink), name.clone())
            }
            _ => chip(
                Spinner::new().px(14.).color(ink).build(t),
                format!("Loading {name}…"),
            ),
        };
        let target = id.clone();
        tile.role("button")
            .label(format!("Open {name}"))
            .cursor(rune::Cursor::Pointer)
            .on_press(move |s: &mut State| nav::navigate(Route::Object(target.clone()), s))
    });
    Some(col().gap(6.).items_start().children(tiles))
}

/// A row's avatar; an author that is an object opens it on click, with a hover ring and an
/// "Open …" tooltip.
fn row_avatar(
    d: &Disc,
    s: &State,
    t: &Theme,
    images: Option<&Images>,
    author: &str,
    ix: usize,
) -> El {
    let store = &s.store;
    let face = avatar(
        t,
        images,
        &d.avatar_emoji(author, store),
        d.initials(author, store),
        author,
        ROW_AVATAR,
    );
    if !d.author_links(author, store) {
        return face;
    }
    let target = author.to_string();
    let r = avatar_radius(ROW_AVATAR.size);
    // Inside the avatar, over the picture: a ring outside it was cut off where
    // the chat list clips its left edge.
    let ring = el()
        .key(format!("avatar-{ix}-ring"))
        .pin(Some(0.), Some(0.), Some(0.), Some(0.))
        .radius(r)
        .border(2., t.accent)
        .group_opacity(0., 1.);
    let wrapped = row()
        .key(format!("avatar-{ix}"))
        .size(ROW_AVATAR.size, ROW_AVATAR.size)
        .hover_group()
        .hover_alone()
        .cursor(rune::Cursor::Pointer)
        .on_press(move |s: &mut State| nav::navigate(Route::Object(target.clone()), s))
        .child(face)
        .child(ring);
    roostr_ui::tooltip(wrapped, &format!("Open {}", d.who(author, store)))
}

/// A message's delivery or processing status line (with Retry where it failed).
fn status(
    d: &Disc,
    s: &State,
    t: &Theme,
    ix: usize,
    m: &ChatMessage,
    entry: &roostr_core::MailboxEntry,
    own: bool,
) -> Option<El> {
    let store = &s.store;
    let failed_delivery = if entry.outgoing {
        entry.deliveries.iter().find(|x| x.status == "failed")
    } else {
        None
    };
    let pending_delivery = entry.outgoing && entry.deliveries.iter().any(|x| x.status == "pending");
    let local_work = entry.incoming
        && (!entry.message.operation.is_empty()
            || entry
                .message
                .recipients
                .iter()
                .any(|r| r.object_id == d.object_id && !r.agent_id.is_empty()));
    let waiting_for = entry
        .message
        .recipients
        .iter()
        .filter(|r| !r.agent_id.is_empty())
        .map(|r| {
            store
                .agent(&r.agent_id)
                .map(|a| a.name.clone())
                .filter(|n| !n.is_empty())
                .unwrap_or_else(|| "the agent".into())
        })
        .collect::<Vec<_>>()
        .join(", ");
    let suffix = |e: &str| {
        if e.is_empty() {
            String::new()
        } else {
            format!(" · {e}")
        }
    };
    // `.status { display: flex; flex-wrap: wrap }`: the sentence wraps within the bubble
    // column and Retry follows it.
    let line = |failed: bool, message: String| {
        let r = row().flex_wrap().items_end().gaps(3., 6.).pt(2.).child(
            text(message)
                .font(ty::ui(11.).lh(16.5))
                .color(if failed { t.orange } else { t.muted })
                .wrap()
                .shrink(),
        );
        // Under the bubble column, 2px in.
        if own { r.px(2.) } else { r.pl(2.).pr(2.) }
    };
    let retry_button = |id: String, busy: bool, on: Box<dyn Fn(&mut Disc, &mut State)>| {
        let key = d.key.clone();
        Button::new(id)
            .outline()
            .xsmall()
            .label("Retry")
            .disabled(busy)
            .on_press(move |s: &mut State| {
                with(s, &key, |d, s| on(d, s));
            })
            .build(t)
    };
    if let Some(failed) = failed_delivery {
        let recipient = failed.recipient.object_id.clone();
        let busy = d
            .retrying
            .contains(&format!("{}:delivery:{}", m.id, recipient));
        let id = m.id.clone();
        return Some(
            line(
                true,
                format!(
                    "Not delivered to {}{}",
                    d.member_name(&failed.recipient, store),
                    suffix(&failed.error)
                ),
            )
            .child(retry_button(
                format!("retry-d-{ix}"),
                busy,
                Box::new(move |d, s| d.retry(id.clone(), "delivery", recipient.clone(), s)),
            )),
        );
    }
    if local_work && entry.processing.status == "failed" {
        let busy = d.retrying.contains(&format!("{}:processing:", m.id));
        let id = m.id.clone();
        return Some(
            line(true, format!("Failed{}", suffix(&entry.processing.error))).child(retry_button(
                format!("retry-p-{ix}"),
                busy,
                Box::new(move |d, s| d.retry(id.clone(), "processing", String::new(), s)),
            )),
        );
    }
    if local_work && entry.processing.status == "held" {
        return Some(line(
            true,
            format!("On hold{}", suffix(&entry.processing.error)),
        ));
    }
    if local_work && entry.processing.status == "awaiting_approval" {
        return Some(line(
            false,
            "Waiting for approval on the serving computer".into(),
        ));
    }
    if pending_delivery {
        return Some(line(false, "Sending…".into()));
    }
    if local_work
        && (entry.processing.status == "pending" || entry.processing.status == "processing")
    {
        return Some(line(false, format!("Waiting for {waiting_for}…")));
    }
    None
}

/// The trailing row: empty note, each agent's live progress or typing dots, or its problem.
fn footer(d: &Disc, s: &State, t: &Theme, images: Option<&Images>) -> El {
    let n = d.messages.len();
    let mut f = col().key("chat-footer").pb(4.);
    let runs: Vec<&Run> = s
        .get::<LiveRuns>()
        .map(|r| r.shown(&d.object_id, &d.thread_id).collect())
        .unwrap_or_default();
    // An agent whose run shows live needs no typing dots.
    let working: Option<AgentPresence> = d
        .agent_working()
        .filter(|p| !runs.iter().any(|r| r.agent == p.id))
        .cloned();
    let problem = if working.is_none() && runs.is_empty() {
        d.agent_error().cloned()
    } else {
        None
    };
    if n == 0 || working.is_some() || !runs.is_empty() || problem.is_some() {
        f = f.pt(if n == 0 { 4. } else { 14. });
    }
    if n == 0 {
        // A muted one-liner, not the kit's centred empty-state card.
        f = f.child(
            text("No messages yet.")
                .font(ty::ui(13.))
                .color(t.muted)
                .my(13.),
        );
    }
    for run in &runs {
        f = f.child(live_row(d, s, t, images, run));
    }
    if let Some(agent) = &working {
        let typing = row()
            .key("agent-typing")
            .center_y()
            .gap(12.)
            .px(8.)
            .pt(12.)
            .pb(6.)
            .child(avatar(
                t,
                images,
                &agent.icon,
                agent.name.chars().take(2).collect(),
                &agent.id,
                ROW_AVATAR,
            ))
            .child(
                text(agent.name.clone())
                    .font(ty::ui(12.).medium())
                    .color(author_ink(d, &s.store, t, &agent.id)),
            )
            .child(typing_dots(t));
        f = f.child(roostr_ui::tooltip(typing, "The agent is composing a reply"));
    }
    if let Some(agent) = problem {
        let hint = {
            let detail = agent.detail.to_lowercase();
            if ["auth", "key", "401", "credential"]
                .iter()
                .any(|k| detail.contains(k))
            {
                "Fix in Settings → Agent."
            } else {
                ""
            }
        };
        f = f.child(
            Alert::warning(
                "agent-problem",
                format!(
                    "{} hit a problem: {}{}",
                    agent.name,
                    agent.detail,
                    if hint.is_empty() {
                        String::new()
                    } else {
                        format!(" — {hint}")
                    }
                ),
            )
            .size(roostr_ui::KitSize::XSmall)
            .text_size(12.)
            .line_height(1.4)
            .text_color(t.orange)
            .build(t)
            .mt(2.)
            .mb(6.),
        );
    }
    f
}

/// An agent in the `@` menu: avatar, `@Name`, and the object's own name.
fn mention_row(t: &Theme, images: Option<&Images>, option: &ObjectAgentOption) -> El {
    let initials = option
        .agent_name
        .chars()
        .take(2)
        .collect::<String>()
        .to_uppercase();
    let mut r = row()
        .grow(1.)
        .shrink()
        .min_w(0.)
        .center_y()
        .gap(8.)
        .child(avatar(
            t,
            images,
            &option.icon,
            initials,
            &option.endpoint.agent_id,
            MENU_AVATAR,
        ))
        .child(
            text(format!("@{}", option.agent_name))
                .font(ty::ui(13.).medium())
                .color(t.fg),
        );
    if option.name != option.agent_name {
        r = r.child(
            text(option.name.clone())
                .font(ty::ui(11.))
                .color(t.muted)
                .truncate()
                .shrink(),
        );
    }
    r
}

/// An object in the `!` menu: icon, name, and `opened` when recently opened.
fn object_row(t: &Theme, images: Option<&Images>, icon: &str, name: &str, opened: bool) -> El {
    let icon_el = row().size(20., 20.).center();
    let icon_el = if is_image_icon(icon) {
        match images {
            Some(images) => icon_el.child(roostr_ui::emoji_or_image_icon(icon, 18., images)),
            None => icon_el,
        }
    } else {
        icon_el.child(roostr_ui::emoji_text(icon, ty::ui(13.)))
    };
    let mut r = row()
        .grow(1.)
        .shrink()
        .min_w(0.)
        .center_y()
        .gap(8.)
        .child(icon_el)
        .child(
            text(name.to_string())
                .font(ty::ui(13.).medium())
                .color(t.fg)
                .truncate()
                .grow(1.)
                .shrink(),
        );
    if opened {
        r = r.child(
            Tag::secondary()
                .size(KitSize::Small)
                .label("opened")
                .build(t),
        );
    }
    r
}

/// The `!` menu's search row: what is typed after `!` in the composer, or the hint before
/// anything is.
fn object_search_row(t: &Theme, q: &str) -> El {
    row()
        .center_y()
        .gap(8.)
        .px(12.)
        .py(8.)
        .border_sides([0., 0., 1., 0.], t.border)
        .child(glyph(Glyph::KitSearch, 14.).color(t.muted))
        .child(
            text(if q.is_empty() {
                "Search objects - recently opened first".to_string()
            } else {
                q.to_string()
            })
            .font(ty::ui(13.))
            .color(if q.is_empty() { t.muted } else { t.fg })
            .truncate()
            .shrink(),
        )
}

/// A kit command-list row: the highlight follows the keyboard (`index`) and the pointer;
/// a press picks it without taking the focus from the composer.
fn menu_item(
    t: &Theme,
    key: String,
    selected: bool,
    content: El,
    hover: impl Fn(&mut State) + 'static,
    pick: impl Fn(&mut State) + 'static,
) -> El {
    let mut e = row()
        .key(key)
        .role("option")
        .checked(selected)
        .w_full()
        .center_y()
        .gap(rems(0.5))
        .px(rems(0.5))
        .py(rems(0.375))
        .radius(radius::KIT_LG - rems(0.25) - 1.0)
        .preserve_focus_on_press(true)
        .on_hover(hover)
        .on_press(pick)
        .child(content);
    if selected {
        e = e.fill(t.kit.accent);
    }
    e
}

/// The kit command palette's frame around a menu's rows (panel, border, menu radius, the
/// chat's shadow), `max_h` tall at most.
fn menu_frame(
    t: &Theme,
    key: &str,
    header: Option<El>,
    rows: Vec<El>,
    empty: El,
    max_h: f32,
) -> El {
    let body = if rows.is_empty() {
        col().w_full().padding(rems(0.25)).child(empty)
    } else {
        col()
            .key(format!("{key}-list"))
            .role("listbox")
            .w_full()
            .max_h(max_h)
            .min_h(0.)
            .shrink()
            .scroll(ScrollSpec {
                stick_bottom: false,
            })
            .child(col().padding(rems(0.25)).items_stretch().children(rows))
    };
    col()
        .key(key.to_string())
        .fill(t.kit.popover)
        .border(1., t.kit.border)
        .radius(radius::KIT_LG)
        .box_shadow(roostr_ui::shadow::gpui(6., 24., 0., 0.35))
        .children(header)
        .child(body)
}

/// The `@` agent menu or the `!` object menu above the composer, into `layer`.
fn composer_menu(d: &Disc, s: &State, cx: &mut Cx<State>, t: &Theme, layer: &mut Vec<El>) {
    let key = d.key.clone();
    let anchor = composer_box_key(&key);
    let width = cx.layout_size(&anchor).map(|[w, _]| w);
    let images = s.get::<Images>();
    let place = |menu: El| {
        let menu = match width {
            Some(w) => menu.w(w),
            None => menu,
        };
        menu.anchored(anchor.clone(), Side::Top, rune::Align::Start, 6.)
            .transition(motion::appear(4.))
    };
    if let Some(mention) = &d.mention {
        let matches = d.mention_matches();
        if !matches.is_empty() || d.tag_options.is_some() {
            let index = d.mention_index.min(matches.len().saturating_sub(1));
            let rows: Vec<El> = matches
                .iter()
                .enumerate()
                .map(|(i, option)| {
                    let (k1, k2) = (key.clone(), key.clone());
                    let pick = option.clone();
                    menu_item(
                        t,
                        format!("mention-{}", option.key()),
                        i == index,
                        mention_row(t, images, option),
                        move |s: &mut State| {
                            with(s, &k1, |d, _| d.mention_index = i);
                        },
                        move |s: &mut State| {
                            let pick = pick.clone();
                            with(s, &k2, |d, s| d.pick_mention(pick, s));
                        },
                    )
                })
                .collect();
            let empty = row().px(8.).py(6.).child(
                text(format!("No agent matches “{}”.", mention.query))
                    .font(ty::ui(13.))
                    .color(t.muted),
            );
            let mut menu = menu_frame(
                t,
                if matches.is_empty() {
                    "mention-none"
                } else {
                    "mention-menu"
                },
                None,
                rows,
                empty,
                252.,
            );
            // Lite: the agents listed are those this device holds; offer the space.
            if let Some(notice) = roostr_core::lite_ui::space_notice(s.nav.space(), s) {
                menu = menu.child(row().px(8.).child(notice_row(t, notice)));
            }
            layer.push(place(menu));
            return;
        }
    }
    let Some(tag) = &d.object_tag else {
        return;
    };
    let matches = d.object_matches(&s.store);
    let index = d.object_tag_index.min(matches.len().saturating_sub(1));
    let space = s.nav.space().to_string();
    let rows: Vec<El> = matches
        .iter()
        .enumerate()
        .map(|(i, option)| {
            let icon = object_icon(&s.store, &option.icon, &option.type_key, &space);
            let opened = d.opened_rank.contains_key(&option.id);
            let (k1, k2) = (key.clone(), key.clone());
            let pick = option.clone();
            menu_item(
                t,
                format!("object-{}", option.id),
                i == index,
                object_row(t, images, &icon, &option.name, opened),
                move |s: &mut State| {
                    with(s, &k1, |d, _| d.object_tag_index = i);
                },
                move |s: &mut State| {
                    let pick = pick.clone();
                    with(s, &k2, |d, s| d.pick_object(pick, s));
                },
            )
        })
        .collect();
    let q = tag.query.trim().to_string();
    let empty = row().px(8.).py(6.).child(
        text(format!("No object matches “{q}”."))
            .font(ty::ui(13.))
            .color(t.muted),
    );
    let menu = menu_frame(
        t,
        "object-menu",
        Some(object_search_row(t, &q)),
        rows,
        empty,
        220.,
    );
    layer.push(place(menu));
}

/// The composer box's own key, unique per discussion and without a `/`: an
/// anchored overlay finds its anchor by the element's own key, never by a
/// path - `<discussion>/composer-box` matched nothing, and the `@` menu
/// opened at the window's top-left corner.
fn composer_box_key(discussion: &str) -> String {
    format!("{discussion}-composer-box")
}

/// How every lite notice draws: wrapping, 12px muted, with its action as an outline button.
fn notice_row(t: &Theme, notice: roostr_core::lite_ui::Notice) -> El {
    let mut r = row().flex_wrap().center_y().gap(10.).py(6.).child(
        text(notice.text)
            .font(ty::ui(12.))
            .color(t.muted)
            .wrap()
            .shrink(),
    );
    if let Some(action) = notice.action {
        r = r.child(
            roostr_ui::outline_button("lite-notice", action.label())
                .on_press(move |s: &mut State| action.run(s))
                .build(t),
        );
    }
    r
}

/// The composer: the files going with the message, the field (tags as pills; an image pasted
/// or a file dropped becomes such a file), then the round send button in your colour.
fn composer(d: &Disc, s: &State, cx: &mut Cx<State>, t: &Theme, layer: &mut Vec<El>) -> El {
    let key = d.key.clone();
    let store = &s.store;
    let colors = bubble(&store.chat_color);
    let can_send = d.can_send();
    let placeholder = if d.is_exchange {
        "Write a message… (@ agent, ! object)"
    } else {
        "Write a comment… (@ agent, ! object)"
    };
    // Each pill in the kit tag look: an agent in its chat colour, an object in green.
    let space = s.nav.space().to_string();
    let mut styles: HashMap<String, PillStyle> = HashMap::new();
    for option in &d.tagged {
        let paint = agent_tag_paint(t, store, &option.agent_name, &space);
        styles.insert(
            format!("{AGENT_TOKEN}{}", option.key()),
            PillStyle::new().bg(paint.bg).fg(paint.fg),
        );
    }
    let green = tint_paint(t, roostr_ui::color::rgb(OBJECT_GREEN));
    for tag in &d.tagged_objects {
        styles.insert(
            format!("{OBJECT_TOKEN}{}", tag.id),
            PillStyle::new().bg(green.bg).fg(green.fg),
        );
    }
    let line = 14. * 1.45;
    let (k_edit, k_change, k_key, k_submit) = (key.clone(), key.clone(), key.clone(), key.clone());
    let k_paste = key.clone();
    let field = text_input(&d.edit, ty::ui(14.).lh(line))
        .key("composer")
        .w_full()
        .color(t.fg)
        .placeholder(placeholder, t.kit.muted_foreground)
        .caret(t.kit.caret, t.kit.selection)
        .pill_style(move |id| styles.get(id).cloned().unwrap_or_default())
        .on_edit(rune::edit_access(move |s: &mut State| {
            composer_edit(s, &k_edit)
        }))
        .on_input_change(move |s: &mut State, _: &mut Tx| {
            with(s, &k_change, |d, s| d.on_change(s));
        })
        .on_input_key(move |s: &mut State, ev: &rune::KeyEvent, _: &mut Tx| {
            with(s, &k_key, |d, s| d.on_key(ev, s)).unwrap_or(false)
        })
        // An image pasted becomes an attachment; text pastes as it always did.
        .on_input_paste(
            move |s: &mut State, content: &rune::ClipboardContent, _: &mut Tx| {
                with(s, &k_paste, |d, s| d.on_paste(content, s)).unwrap_or(false)
            },
        )
        .on_submit(move |s: &mut State, _: &mut Tx| {
            with(s, &k_submit, |d, s| d.send(s));
        });
    // Up to 16 lines, then the field scrolls.
    let field = col()
        .max_h(line * 16. + 16.)
        .min_h(0.)
        .shrink()
        .scroll(ScrollSpec {
            stick_bottom: false,
        })
        .child(col().px(8.).pt(12.).pb(4.).child(field));
    composer_menu(d, s, cx, t, layer);
    let k_send = key.clone();
    let send = row()
        .key("send")
        .role("button")
        .label("Send")
        .size(24., 24.)
        .center()
        .radius(12.)
        .fill(colors.bg)
        .opacity(if can_send { 1. } else { 0.35 })
        .cursor(rune::Cursor::Pointer)
        .on_press(move |s: &mut State| {
            with(s, &k_send, |d, s| {
                if d.can_send() {
                    d.send(s);
                }
            });
        })
        .child(glyph(Glyph::KitArrowUp, 16.).color(colors.ink));
    let mut frame = col()
        .key(composer_box_key(&key))
        .fill(t.panel)
        .border(1., t.border)
        .radius(radius::MENU)
        .mt(10.)
        .children(composer_attachments(d, s, t))
        .child(field)
        .child(
            row()
                .center_y()
                .justify_between()
                .px(8.)
                .pt(4.)
                .pb(8.)
                .min_h(48.)
                .child(el())
                .child(send),
        );
    if d.takes_files() {
        let k_drop = key.clone();
        frame = frame.on_drop_files(
            move |s: &mut State, files: &[std::path::PathBuf], _: &mut Tx| {
                let files = files.to_vec();
                with(s, &k_drop, |d, s| d.drop_files(files, s));
            },
        );
    }
    frame
}

/// The files going with the next message, above the field: an image as a thumbnail, any
/// other file by its name; each dimmed under a spinner while it uploads, with a × that takes
/// it out.
fn composer_attachments(d: &Disc, s: &State, t: &Theme) -> Option<El> {
    if d.attachments.is_empty() {
        return None;
    }
    const SIDE: f32 = 56.;
    let images = s.get::<Images>();
    let tiles = d.attachments.iter().map(|a| {
        let image = a
            .preview
            .as_deref()
            .and_then(|uri| images?.image(uri, SIDE));
        let face = match image {
            Some(image) => el()
                .w_full()
                .h_full()
                .radius(8.)
                .pixels_fit(image.pixels(), SurfaceFit::Fill),
            None => col()
                .w_full()
                .h_full()
                .center()
                .gap(2.)
                .px(4.)
                .child(glyph(Glyph::FileText, 16.).color(t.muted))
                .child(
                    text(a.name.clone())
                        .font(ty::ui(9.).lh(12.))
                        .color(t.muted)
                        .truncate(),
                ),
        };
        let mut tile = el()
            .key(format!("attachment-{}", a.local))
            .label(a.name.clone())
            .size(SIDE, SIDE)
            .radius(8.)
            .fill(t.hover)
            .border(1., t.border)
            .child(face);
        if a.uploading() {
            tile = tile.child(
                row()
                    .key(format!("attachment-uploading-{}", a.local))
                    .pin(Some(0.), Some(0.), Some(0.), Some(0.))
                    .radius(8.)
                    .fill(t.panel.alpha(0.6))
                    .center()
                    .child(Spinner::new().px(16.).color(t.fg).build(t)),
            );
        }
        let (k, local) = (d.key.clone(), a.local.clone());
        tile.child(
            row()
                .key(format!("attachment-remove-{}", a.local))
                .role("button")
                .label(format!("Remove {}", a.name))
                .pin(None, Some(-6.), Some(-6.), None)
                .size(18., 18.)
                .radius(9.)
                .center()
                .fill(t.panel)
                .border(1., t.border)
                .cursor(rune::Cursor::Pointer)
                .on_press(move |s: &mut State| {
                    with(s, &k, |d, _| d.remove_attachment(&local));
                })
                .child(glyph(Glyph::Close, 10.).color(t.fg)),
        )
    });
    Some(
        row()
            .flex_wrap()
            .gaps(8., 8.)
            .px(10.)
            .pt(10.)
            .children(tiles),
    )
}

/// An exchange's reply audience: who the next message goes to, a private reply, and whether
/// agents are asked to respond.
fn audience(d: &Disc, s: &State, t: &Theme, layer: &mut Vec<El>) -> El {
    let store = &s.store;
    let key = d.key.clone();
    let reply_all = d.reply_all();
    let all_label = format!("Reply to all ({})", reply_all.len());
    let current = if d.private_recipient.is_empty() {
        all_label.clone()
    } else {
        reply_all
            .iter()
            .find(|r| r.object_id == d.private_recipient)
            .map(|r| format!("Private · {}", d.member_name(r, store)))
            .unwrap_or_else(|| all_label.clone())
    };
    let options: Vec<(String, String)> = std::iter::once((String::new(), all_label))
        .chain(reply_all.iter().map(|r| {
            (
                r.object_id.clone(),
                format!("Private · {}", d.member_name(r, store)),
            )
        }))
        .collect();
    let mut menu = PopupMenu::new();
    for (value, label) in options {
        let k = key.clone();
        let checked = value == d.private_recipient;
        menu = menu.item(PopupMenuItem::new(label).checked(checked).on_press(
            move |s: &mut State| {
                with(s, &k, |d, _| d.private_recipient = value.clone());
            },
        ));
    }
    let (k_open, k_close) = (key.clone(), key.clone());
    let trigger = Button::new("audience")
        .label(current)
        .outline()
        .xsmall()
        .disabled(d.sending)
        .on_press(move |s: &mut State| {
            with(s, &k_open, |d, _| d.audience_open = !d.audience_open);
        })
        .build(t);
    let picker = DropdownMenu::new("audience-menu")
        .trigger(trigger)
        .menu(menu)
        .open(d.audience_open)
        .placement(Placement::BottomStart)
        .on_close(move |s: &mut State| {
            with(s, &k_close, |d, _| d.audience_open = false);
        })
        .build(t, layer);
    let audience_text = if d.private_recipient.is_empty() {
        let names = d
            .audience()
            .iter()
            .map(|e| d.member_name(e, store))
            .collect::<Vec<_>>()
            .join(", ");
        format!(
            "To {}",
            if names.is_empty() {
                "no recipients".to_string()
            } else {
                names
            }
        )
    } else {
        "New private exchange. Only this recipient receives your message.".to_string()
    };
    let small = ty::ui(11.).lh(16.5);
    let k_check = key.clone();
    col()
        .mt(10.)
        .gap(5.)
        .child(
            row()
                .center_y()
                .gap(7.)
                .child(text("Reply audience").font(small).color(t.muted))
                .child(picker),
        )
        .child(text(audience_text).font(small).color(t.muted).wrap())
        .child(
            Checkbox::new("request-reply")
                .label("Ask agents to respond")
                .checked(d.request_reply)
                .disabled(d.sending)
                .xsmall()
                .on_change(move |s: &mut State, checked: bool| {
                    with(s, &k_check, |d, _| d.request_reply = checked);
                })
                .build(t),
        )
}

/// `object_id`'s instance at `place` exists (for the drawer and chat page, which make theirs
/// with fixed options).
pub(crate) fn place_options(place: Place) -> DiscussionOptions {
    match place {
        Place::Pane => DiscussionOptions {
            full: true,
            embedded: true,
            ..DiscussionOptions::default()
        },
        Place::Page => DiscussionOptions {
            full: true,
            page_mode: true,
            ..DiscussionOptions::default()
        },
        Place::Inline => DiscussionOptions::default(),
    }
}
