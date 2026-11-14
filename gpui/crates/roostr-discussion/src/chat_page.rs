//! `/app/chat/:id` (`routes/app/chat/[id]/+page.svelte`): the full-screen discussion -
//! header with back, object icon + name and comment count, messages filling the window,
//! composer pinned to the bottom.

use roostr_core::nav::{self, Route};
use roostr_core::objects::object_icon;
use roostr_core::{Object, State};
use roostr_ui::layout::is_mobile;
use roostr_ui::ty::Weights as _;
use roostr_ui::{
    Button, ButtonCustomVariant, Glyph, Images, Theme, emoji_or_image_icon, glyph, is_image_icon,
    ty,
};
use rune::{Cx, El, col, el, row, text};

use crate::chat::{DISCUSSION, message_count};
use crate::discussion::Place;
use crate::discussion_view::{instance, place_options, view};

/// Messages directly under the discussion root (the header's count).
fn comment_count(object: &Object) -> usize {
    message_count(object, DISCUSSION)
}

/// The full-screen chat of `object_id`; fills a bounded parent.
pub fn chat_page(
    s: &State,
    cx: &mut Cx<State>,
    t: &Theme,
    layer: &mut Vec<El>,
    object_id: &str,
    window: [f32; 2],
) -> El {
    let shell = col().key("chat-page").w_full().h_full().fill(t.bg);
    // The page's discussion exists (and loads the object) from its first frame.
    let discussion = instance(s, object_id, &place_options(Place::Page));
    let Some(object) = s.store.object(object_id) else {
        return shell;
    };
    let space = s.nav.space().to_string();
    let icon = object_icon(
        &s.store,
        object.field_str("iconEmoji"),
        &object.type_key,
        &space,
    );
    let name = if object.name().is_empty() {
        "Untitled".to_string()
    } else {
        object.name().to_string()
    };
    let comments = comment_count(&object);
    let back_target = object.id.clone();
    // A chat object opens here in its own page's place on a phone, so that page would only
    // send it straight back: go back past it.
    let back_in_history = object.type_key == "chat" && is_mobile(window[0]);
    let icon_el = if is_image_icon(&icon) {
        match s.get::<Images>() {
            Some(images) => emoji_or_image_icon(&icon, 15., images),
            None => el().size(15., 15.),
        }
    } else {
        roostr_ui::emoji_text(&icon, ty::ui(15.))
    };
    // The round back button with the phone's back glyph.
    let back = Button::new("back")
        .custom(
            ButtonCustomVariant::new(t)
                .color(t.panel)
                .foreground(t.fg)
                .hover(t.panel)
                .active(t.panel),
        )
        .square(40.)
        .p(0.)
        .radius(20.)
        .bg(t.panel)
        .on_press(move |s: &mut State| {
            if back_in_history {
                if s.nav.can_go_back() {
                    s.nav.back(&mut s.ui);
                } else {
                    s.nav.navigate(Route::Home, &mut s.ui);
                }
            } else {
                nav::navigate(Route::Object(back_target.clone()), s)
            }
        })
        .child(glyph(Glyph::MobileBack, 20.).color(t.fg))
        .build(t);
    let title = col()
        .grow(1.)
        .shrink()
        .min_w(0.)
        .items_center()
        .child(
            row()
                .center_y()
                .gap(4.)
                .shrink()
                .min_w(0.)
                .child(icon_el)
                .child(
                    text(name)
                        .font(ty::ui(15.).semibold().lh(15. * 1.618))
                        .color(t.fg)
                        .truncate()
                        .shrink(),
                ),
        )
        .child(
            text(format!(
                "{comments} comment{}",
                if comments == 1 { "" } else { "s" }
            ))
            .font(ty::ui(12.).lh(12. * 1.618))
            .color(t.muted),
        );
    let header = row()
        .center_y()
        .gap(4.)
        .pt(10.)
        .px(12.)
        .pb(8.)
        .child(back)
        .child(title)
        .child(el().w(40.));
    let messages = match discussion {
        Some(d) => view(d, s, cx, t, layer, window),
        None => col(),
    };
    shell
        .child(header)
        .child(col().grow(1.).min_h(0.).px(12.).pb(8.).child(messages))
}
