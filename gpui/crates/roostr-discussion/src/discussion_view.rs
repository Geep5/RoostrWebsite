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
    KitSize, Placement, PopupMenu, PopupMenuItem, Tag, Theme, glyph, motion, radius, rems, ty,
};
use rune::{
    Color, Cx, Cycle, Effect, El, PillStyle, ScrollSpec, Side, Spring, SpringPair, Transition, Tx,
    col, el, memo, row, text, text_input,
};

use crate::chat::ChatMessage;
use crate::discussion::{
    AGENT_TOKEN, AgentPresence, Disc, DiscussionOptions, OBJECT_GREEN, OBJECT_TOKEN, Place,
    ScrollAsk, agent_tag_paint, composer_edit, get, instance_key, origin_label, request, with,
};
use crate::md::{MdEnv, MdTheme, OBJECT_LINK, URL_LINK, render_markdown};
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
    size: 28.,
    letters: 10.,
    emoji: 17.,
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

/// A newly arrived message: opacity 0 → 1 while it rises 6px into place over 160ms.
fn arrival() -> Transition {
    Transition {
        insertion: Effect {
            offset: rune::Vec2::new(0.0, 6.0),
            ..Effect::opacity(0.0)
        },
        removal: Effect::IDENTITY,
        springs: SpringPair::new(motion::COLUMNS.spring(), Spring::NONE),
        ..Transition::identity()
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
                .underline(t.fg.alpha(0.4))
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

    // Messages.
    let rows = message_rows(d, s, t);
    if d.own_scroller() {
        let vp = path(key, "chat-messages");
        let (k1, k2, k3) = (key.to_string(), key.to_string(), key.to_string());
        let vp_follow = vp.clone();
        let content = col()
            .key("chat-rows")
            .w_full()
            .selection_group(true)
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
        let mut list = col().key("chat-rows").selection_group(true).children(rows);
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
        let version = (
            d.fingerprints.get(ix).copied().unwrap_or_default(),
            sig,
            sending,
            retrying,
            ix == 0,
            last,
            t.dark,
            when(m.ts),
        );
        let row_key = format!("msg-{}", m.id);
        let built = memo(row_key.clone(), version, || {
            message_row(d, s, t, ix, m, &space, images, &mention_names)
        });
        let built = if d.arrived.as_deref() == Some(m.id.as_str()) {
            built.transition(arrival())
        } else {
            built
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
    rows.push(footer(d, t, images));
    rows
}

/// Presses on a message's links: an object opens here, a web address in the browser.
fn on_link(s: &mut State, id: &str, tx: &mut Tx) {
    if let Some(object) = id.strip_prefix(OBJECT_LINK) {
        nav::navigate(Route::Object(object.to_string()), s);
    } else if let Some(url) = id.strip_prefix(URL_LINK) {
        if let Err(error) = tx.open_url(url) {
            log::warn!("open {url}: {error:?}");
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn message_row(
    d: &Disc,
    s: &State,
    t: &Theme,
    ix: usize,
    m: &ChatMessage,
    space: &str,
    images: Option<&Images>,
    mention_names: &[(String, String)],
) -> El {
    let store = &s.store;
    let own = m.author == d.me;
    let color = if own {
        store.chat_color.clone()
    } else {
        store
            .agent(&m.author)
            .map(|a| a.chat_color.clone())
            .unwrap_or_default()
    };
    let tinted = !color.is_empty();
    let colors: Bubble = bubble(&color);

    // Header: author and time.
    let sending = d.outgoing.as_ref().is_some_and(|o| o.id == m.id);
    let time = text(if sending {
        "Sending…".to_string()
    } else {
        when(m.ts)
    })
    .font(ty::ui(11.).lh(11. * 1.618))
    .color(t.muted);
    let mut meta = row().items_end().gap(8.);
    if own {
        meta = meta.justify_end().child(time);
    } else {
        let name = d.who(&m.author, store);
        let name_el = if d.author_links(&m.author, store) {
            let target = m.author.clone();
            roostr_ui::Link::new(format!("author-{ix}"))
                .label(name)
                .text_size(12.)
                .weight(ty::SEMIBOLD)
                .color(t.fg)
                .underline(t.fg.alpha(0.4))
                .on_press(move |s: &mut State| nav::navigate(Route::Object(target.clone()), s))
                .build(t)
        } else {
            text(name)
                .font(ty::ui(12.).semibold().lh(12. * 1.618))
                .color(t.fg)
        };
        meta = meta.child(name_el).child(time);
    }

    // The bubble: your chat colour, an agent's own colour, else neutral; the corner by the
    // sender is the sharp one.
    let md_theme = MdTheme {
        ink: if tinted { colors.ink } else { t.fg },
        tint: tinted.then_some(colors.tint),
        mention_names: mention_names.to_vec(),
    };
    // The object it came from leads the body as that object's tag; a deleted one, none.
    let origin = origin_label(&m.origin, &d.object_id, store);
    let env = MdEnv { t, store, space };
    let body = render_markdown(
        &format!("md-{}", m.id),
        &m.text,
        origin.as_deref().map(|name| (m.origin.as_str(), name)),
        &md_theme,
        &env,
    );
    let bubble_el = col()
        .key(format!("msg-text-{}", m.id))
        .corners(if own {
            [16., 16., 5., 16.]
        } else {
            [16., 16., 16., 5.]
        })
        .px(12.)
        .py(7.)
        .fill(if tinted { colors.bg } else { t.hover })
        .shrink()
        .min_w(0.)
        .on_link(on_link)
        .child(body);

    // Delivery / processing status: quiet when all is well.
    let status_line = m
        .mailbox
        .as_ref()
        .and_then(|entry| status(d, s, t, ix, m, entry, own));

    // Hover actions, beside the bubble on the side away from the sender.
    let actions = (!d.read_only).then(|| {
        let exchange = d.is_exchange;
        let (id_reply, id_private) = (m.id.clone(), m.id.clone());
        let mut a = row()
            .gap(2.)
            .group_opacity(0., 1.)
            .hover_motion(motion::HOVER_REVEAL.anim(), motion::HOVER_REVEAL.anim())
            .child(
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
        // Out of the flow: a zero-wide box beside the bubble holds them.
        let placed = if own {
            a.pin(None, Some(0.), Some(10.), None)
        } else {
            a.absolute(10., 0.)
        };
        el().w(0.).child(placed)
    });
    let mut content = row().w_full().items_start();
    if own {
        content = content.justify_end();
    }
    content = match (own, actions) {
        (true, Some(a)) => content.child(a).child(bubble_el),
        (false, Some(a)) => content.child(bubble_el).child(a),
        (_, None) => content.child(bubble_el),
    };

    // The bubble column is 78% of the row; the bubble hugs its text on its side.
    let mut column = col()
        .w_frac(0.78)
        .min_w(0.)
        .child(meta)
        .child(content)
        .children(status_line);
    column = if own {
        column.items_end()
    } else {
        column.items_start().pl(10.)
    };
    let mut r = row()
        .w_full()
        .items_start()
        .hover_group()
        .pt(if ix == 0 { 4. } else { 14. });
    if own {
        r = r.justify_end();
    } else {
        r = r.child(row_avatar(d, s, t, images, &m.author, ix));
    }
    r.child(column)
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
    let ring = el()
        .pin(Some(-2.), Some(-2.), Some(-2.), Some(-2.))
        .radius(r + 2.)
        .border(2., t.accent)
        .group_opacity(0., 1.);
    let wrapped = row()
        .key(format!("avatar-{ix}"))
        .size(ROW_AVATAR.size, ROW_AVATAR.size)
        .hover_group()
        .hover_alone()
        .cursor(rune::Cursor::Pointer)
        .on_press(move |s: &mut State| nav::navigate(Route::Object(target.clone()), s))
        .child(ring)
        .child(face);
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

/// The trailing row: empty note, the agent composing, or its problem.
fn footer(d: &Disc, t: &Theme, images: Option<&Images>) -> El {
    let n = d.messages.len();
    let mut f = col().key("chat-footer").pb(4.);
    let working: Option<AgentPresence> = d.agent_working().cloned();
    let problem = if working.is_none() {
        d.agent_error().cloned()
    } else {
        None
    };
    if n == 0 || working.is_some() || problem.is_some() {
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
    if let Some(agent) = &working {
        let typing = row()
            .key("agent-typing")
            .center_y()
            .gap(8.)
            .pt(2.)
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
                    .color(t.muted),
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
    let anchor = path(&key, "composer-box");
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

/// The composer: the field (tags as pills), then the round send button in your colour.
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
    col()
        .key("composer-box")
        .fill(t.panel)
        .border(1., t.border)
        .radius(radius::MENU)
        .mt(10.)
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
