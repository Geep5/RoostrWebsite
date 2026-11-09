//! Anytype's searchObject menu for "Link to existing page"
//! (`LinkPicker.svelte`), and the "Link URL:" prompt the marks toolbar asks.
//! Both are kit popovers held open at a point: an outside press or Escape
//! dismisses them; their field has the keyboard while they are open.

use roostr_core::State;
use roostr_core::types::{Block, is_system_type, mark, pos};
use roostr_ui::{Theme, outline_button, popover_surface, primary_button, rems, ty};
use rune::{El, Key, KeyEvent, TextEdit, TextStyle, Tx, col, el, row, text};

use crate::editor::{Editor, Editors, Toolbar, custom_content, new_id, with_editor};
use crate::slash_menu::{cmd_field, cmd_item, cmd_scroll, cmd_search, window_size};

/// The open object search: the block it links from and where it hangs.
pub(crate) struct LinkPickerState {
    /// The block the slash menu ran in (replaced when empty, else linked below).
    pub block: String,
    /// Window point the picker's top-left hangs from.
    pub at: (f32, f32),
    pub query: TextEdit,
    /// The lit candidate: the first, unless ↑/↓ moved it.
    pub highlight: usize,
}

impl LinkPickerState {
    pub(crate) fn new(block: String, at: (f32, f32)) -> Self {
        let mut query = TextEdit::new("");
        query.focused = true;
        Self {
            block,
            at,
            query,
            highlight: 0,
        }
    }
}

/// The browser's `prompt("Link URL:")` over a text selection.
pub(crate) struct LinkPromptState {
    /// The selection the toolbar's 🔗 was pressed for.
    pub toolbar: Toolbar,
    pub url: TextEdit,
}

impl LinkPromptState {
    pub(crate) fn new(toolbar: Toolbar) -> Self {
        let mut url = TextEdit::new("");
        url.focused = true;
        Self { toolbar, url }
    }
}

/// A candidate object: (id, icon, name, type key).
type Candidate = (String, String, String, String);

/// The first 8 objects of the space whose name matches `query`, minus
/// `exclude` (the page itself) and system types.
fn candidates(s: &State, exclude: &str, query: &str) -> Vec<Candidate> {
    let q = query.trim().to_lowercase();
    let store = &s.store;
    store
        .summaries
        .iter()
        .filter(|o| o.id != exclude && !is_system_type(&o.type_key))
        .filter(|o| {
            q.is_empty()
                || (if o.name.is_empty() {
                    "untitled"
                } else {
                    &o.name
                })
                .to_lowercase()
                .contains(&q)
        })
        .take(8)
        .map(|o| {
            let icon =
                roostr_core::objects::object_icon(store, &o.icon, &o.type_key, &o.channel_id);
            (
                o.id.clone(),
                icon,
                if o.name.is_empty() {
                    "Untitled".into()
                } else {
                    o.name.clone()
                },
                o.type_key.clone(),
            )
        })
        .collect()
}

/// The `TextEdit` of `object_id`'s open link picker (the scratch edit once it is gone).
pub(crate) fn link_picker_query<'a>(s: &'a mut State, object_id: &str) -> &'a mut TextEdit {
    let editors = s.slot::<Editors>();
    let present = editors
        .map
        .get(object_id)
        .is_some_and(|e| e.link_picker.is_some());
    if present {
        &mut editors
            .map
            .get_mut(object_id)
            .and_then(|e| e.link_picker.as_mut())
            .expect("checked above")
            .query
    } else {
        editors.scratch = TextEdit::new("");
        &mut editors.scratch
    }
}

/// The `TextEdit` of `object_id`'s open link prompt (the scratch edit once it is gone).
pub(crate) fn link_prompt_url<'a>(s: &'a mut State, object_id: &str) -> &'a mut TextEdit {
    let editors = s.slot::<Editors>();
    let present = editors
        .map
        .get(object_id)
        .is_some_and(|e| e.link_prompt.is_some());
    if present {
        &mut editors
            .map
            .get_mut(object_id)
            .and_then(|e| e.link_prompt.as_mut())
            .expect("checked above")
            .url
    } else {
        editors.scratch = TextEdit::new("");
        &mut editors.scratch
    }
}

