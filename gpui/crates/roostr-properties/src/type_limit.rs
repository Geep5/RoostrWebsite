//! Anytype's "Limit object types" (`TypeLimitPicker.svelte`): an Object
//! property can name the types it accepts; the value picker then offers
//! those alone. Empty means any object.

use std::rc::Rc;

use gpui_kit::component::list::ListItem;
use gpui_kit::component::{Sizable as _, h_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use roostr_core::ui::{checkbox_display, section_label};
use roostr_core::{Store, theme};

use crate::widgets::{empty_note, note};

type ToggleFn = Rc<dyn Fn(&str, &mut Window, &mut App)>;

/// The type list with checks; `on_toggle` gets the type object id.
#[derive(IntoElement)]
pub(crate) struct TypeLimitPicker {
    id: ElementId,
    selected: Vec<String>,
    space: String,
    on_toggle: ToggleFn,
}

impl TypeLimitPicker {
    pub(crate) fn new(
        id: impl Into<ElementId>,
        selected: Vec<String>,
        space: String,
        on_toggle: impl Fn(&str, &mut Window, &mut App) + 'static,
    ) -> Self {
        Self {
            id: id.into(),
            selected,
            space,
            on_toggle: Rc::new(on_toggle),
        }
    }
}

/// `selected` with `id` toggled.
pub(crate) fn toggled(selected: &[String], id: &str) -> Vec<String> {
    if selected.iter().any(|x| x == id) {
        selected.iter().filter(|x| *x != id).cloned().collect()
    } else {
        selected.iter().cloned().chain([id.to_string()]).collect()
    }
}

impl RenderOnce for TypeLimitPicker {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let store = Store::global(cx).read(cx);
        // Bundled types carry no space stamp; they belong to every space.
        let types: Vec<_> = store
            .types
            .iter()
            .filter(|t| t.space.is_empty() || t.space == self.space)
            .cloned()
            .collect();
        let note_text = if self.selected.is_empty() {
            "Any object can be linked."
        } else {
            "Only these types can be linked."
        };
        let mut list = div()
            .id(self.id)
            .flex()
            .flex_col()
            .max_h(px(148.))
            .overflow_y_scroll();
        for t in &types {
            let on = self.selected.contains(&t.id);
            let id = t.id.clone();
            let toggle = self.on_toggle.clone();
            list = list.child(
                ListItem::new(SharedString::from(format!("limit-{}", t.id)))
                    .px(px(8.))
                    .py(px(6.))
                    .rounded(theme::radius::sm())
                    .text_size(px(13.))
                    .cursor_pointer()
                    .when(on, |d| d.bg(theme::hl_light()))
                    .on_click(move |_, window, cx| toggle(&id, window, cx))
                    .child(
                        h_flex()
                            .gap(px(8.))
                            .child(div().flex_none().w(px(18.)).text_center().child(
                                if t.icon.is_empty() {
                                    "▣".to_string()
                                } else {
                                    t.icon.clone()
                                },
                            ))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .text_ellipsis()
                                    .child(if t.name.is_empty() {
                                        t.key.clone()
                                    } else {
                                        t.name.clone()
                                    }),
                            )
                            .child(
                                checkbox_display(
                                    SharedString::from(format!("limit-chk-{}", t.id)),
                                    on,
                                )
                                .small(),
                            ),
                    ),
            );
        }
        if types.is_empty() {
            list = list.child(
                empty_note("This space has no types yet.")
                    .px(px(6.))
                    .pt(px(2.))
                    .pb(px(4.))
                    .text_size(px(11.)),
            );
        }
        div()
            .flex()
            .flex_col()
            .gap(px(2.))
            .min_w_0()
            .child(
                div()
                    .px(px(6.))
                    .pt(px(6.))
                    .pb(px(2.))
                    .child(section_label("Limit object types")),
            )
            .child(list)
            .child(note(note_text, 11.).px(px(6.)).pt(px(2.)).pb(px(4.)))
    }
}
