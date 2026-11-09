//! The space rail (`nav.vault`, `+layout.svelte:1317-1374`, CSS 1959-2134):
//! spaces in this person's order with drag reorder, New space, the sync dot,
//! the account/Settings button and the resize handle.

use roostr_core::State;
use roostr_core::sync_status::{sync_progress, sync_tip};
use roostr_core::types::{Space, SyncPhase};
use roostr_ui::ty::Weights as _;
use roostr_ui::{Avatar, Glyph, Images, KitSize, ProgressCircle, Theme, motion, radius, ty};
use rune::{Cursor, Drag, El, LoopSpec, Tx, col, el, row, text};

use crate::drag::{self, Axis};
use crate::shell::{Resize, Shell, current_space, fill, hover_fg, icon_text};

/// A rail tile button below the spaces (New space, Settings): radius 10,
/// muted → fg on an `hl-med` fill; a 52 pt row in a wide rail, else a 44 pt
/// square.
fn rail_tile(t: &Theme, key: &'static str, wide: bool) -> El {
    let tile = fill(key, t.hl_med).radius(10.);
    if wide {
        tile.w_full().h(52.).px(8.).justify_start().gap(12.)
    } else {
        tile.size(44., 44.).gap(12.)
    }
}

/// A space's square: its icon (or initial) on the panel fill.
fn space_icon(s: &State, t: &Theme, icon: &str, name: &str, size: f32, font: f32) -> El {
    let icon = if icon.is_empty() {
        name.chars()
            .next()
            .map(|c| c.to_uppercase().collect::<String>())
            .unwrap_or_else(|| "?".into())
    } else {
        icon.to_string()
    };
    row()
        .size(size, size)
        .center()
        .radius(9.)
        .fill(t.panel)
        .clip_rounded(9.)
        .child(icon_text(s, &icon, size, ty::ui(font)).color(t.fg))
}