impl Editor {
    /// "Link to object" from the slash menu: search the space's objects at `at`.
    pub(crate) fn open_link_picker(&mut self, id: &str, at: (f32, f32), _s: &mut State) {
        self.link_picker = Some(LinkPickerState::new(id.to_string(), at));
    }

    /// Create the link block: replace the empty slash block, else below.
    fn insert_link_block(&mut self, id: &str, target: &str, s: &mut State) {
        let empty = self.live(id).0.is_empty();
        let block = Block {
            id: new_id(),
            content: custom_content("link", &[("target", target)]),
            ..Default::default()
        };
        let task = self.w_block_add(block, id, if empty { pos::REPLACE } else { pos::BOTTOM }, s);
        self.await_then_refresh(vec![task], s);
    }

    /// The picker's candidate `target` was picked: link it and close.
    fn link_picked(&mut self, target: &str, s: &mut State) {
        let Some(picker) = self.link_picker.take() else {
            return;
        };
        self.insert_link_block(&picker.block, target, s);
    }

    /// Keys in the picker's search field: ↑/↓ move the highlight, Enter
    /// takes it, Escape closes.
    fn link_picker_key(&mut self, ev: &KeyEvent, s: &mut State, tx: &mut Tx) -> bool {
        let Some(picker) = self.link_picker.as_mut() else {
            return false;
        };
        match ev.key {
            Key::Escape => {
                self.link_picker = None;
                true
            }
            Key::Up | Key::Down => {
                let rows = candidates(s, &self.object_id, &picker.query.text);
                if rows.is_empty() {
                    return true;
                }
                let n = rows.len() as isize;
                let step = if ev.key == Key::Down { 1 } else { -1 };
                let at = picker.highlight.min(rows.len() - 1) as isize;
                let next = (at + step).rem_euclid(n) as usize;
                picker.highlight = next;
                tx.scroll_into_view(format!("cand-{}", rows[next].0));
                true
            }
            Key::Enter => {
                let rows = candidates(s, &self.object_id, &picker.query.text);
                if let Some((id, ..)) = rows.into_iter().nth(picker.highlight) {
                    self.link_picked(&id, s);
                }
                true
            }
            _ => false,
        }
    }

    /// "🔗" in the toolbar: ask for the URL (the web's `prompt("Link URL:")`).
    pub(crate) fn add_link(&mut self, _s: &mut State) {
        let Some(tb) = self.toolbar.take() else {
            return;
        };
        self.link_prompt = Some(LinkPromptState::new(tb));
    }

    /// The prompt's OK (or Enter): link the selection to the URL unless it is empty.
    fn link_prompt_submit(&mut self, s: &mut State) {
        let Some(prompt) = self.link_prompt.take() else {
            return;
        };
        let url = prompt.url.text.trim().to_string();
        if url.is_empty() {
            return;
        }
        let tb = prompt.toolbar;
        self.toolbar = Some(tb.clone());
        self.apply_mark(&tb.block, mark::LINK, Some(url), s);
    }
}

const PICKER_MIN_W: f32 = 280.;
const PICKER_MAX_W: f32 = 340.;
const PROMPT_W: f32 = 320.;
/// gpui's `snap_to_window_with_margin`.
const MARGIN: f32 = 8.;

/// `[x, y]` moved so a `size` box stays `MARGIN` inside the window.
fn snapped(cx: &rune::Cx<State>, key: &str, x: f32, y: f32, fallback: [f32; 2]) -> (f32, f32) {
    let [vw, vh] = window_size(cx);
    let [w, h] = cx.layout_size(key).unwrap_or(fallback);
    (
        x.min(vw - w - MARGIN).max(MARGIN),
        y.min(vh - h - MARGIN).max(MARGIN),
    )
}

