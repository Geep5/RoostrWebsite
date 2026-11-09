//! Anytype's "Limit object types" (`TypeLimitPicker.svelte`): an Object
//! property can name the types it accepts; the value picker then offers
//! those alone. Empty means any object.

use std::rc::Rc;

use roostr_core::State;
use roostr_ui::{ListItem, Theme, checkbox_display, radius, section_label, ty};
use rune::{El, ScrollSpec, col, el, row, text};

use crate::widgets::{empty_note, note};

/// `selected` with `id` toggled.
pub(crate) fn toggled(selected: &[String], id: &str) -> Vec<String> {
    if selected.iter().any(|x| x == id) {
        selected.iter().filter(|x| *x != id).cloned().collect()
    } else {
        selected.iter().cloned().chain([id.to_string()]).collect()
    }
}

/// The type list with checks, keyed `key` (rows `limit-<type id>`); `on_toggle` gets the
/// type object id.
pub(crate) fn type_limit_picker(
    s: &State,
    t: &Theme,
    key: &str,
    selected: &[String],
    space_id: &str,
    on_toggle: Rc<dyn Fn(&mut State, &str)>,
) -> El {
    // Bundled types carry no space stamp; they belong to every space.
    let types: Vec<_> = s
        .store
        .types
        .iter()
        .filter(|ty| ty.space.is_empty() || ty.space == space_id)
        .collect();
    let note_text = if selected.is_empty() {
        "Any object can be linked."
    } else {
        "Only these types can be linked."
    };
    let mut list = col().items_stretch();
    for ty in &types {
        let on = selected.contains(&ty.id);
        let id = ty.id.clone();
        let toggle = on_toggle.clone();
        let icon = if ty.icon.is_empty() {
            "▣".to_string()
        } else {
            ty.icon.clone()
        };
        let name = if ty.name.is_empty() {
            ty.key.clone()
        } else {
            ty.name.clone()
        };
        let item = ListItem::new(format!("limit-{}", ty.id))
            .px(8.0)
            .py(6.0)
            .radius(radius::SM)
            .text_size(13.0)
            .pointer()
            .on_press(move |s: &mut State| toggle(s, &id))
            .child(
                row()
                    .grow(1.0)
                    .shrink()
                    .min_w(0.0)
                    .center_y()
                    .gap(8.0)
                    .child(
                        el().w(18.0)
                            .center()
                            .child(text(icon).font(ty::ui(13.0)).color(t.fg)),
                    )
                    .child(
                        text(name)
                            .font(ty::ui(13.0))
                            .color(t.fg)
                            .truncate()
                            .grow(1.0)
                            .shrink()
                            .min_w(0.0),
                    )
                    .child(
                        checkbox_display(format!("limit-chk-{}", ty.id), on)
                            .small()
                            .build(t),
                    ),
            )
            .build(t);
        list = list.child(if on { item.fill(t.hl_light) } else { item });
    }
    if types.is_empty() {
        list = list.child(el().px(6.0).pt(2.0).pb(4.0).child(empty_note(
            t,
            "This space has no types yet.",
            11.0,
        )));
    }
    col()
        .key(key.to_string())
        .gap(2.0)
        .min_w(0.0)
        .items_stretch()
        .child(
            el().px(6.0)
                .pt(6.0)
                .pb(2.0)
                .child(section_label(t, "Limit object types")),
        )
        .child(
            col()
                .key("limit")
                .max_h(148.0)
                .min_h(0.0)
                .shrink()
                .scroll(ScrollSpec {
                    stick_bottom: false,
                })
                .items_stretch()
                .child(list),
        )
        .child(el().px(6.0).pt(2.0).pb(4.0).child(note(t, note_text, 11.0)))
}

#[cfg(test)]
mod tests {
    use super::toggled;

    #[test]
    fn toggled_adds_and_removes() {
        let a = vec!["t1".to_string()];
        assert_eq!(toggled(&a, "t2"), vec!["t1", "t2"]);
        assert_eq!(toggled(&a, "t1"), Vec::<String>::new());
    }
}