/// The rail card, `width` wide.
pub(crate) fn view(s: &State, _cx: &mut rune::Cx<State>, t: &Theme, sh: &Shell, width: f32) -> El {
    let wide = sh.rail_wide();
    let store = &s.store;
    let spaces: Vec<&Space> = store.ordered_spaces();
    let user_color = roostr_ui::color::hex(&store.chat_color).unwrap_or_else(|| {
        roostr_ui::color::hex(roostr_ui::color::DEFAULT_CHAT_COLOR).expect("valid")
    });
    let status = &store.status;
    let current = current_space(s).map(|c| c.id.clone());

    let mut rail = col()
        .key("rail")
        .ml(6.)
        .w(width)
        .h_full()
        .gap(4.)
        .py(10.)
        .radius(radius::PANE)
        .fill(t.bg);
    rail = if wide {
        rail.px(8.).items_stretch()
    } else {
        rail.items_center()
    };
    // The spaces and New space scroll when the window is too short for
    // them (a phone in landscape); the sync dot and Settings stay put.
    let mut list = col()
        .key("rail-spaces")
        .grow(1.)
        .min_h(0.)
        .w_full()
        .gap(4.)
        .scroll(rune::ScrollSpec {
            stick_bottom: false,
        });
    list = if wide {
        list.items_stretch()
    } else {
        list.items_center()
    };
    // The active space's highlight is one element that slides to the picked
    // tile, under the tiles (tiles are fixed-size: 44 pt narrow, 52 pt wide,
    // 4 pt apart).
    let tile_h = if wide { 52. } else { 44. };
    if let Some(index) = current
        .as_deref()
        .and_then(|c| spaces.iter().position(|sp| sp.id == c))
    {
        let (left, w) = if wide {
            (0., width - 16.)
        } else {
            ((width - 44.) / 2., 44.)
        };
        list = list.child(
            el().key("rail-active")
                .absolute(left, 0.)
                .w(w)
                .h(tile_h)
                .offset(0., index as f32 * (tile_h + 4.))
                .offset_motion(motion::Tween::bezier(260, [0.22, 1.0, 0.36, 1.0]).anim())
                .radius(10.)
                .fill(roostr_ui::color::mix_srgb(user_color, 0.16, t.panel))
                .border(2., user_color),
        );
    }
    let dragging = drag::dragging(s, "spaces").map(str::to_string);
    for space in &spaces {
        let id = space.id.clone();
        let active = current.as_deref() == Some(&id);
        let title = if space.members.is_empty() {
            space.name.clone()
        } else {
            format!("{} · {} member(s)", space.name, space.members.len())
        };
        let mut tile = row()
            .key(format!("space:{id}"))
            .items_center()
            .gap(12.)
            .radius(10.)
            .cursor(Cursor::Pointer)
            .label(space.name.clone())
            .child(space_icon(
                s,
                t,
                &space.icon,
                &space.name,
                if wide { 38. } else { 36. },
                if wide { 16. } else { 15. },
            ));
        tile = if wide {
            tile.w_full().h(52.).px(8.).justify_start().child(
                text(space.name.clone())
                    .font(ty::ui(15.).medium())
                    .color(if active { t.fg } else { t.muted })
                    .truncate()
                    .grow(1.)
                    .shrink()
                    .min_w(0.),
            )
        } else {
            tile.size(44., 44.).justify_center()
        };
        if !active {
            tile = tile.hover(t.panel);
        }
        if dragging.as_deref() == Some(id.as_str()) {
            tile = tile.opacity(0.4);
        }
        if let Some(after) = drag::over(s, "spaces", &id) {
            let edge = if after {
                (None, Some(-4.))
            } else {
                (Some(-4.), None)
            };
            tile = tile.child(
                el().pin(Some(-2.), edge.0, Some(-2.), edge.1)
                    .h(2.)
                    .radius(2.)
                    .fill(t.accent),
            );
        }
        let pick = id.clone();
        let tile = tile.on_press(move |s: &mut State| s.nav.use_space(&pick, &mut s.ui));
        let tile = drag::item(tile, "spaces", &id, Axis::Y, commit_space_move);
        list = list.child(roostr_ui::tooltip(tile, title));
    }

    // New space.
    let mut new_space_tile = rail_tile(t, "new-space", wide).label("New space").child(
        row()
            .size(if wide { 38. } else { 36. }, if wide { 38. } else { 36. })
            .center()
            .radius(9.)
            .fill(t.panel)
            .child(
                text("+")
                    .font(ty::ui(if wide { 16. } else { 15. }))
                    .color(t.fg),
            ),
    );
    if wide {
        new_space_tile = new_space_tile.child(hover_fg(
            text("New space")
                .font(ty::ui(15.).medium())
                .truncate()
                .shrink()
                .min_w(0.),
            t,
        ));
    }
    list = list.child(roostr_ui::tooltip(
        new_space_tile.on_press(new_space),
        "New space",
    ));
    rail = rail.child(list);

    // Sync dot.
    let progress = sync_progress(status);
    if status.phase != SyncPhase::Live || status.pending.unwrap_or(0) > 0 || progress.is_some() {
        let err = status.phase == SyncPhase::Error;
        let pct = progress.and_then(|p| p.pct);
        // The state colours cross-fade over 200 ms.
        let color = match (err, pct) {
            (true, _) => t.red,
            (false, Some(_)) => t.border,
            (false, None) => t.accent,
        };
        let mut dot = row()
            .key("sync-dot")
            .size(8., 8.)
            .mb(6.)
            .self_center()
            .radius(4.)
            .fill(color)
            .token_motion(motion::Tween::ease(200).spring());
        dot = match (err, pct) {
            (true, _) => dot,
            // The pass's percentage as the kit's progress ring (eased to
            // each new value) on the dot's border-grey fill.
            (false, Some(pct)) => dot.child(
                ProgressCircle::new("sync-ring")
                    .color(t.accent)
                    .value(pct as f32)
                    .size(KitSize::Px(8.))
                    .build(t),
            ),
            (false, None) => dot.looping(LoopSpec::opacity(1.2, 0.0, 0.3, 1.0)),
        };
        rail = rail.child(roostr_ui::tooltip(dot, sync_tip(status)));
    }

    // Account / Settings.
    let profile_name = s.profile.name().to_string();
    let label = if profile_name.is_empty() {
        "Settings".to_string()
    } else {
        format!("@{}", profile_name.trim_start_matches('@'))
    };
    let mut settings = rail_tile(t, "rail-settings", wide)
        .label(label.clone())
        .child(account_avatar(
            s,
            t,
            if wide { 38. } else { 36. },
            Glyph::Person,
        ));
    if wide {
        settings = settings.child(hover_fg(
            text(label)
                .font(ty::ui(15.).medium())
                .truncate()
                .shrink()
                .min_w(0.),
            t,
        ));
    }
    rail.child(roostr_ui::tooltip(
        settings.on_press(crate::open_settings),
        "Settings",
    ))
}

/// The floating copy of a dragged space tile.
pub(crate) fn space_ghost(s: &State, t: &Theme, layer: &mut Vec<El>) {
    let Some(id) = drag::dragging(s, "spaces") else {
        return;
    };
    let Some(space) = s.store.space(id) else {
        return;
    };
    let ghost = el()
        .opacity(0.8)
        .child(space_icon(s, t, &space.icon, &space.name, 36., 15.));
    drag::ghost(s, "spaces", ghost, layer);
}