/// The open object search, hung from its point.
pub(crate) fn link_picker_view(
    s: &State,
    cx: &mut rune::Cx<State>,
    t: &Theme,
    e: &Editor,
    layer: &mut Vec<El>,
) {
    let Some(picker) = &e.link_picker else { return };
    let oid = e.object_id.clone();
    let rows = candidates(s, &oid, &picker.query.text);
    let images = s.get::<roostr_ui::Images>();

    let mut list = col().padding(rems(0.25)).items_stretch();
    if rows.is_empty() {
        list = list.child(
            row().px(6.).py(4.).child(
                text("No matching objects.")
                    .font(ty::ui(12.))
                    .color(t.muted),
            ),
        );
    }
    let highlight = picker.highlight.min(rows.len().saturating_sub(1));
    for (i, (id, icon, name, type_key)) in rows.into_iter().enumerate() {
        let icon_el = match images {
            Some(images) => roostr_ui::emoji_or_image_icon(&icon, 16., images),
            None => el()
                .size(16., 16.)
                .center()
                .child(text(icon.clone()).font(ty::ui(16.))),
        };
        let (press_oid, hover_oid, target) = (oid.clone(), oid.clone(), id.clone());
        list = list.child(
            cmd_item(t, format!("cand-{id}"), i == highlight)
                .label(name.clone())
                .child(
                    row()
                        .grow(1.)
                        .shrink()
                        .min_w(0.)
                        .center_y()
                        .gap(8.)
                        .child(icon_el)
                        .child(
                            text(name)
                                .font(ty::ui(13.))
                                .color(t.fg)
                                .truncate()
                                .grow(1.)
                                .shrink()
                                .min_w(0.)
                                .a11y_hidden(),
                        )
                        .child(
                            text(type_key)
                                .font(ty::ui(11.))
                                .color(t.muted)
                                .a11y_hidden(),
                        ),
                )
                .on_press(move |s: &mut State| {
                    with_editor(s, &press_oid, |e, s| e.link_picked(&target, s));
                })
                .on_hover_change(move |s: &mut State, inside: bool, _: &mut Tx| {
                    if inside {
                        with_editor(s, &hover_oid, |e, _| {
                            if let Some(p) = e.link_picker.as_mut() {
                                p.highlight = i;
                            }
                        });
                    }
                }),
        );
    }

    let (edit_oid, change_oid, key_oid) = (oid.clone(), oid.clone(), oid.clone());
    let field = cmd_field(t, &picker.query, "link-search", "Search objects…")
        .on_edit(rune::edit_access(move |s: &mut State| {
            link_picker_query(s, &edit_oid)
        }))
        .on_input_change(move |s: &mut State, _: &mut Tx| {
            // The store answers the query (its first 8 matches): the first lights.
            with_editor(s, &change_oid, |e, _| {
                if let Some(p) = e.link_picker.as_mut() {
                    p.highlight = 0;
                }
            });
        })
        .on_input_key(move |s: &mut State, ev: &KeyEvent, tx: &mut Tx| {
            with_editor(s, &key_oid, |e, s| e.link_picker_key(ev, s, tx)).unwrap_or(false)
        });
    // The kit Command, unbordered, inside a held kit Popover (padding 4).
    let command = col()
        .items_stretch()
        .fill(t.kit.popover)
        .child(cmd_search(t, field))
        .child(cmd_scroll(list, rems(18.75)).preserve_focus_on_press(true));
    let surface = popover_surface(
        t,
        col()
            .key("link-picker")
            .role("dialog")
            .min_w(PICKER_MIN_W)
            .max_w(PICKER_MAX_W)
            .items_stretch()
            .child(command),
    )
    .padding(4.);
    let close_oid = oid.clone();
    let surface = roostr_ui::overlay::dismissable(
        surface,
        "link-picker",
        move |s: &mut State| {
            with_editor(s, &close_oid, |e, _| e.link_picker = None);
        },
        layer,
    );
    // The kit popover sits 0.25 rem under its point.
    let (x, y) = snapped(
        cx,
        "link-picker",
        picker.at.0,
        picker.at.1 + rems(0.25),
        [PICKER_MAX_W, 320.],
    );
    layer.push(
        surface
            .absolute(x, y)
            .transition(roostr_ui::motion::appear(-4.))
            .instant_changes(),
    );
}

