//! A property chooser for filter and sort rules (`KeyPicker.svelte`): a
//! button showing the current property; opening it gives a search box over
//! the list. Built on the kit `Combobox` (single select, searchable): the kit
//! owns the popup, search, keyboard (Up/Down/Enter), Escape and click-away;
//! Roostr draws the button and the rows.

use std::cell::RefCell;
use std::rc::Rc;

use gpui_kit::component::combobox::{Combobox, ComboboxEvent, ComboboxState};
use gpui_kit::component::searchable_list::{SearchableListDelegate, SearchableListItem};
use gpui_kit::component::{FocusableExt as _, Icon, IndexPath};
use gpui_kit::*;
use roostr_core::{Glyph, theme};

use crate::widgets::empty_note;

type LabelFn = Rc<dyn Fn(&str, &App) -> SharedString>;
type PickFn = Rc<dyn Fn(&str, &mut Window, &mut App)>;

/// Button + searchable property menu (web KeyPicker.svelte).
#[derive(IntoElement)]
pub struct KeyPicker {
    id: ElementId,
    value: String,
    keys: Vec<String>,
    label: LabelFn,
    on_pick: Option<PickFn>,
}

impl KeyPicker {
    /// `label` says how a key reads in the list and on the button (emoji + name).
    pub fn new(id: impl Into<ElementId>, value: String, keys: Vec<String>, label: LabelFn) -> Self {
        Self {
            id: id.into(),
            value,
            keys,
            label,
            on_pick: None,
        }
    }

    /// Called with a key other than the current one.
    pub fn on_pick(mut self, f: impl Fn(&str, &mut Window, &mut App) + 'static) -> Self {
        self.on_pick = Some(Rc::new(f));
        self
    }
}

/// One property row: its key and how it reads.
#[derive(Clone, PartialEq)]
struct KeyItem {
    key: String,
    label: SharedString,
}

impl SearchableListItem for KeyItem {
    type Value = String;

    fn title(&self) -> SharedString {
        self.label.clone()
    }

    fn value(&self) -> &String {
        &self.key
    }

    /// The label or the raw key, ignoring case.
    fn matches(&self, query: &str) -> bool {
        let q = query.trim().to_lowercase();
        q.is_empty()
            || self.label.to_lowercase().contains(&q)
            || self.key.to_lowercase().contains(&q)
    }
}

/// The property list: filtered by the query, which the empty note quotes.
struct KeyList {
    items: Vec<KeyItem>,
    matched: Vec<usize>,
    query: Rc<RefCell<String>>,
}

impl KeyList {
    fn new(items: Vec<KeyItem>, query: Rc<RefCell<String>>) -> Self {
        Self {
            matched: (0..items.len()).collect(),
            items,
            query,
        }
    }
}

impl SearchableListDelegate for KeyList {
    type Item = KeyItem;

    fn items_count(&self, _: usize) -> usize {
        self.matched.len()
    }

    fn item(&self, ix: IndexPath) -> Option<&KeyItem> {
        self.items.get(*self.matched.get(ix.row)?)
    }

    fn position<V>(&self, value: &V) -> Option<IndexPath>
    where
        KeyItem: SearchableListItem<Value = V>,
        V: PartialEq,
    {
        self.matched
            .iter()
            .position(|&i| self.items[i].value() == value)
            .map(|row| IndexPath::default().row(row))
    }

    fn perform_search(&mut self, query: &str, _: &mut Window, _: &mut App) -> Task<()> {
        *self.query.borrow_mut() = query.trim().to_string();
        self.matched = (0..self.items.len())
            .filter(|&i| self.items[i].matches(query))
            .collect();
        Task::ready(())
    }

    /// Roostr's row: 13px, the current property in the accent colour.
    fn render_item(
        &self,
        _: IndexPath,
        item: &KeyItem,
        checked: bool,
        _: &mut Window,
        _: &mut App,
    ) -> Option<AnyElement> {
        Some(
            div()
                .py(px(2.))
                .text_size(px(13.))
                .text_color(if checked {
                    theme::accent()
                } else {
                    theme::fg()
                })
                .overflow_hidden()
                .whitespace_nowrap()
                .text_ellipsis()
                .child(item.label.clone())
                .into_any_element(),
        )
    }
}

struct KeyPickerState {
    combobox: Entity<ComboboxState<KeyList>>,
    query: Rc<RefCell<String>>,
    value: String,
    /// How the current value reads on the button.
    current: SharedString,
    items: Vec<KeyItem>,
    on_pick: Option<PickFn>,
    _sub: Subscription,
}

impl KeyPickerState {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let query = Rc::new(RefCell::new(String::new()));
        let list = KeyList::new(vec![], query.clone());
        let combobox = cx.new(|cx| ComboboxState::new(list, vec![], window, cx).searchable(true));
        let sub = cx.subscribe_in(
            &combobox,
            window,
            |this, _, ev: &ComboboxEvent<KeyList>, window, cx| {
                if let ComboboxEvent::Change(values) = ev
                    && let Some(key) = values.first()
                    && *key != this.value
                    && let Some(f) = this.on_pick.clone()
                {
                    f(key, window, cx);
                }
            },
        );
        Self {
            combobox,
            query,
            value: String::new(),
            current: SharedString::default(),
            items: vec![],
            on_pick: None,
            _sub: sub,
        }
    }

    /// Takes the host's props; the list and selection only change when they do,
    /// so a re-render never clears what the person is typing.
    fn sync(
        &mut self,
        value: String,
        current: SharedString,
        items: Vec<KeyItem>,
        on_pick: Option<PickFn>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.on_pick = on_pick;
        self.current = current;
        let items_changed = items != self.items;
        if items_changed {
            self.items = items.clone();
            let list = KeyList::new(items, self.query.clone());
            self.combobox
                .update(cx, |c, cx| c.set_items(list, window, cx));
        }
        if items_changed || value != self.value {
            self.value = value.clone();
            self.combobox
                .update(cx, |c, cx| c.set_selected_values(&[value], window, cx));
        }
    }
}

impl Render for KeyPickerState {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let current = self.current.clone();
        let query = self.query.clone();
        div().flex().flex_none().child(
            Combobox::new(&self.combobox)
                .focus_ring(false)
                .search_placeholder("Search properties")
                .menu_width(px(240.))
                .menu_max_h(px(300.))
                .bg(theme::panel())
                .border_color(theme::border())
                .rounded(theme::radius::sm())
                .h_auto()
                .px(px(8.))
                .py(px(4.))
                .text_size(px(13.))
                .text_color(theme::fg())
                .render_trigger(move |_, _, _| {
                    div()
                        .flex()
                        .items_center()
                        .gap(px(4.))
                        .whitespace_nowrap()
                        .child(current.clone())
                        .child(
                            Icon::new(Glyph::ChevronDown)
                                .size(px(10.))
                                .text_color(theme::muted()),
                        )
                })
                .empty(move |_, _| {
                    empty_note(format!("No property matches “{}”.", query.borrow()))
                        .px(px(8.))
                        .py(px(6.))
                        .text_size(px(12.))
                }),
        )
    }
}

impl RenderOnce for KeyPicker {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let state = window.use_keyed_state(self.id.clone(), cx, |window, cx| {
            KeyPickerState::new(window, cx)
        });
        let items = self
            .keys
            .iter()
            .map(|k| KeyItem {
                key: k.clone(),
                label: (self.label)(k, cx),
            })
            .collect();
        let current = (self.label)(&self.value, cx);
        state.update(cx, |s, cx| {
            s.sync(self.value, current, items, self.on_pick, window, cx)
        });
        state
    }
}