/// `.rail-resize`: a 12 pt hit zone right of the rail with a centred pill.
/// `transition: all 0.15s cubic-bezier(0.22, 1, 0.36, 1)`: the pill fades in
/// while the zone is hovered or held, stretches 32 → 64 pt while dragging,
/// and darkens under the pointer. A press that doesn't move toggles the rail.
pub(crate) fn resize_handle(t: &Theme, sh: &Shell, rail_col: f32) -> El {
    let active = matches!(sh.resize, Some(Resize::Rail { .. }));
    let height = if active { 64. } else { 32. };
    let pill = el()
        .key("rail-handle-pill")
        .w(6.)
        .h_full()
        .radius(3.)
        .fill(if active { t.resize_active } else { t.hover })
        .group_opacity(if active { 1. } else { 0. }, 1.)
        .token_motion(motion::HANDLE.spring());
    let handle = row()
        .key("rail-handle")
        .pin(Some(0.), None, Some(0.), None)
        .self_center()
        .h(height)
        .justify_center()
        .motion(motion::HANDLE.spring())
        .hover_group()
        .hover_motion(motion::HANDLE.anim(), motion::HANDLE.anim())
        .child(pill);
    row()
        .key("rail-resize")
        .pin(Some(rail_col), Some(6.), None, Some(6.))
        .w(12.)
        .items_center()
        .cursor(Cursor::ResizeColumn)
        .hover_group()
        .hover_motion(motion::HANDLE.anim(), motion::HANDLE.anim())
        .on_drag(|s: &mut State, d: &mut Drag, _: &mut Tx| {
            let start = s.slot::<Shell>().rail_width;
            crate::workspace::resize_drag(
                s,
                d,
                Resize::Rail {
                    start,
                    moved: false,
                },
            );
        })
        .child(handle)
}

/// `newSpace`: name prompt → `space.create` → refresh → select it.
pub(crate) fn new_space(s: &mut State) {
    crate::modal::ask(s, "Space name:", |s, name| {
        if name.is_empty() {
            return;
        }
        let task = roostr_core::api::space::create(&name, None, s);
        s.rt.spawn(async move |cx| {
            let Ok(id) = task.await else { return };
            cx.update(|s| {
                s.store.refresh_all();
                s.nav.use_space(&id, &mut s.ui);
            });
        })
        .detach();
    });
}

/// The account picture as gpui-kit's Avatar: the profile image (a data URL
/// or a web address), else `placeholder` - which also shows while the image
/// loads or if it fails.
pub(crate) fn account_avatar(s: &State, t: &Theme, size: f32, placeholder: Glyph) -> El {
    let mut avatar = Avatar::new()
        .size(KitSize::Px(size))
        .placeholder(placeholder)
        .placeholder_size(size * 0.5);
    let picture = s.profile.picture();
    if !picture.is_empty()
        && let Some(image) = s.get::<Images>().and_then(|i| i.image(picture, size))
    {
        avatar = avatar.src(image.pixels().clone());
    }
    avatar.build(t)
}

/// `commitSpaceMove`: midpoint between the drop neighbours, or a step past the edge.
pub(crate) fn commit_space_move(s: &mut State, dragged: &str, target: &str, after: bool) {
    if dragged.is_empty() || target.is_empty() || dragged == target {
        return;
    }
    let store = &s.store;
    let order_of = |sp: &Space| {
        store
            .space_order
            .get(&sp.id)
            .copied()
            .unwrap_or(sp.created_at as f64)
    };
    let rest: Vec<&Space> = store
        .ordered_spaces()
        .into_iter()
        .filter(|sp| sp.id != dragged)
        .collect();
    let Some(ix) = rest.iter().position(|sp| sp.id == target) else {
        return;
    };
    let at = ix + usize::from(after);
    let before = at.checked_sub(1).and_then(|i| rest.get(i));
    let next = rest.get(at);
    let position = match (before, next) {
        (Some(b), Some(n)) => (order_of(b) + order_of(n)) / 2.,
        (None, Some(n)) => order_of(n) - 1000.,
        (Some(b), None) => order_of(b) + 1000.,
        (None, None) => return,
    };
    s.store.space_order.insert(dragged.to_string(), position);
    let order = s.store.space_order.clone();
    roostr_core::api::user_prefs::set_space_order(&order, s).detach_and_log_err(&s.rt);
}