/// The "Link URL:" prompt, above the selection it links.
pub(crate) fn link_prompt_view(
    _s: &State,
    cx: &mut rune::Cx<State>,
    t: &Theme,
    e: &Editor,
    layer: &mut Vec<El>,
) {
    let Some(prompt) = &e.link_prompt else { return };
    let oid = e.object_id.clone();

    let (edit_oid, submit_oid, key_oid) = (oid.clone(), oid.clone(), oid.clone());
    let field = rune::text_input(&prompt.url, TextStyle::new(13., ty::NORMAL).lh(rems(1.25)))
        .single_line()
        .key("link-url")
        .grow(1.)
        .shrink()
        .min_w(0.)
        .color(t.kit.foreground)
        .label("Link URL")
        .caret(t.kit.caret, t.kit.selection)
        .no_focus_ring()
        .on_edit(rune::edit_access(move |s: &mut State| {
            link_prompt_url(s, &edit_oid)
        }))
        .on_submit(move |s: &mut State, _: &mut Tx| {
            with_editor(s, &submit_oid, |e, s| e.link_prompt_submit(s));
        })
        .on_input_key(move |s: &mut State, ev: &KeyEvent, _: &mut Tx| {
            if ev.key != Key::Escape {
                return false;
            }
            with_editor(s, &key_oid, |e, _| e.link_prompt = None);
            true
        });
    // The kit Input, unbordered, on `hl_med` (h 28, px 8, radius 6).
    let input = row()
        .w_full()
        .h(rems(2.))
        .px(8.)
        .center_y()
        .fill(t.hl_med)
        .radius(6.)
        .cursor(rune::Cursor::Text)
        .child(field);
    let (cancel_oid, ok_oid) = (oid.clone(), oid.clone());
    let buttons = row()
        .justify_end()
        .gap(8.)
        .child(
            outline_button("link-cancel", "Cancel")
                .on_press(move |s: &mut State| {
                    with_editor(s, &cancel_oid, |e, _| e.link_prompt = None);
                })
                .build(t),
        )
        .child(
            primary_button("link-ok", "OK")
                .on_press(move |s: &mut State| {
                    with_editor(s, &ok_oid, |e, s| e.link_prompt_submit(s));
                })
                .build(t),
        );
    let surface = popover_surface(
        t,
        col()
            .key("link-prompt")
            .role("dialog")
            .w(PROMPT_W)
            .gap(8.)
            .items_stretch()
            .child(text("Link URL:").font(ty::ui(13.)).color(t.fg))
            .child(input)
            .child(buttons),
    )
    .padding(10.);
    let close_oid = oid.clone();
    let surface = roostr_ui::overlay::dismissable(
        surface,
        "link-prompt",
        move |s: &mut State| {
            with_editor(s, &close_oid, |e, _| e.link_prompt = None);
        },
        layer,
    );
    // GPUI hung it 160 left of and 130 above the toolbar, which sat
    // centred over the selection's line: over the block's text here.
    let anchor = cx
        .window_layout(&format!("text-{}", prompt.toolbar.block))
        .map_or((0., 0.), |r| (r[0] + r[2] / 2., r[1]));
    let (x, y) = snapped(
        cx,
        "link-prompt",
        anchor.0 - 160.,
        anchor.1 - 130.,
        [PROMPT_W, 120.],
    );
    layer.push(
        surface
            .absolute(x, y)
            .transition(roostr_ui::motion::appear(-4.))
            .instant_changes(),
    );
}
