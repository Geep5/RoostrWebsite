//! A property chooser for filter and sort rules (`KeyPicker.svelte`): a
//! button showing the current property; opening it gives a search box over
//! the list. Built on the kit single-select `Select` (searchable): the kit
//! owns the popup, search, keyboard (Up/Down/Enter), Escape and click-away;
//! Roostr draws the button and the rows.

use std::collections::HashMap;
use std::rc::Rc;

use roostr_core::State;
use roostr_ui::{Glyph, Select, SelectItem, SelectState, Theme, glyph, radius, ty};
use rune::{Cx, El, el, row, text};

use crate::widgets::empty_note;

type LabelFn = Rc<dyn Fn(&str, &State) -> String>;
type PickFn = Rc<dyn Fn(&mut State, &str)>;

/// Open/search state of every key picker, by the caller's key.
#[derive(Default)]
struct KeyPickers(HashMap<String, Picker>);

/// One picker's dropdown, synced to the host's props.
#[derive(Clone, Default)]
struct Picker {
    select: SelectState,
    /// The value the selection was last placed for.
    value: String,
}

impl Picker {
    /// Takes the host's props; the list and selection only change when they do,
    /// so a re-render never clears what the person is typing.
    fn sync(&mut self, value: &str, items: &[SelectItem]) {
        let items_changed = items != self.select.items();
        if items_changed {
            self.select.set_items(items.to_vec());
        }
        if items_changed || value != self.value {
            self.value = value.to_string();
            self.select.set_selected_value(value);
        }
    }
}

/// Button + searchable property menu (web KeyPicker.svelte).
pub struct KeyPicker {
    key: String,
    value: String,
    keys: Vec<String>,
    label: LabelFn,
    on_pick: Option<PickFn>,
}

impl KeyPicker {
    /// `label` says how a key reads in the list and on the button (emoji + name).
    pub fn new(
        key: impl Into<String>,
        value: String,
        keys: Vec<String>,
        label: Rc<dyn Fn(&str, &State) -> String>,
    ) -> Self {
        Self {
            key: key.into(),
            value,
            keys,
            label,
            on_pick: None,
        }
    }

    /// Called with a key other than the current one.
    pub fn on_pick(mut self, f: impl Fn(&mut State, &str) + 'static) -> Self {
        self.on_pick = Some(Rc::new(f));
        self
    }

    pub fn build(self, s: &State, _cx: &mut Cx<State>, t: &Theme, layer: &mut Vec<El>) -> El {
        let items: Vec<SelectItem> = self
            .keys
            .iter()
            .map(|k| SelectItem::new(k.clone(), (self.label)(k, s)).keywords([k.clone()]))
            .collect();
        let current = (self.label)(&self.value, s);
        let mut picker = s
            .get::<KeyPickers>()
            .and_then(|p| p.0.get(&self.key))
            .cloned()
            .unwrap_or_else(|| Picker {
                select: SelectState::default().searchable(true),
                value: String::new(),
            });
        picker.sync(&self.value, &items);
        let query = picker.select.query.value().trim().to_string();

        let (key, value) = (self.key.clone(), self.value.clone());
        let items = Rc::new(items);
        let on_pick = self.on_pick.clone();
        let (accent, fg) = (t.accent, t.fg);
        let select = Select::new(self.key.clone(), &picker.select)
            .focus_ring(false)
            .search_placeholder("Search properties")
            .menu_width(240.0)
            .menu_max_h(300.0)
            .fill(t.panel)
            .border_color(t.border)
            .radius(radius::SM)
            .px(8.0)
            .py(4.0)
            .text_size(13.0)
            .text_color(t.fg)
            .trigger(
                row()
                    .center_y()
                    .gap(4.0)
                    .child(text(current).font(ty::ui(13.0)).color(t.fg))
                    .child(glyph(Glyph::ChevronDown, 10.0).color(t.muted)),
            )
            .render_item(move |item: &SelectItem, checked: bool| {
                el().py(2.0).min_w(0.0).shrink().child(
                    text(item.title.clone())
                        .font(ty::ui(13.0))
                        .color(if checked { accent } else { fg })
                        .truncate(),
                )
            })
            .empty(el().px(8.0).py(6.0).child(empty_note(
                t,
                format!("No property matches “{query}”."),
                12.0,
            )))
            .on_state(move |s: &mut State| {
                let p = s
                    .slot::<KeyPickers>()
                    .0
                    .entry(key.clone())
                    .or_insert_with(|| Picker {
                        select: SelectState::default().searchable(true),
                        value: String::new(),
                    });
                p.sync(&value, &items);
                &mut p.select
            })
            .on_change({
                let value = self.value.clone();
                move |s: &mut State, picked: Option<&str>| {
                    if let Some(k) = picked
                        && k != value
                        && let Some(f) = &on_pick
                    {
                        f(s, k);
                    }
                }
            });
        el().child(select.build(t, layer))
    }
}
