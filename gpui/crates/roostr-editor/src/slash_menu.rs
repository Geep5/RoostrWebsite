//! Anytype's add-menu (`SlashMenu.svelte`): opens at the caret when "/" is
//! typed, sectioned items with a rendered style preview + description,
//! filtered live by the text typed after the slash (name + aliases). The
//! editor keeps keyboard focus (typing goes on into the block) and owns
//! navigation keys (`keys.rs` calls [`Editor::slash_move`] and
//! [`Editor::slash_confirm`]).
//!
//! Also the kit command-list look the three editor menus share (the slash
//! menu, the block menu, the link picker): panel, headings, rows.

use roostr_core::State;
use roostr_core::types::style;
use roostr_ui::ty::Weights as _;
use roostr_ui::{Theme, radius, rems, shadow, ty};
use rune::{Color, Cursor, El, TextStyle, Tx, col, el, row, text};

use crate::editor::{Editor, with_editor};

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum SlashPick {
    Style(u32),
    Table,
    Divider(&'static str),
    Relation(String),
    PropertyAdd,
    LinkObject,
    File,
}

/// How a preview tile draws (`pv-*` classes).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Preview {
    P,
    H1,
    H2,
    H3,
    List,
    Check,
    Quote,
    Code,
    Callout,
    Table,
    Div,
    Link,
    Rel,
}

#[derive(Clone, Debug)]
pub(crate) struct SlashEntry {
    pub section: &'static str,
    pub label: String,
    pub desc: String,
    pub preview: &'static str,
    pub cls: Preview,
    pub aliases: Vec<String>,
    pub pick: SlashPick,
}

fn entry(
    section: &'static str,
    label: &str,
    desc: &str,
    preview: &'static str,
    cls: Preview,
    aliases: &[&str],
    pick: SlashPick,
) -> SlashEntry {
    SlashEntry {
        section,
        label: label.to_string(),
        desc: desc.to_string(),
        preview,
        cls,
        aliases: aliases.iter().map(|a| a.to_string()).collect(),
        pick,
    }
}

/// Sections mirror Anytype's menuBlockAdd: Text / Lists / Other, then Properties.
fn base_entries() -> Vec<SlashEntry> {
    use Preview::*;
    vec![
        entry(
            "Text",
            "Text",
            "Plain paragraph",
            "Ag",
            P,
            &["paragraph", "plain"],
            SlashPick::Style(style::PARAGRAPH),
        ),
        entry(
            "Text",
            "Heading 1",
            "Big section heading",
            "Ag",
            H1,
            &["h1", "head1", "header1"],
            SlashPick::Style(style::HEADER1),
        ),
        entry(
            "Text",
            "Heading 2",
            "Medium section heading",
            "Ag",
            H2,
            &["h2", "head2", "header2"],
            SlashPick::Style(style::HEADER2),
        ),
        entry(
            "Text",
            "Heading 3",
            "Small section heading",
            "Ag",
            H3,
            &["h3", "head3", "header3"],
            SlashPick::Style(style::HEADER3),
        ),
        entry(
            "Text",
            "Quote",
            "Highlight a passage",
            "Ag",
            Quote,
            &["quote", "blockquote"],
            SlashPick::Style(style::QUOTE),
        ),
        entry(
            "Text",
            "Callout",
            "Bordered emphasis box",
            "Ag",
            Callout,
            &["callout"],
            SlashPick::Style(style::CALLOUT),
        ),
        entry(
            "Lists",
            "Checkbox",
            "Task with a to-do state",
            "☑",
            Check,
            &["todo", "checkbox", "task"],
            SlashPick::Style(style::CHECKBOX),
        ),
        entry(
            "Lists",
            "Bulleted list",
            "Simple list of items",
            "•—",
            List,
            &["bullet", "bulleted list", "ul"],
            SlashPick::Style(style::BULLET),
        ),
        entry(
            "Lists",
            "Numbered list",
            "Ordered list of items",
            "1—",
            List,
            &["number", "numbered list", "ol"],
            SlashPick::Style(style::NUMBERED),
        ),
        entry(
            "Lists",
            "Toggle",
            "Collapsible content",
            "▸",
            List,
            &["toggle", "collapse"],
            SlashPick::Style(style::TOGGLE),
        ),
        entry(
            "Other",
            "Code",
            "Monospaced snippet",
            "</>",
            Code,
            &["code", "snippet"],
            SlashPick::Style(style::CODE),
        ),
        entry(
            "Other",
            "Table",
            "3×3 simple table",
            "⊞",
            Table,
            &["table", "grid"],
            SlashPick::Table,
        ),
        entry(
            "Other",
            "Line divider",
            "Horizontal line separator",
            "—",
            Div,
            &["line", "divider", "hr", "---"],
            SlashPick::Divider("line"),
        ),
        entry(
            "Other",
            "Dots divider",
            "Three dots separator",
            "···",
            Div,
            &["dots", "dot divider", "***"],
            SlashPick::Divider("dots"),
        ),
        entry(
            "Other",
            "Link to object",
            "Embed a link to an existing object",
            "🔗",
            Link,
            &["link", "object", "existing", "page"],
            SlashPick::LinkObject,
        ),
        entry(
            "Other",
            "Image or file",
            "Upload an image, PDF or any file",
            "📎",
            Link,
            &[
                "image", "img", "picture", "photo", "pdf", "file", "upload", "attach",
            ],
            SlashPick::File,
        ),
    ]
}

impl Editor {
    /// The menu's entries for `filter` (label, description or alias match).
    pub(crate) fn slash_entries(&self, filter: &str) -> Vec<SlashEntry> {
        let mut all = base_entries();
        // No file picker in the browser.
        all.retain(|e| crate::files::CAN_PICK_FILES || e.pick != SlashPick::File);
        // Anytype's "Relations" section: "New relation" first, then the object's present properties.
        all.push(entry(
            "Properties",
            "Add property",
            "pick existing or create new",
            "＋",
            Preview::Rel,
            &[
                "property",
                "relation",
                "field",
                "add property",
                "new property",
            ],
            SlashPick::PropertyAdd,
        ));
        for (key, name, format) in &self.present_relations {
            all.push(SlashEntry {
                section: "Properties",
                label: if name.is_empty() {
                    key.clone()
                } else {
                    name.clone()
                },
                desc: format.clone(),
                preview: "≔",
                cls: Preview::Rel,
                aliases: vec![key.clone(), "property".into(), "relation".into()],
                pick: SlashPick::Relation(key.clone()),
            });
        }
        let q = filter.trim().to_lowercase();
        if q.is_empty() {
            return all;
        }
        all.into_iter()
            .filter(|e| {
                e.label.to_lowercase().contains(&q)
                    || e.desc.to_lowercase().contains(&q)
                    || e.aliases.iter().any(|a| a.contains(&q))
            })
            .collect()
    }

    /// A new filter typed after the slash: the highlight stays on the same
    /// entry when it survives the filter, else goes to the first (the kit
    /// CommandState's behaviour).
    pub(crate) fn slash_set_filter(&mut self, filter: String) {
        let Some(slash) = &self.slash else { return };
        let current = self
            .slash_entries(&slash.filter)
            .into_iter()
            .nth(slash.selected)
            .map(|e| e.pick);
        let selected = current
            .and_then(|pick| {
                self.slash_entries(&filter)
                    .iter()
                    .position(|e| e.pick == pick)
            })
            .unwrap_or(0);
        if let Some(slash) = self.slash.as_mut() {
            slash.filter = filter;
            slash.selected = selected;
        }
    }

    /// ↑/↓ while the menu is open: move the highlight, wrapping. Returns the
    /// key of the newly highlighted row, for the caller to reveal
    /// (`tx.scroll_into_view`).
    pub(crate) fn slash_move(&mut self, delta: isize) -> Option<String> {
        let slash = self.slash.as_ref()?;
        let n = self.slash_entries(&slash.filter).len() as isize;
        if n == 0 {
            return None;
        }
        let sel = (slash.selected as isize).min(n - 1);
        let next = (sel + delta).rem_euclid(n) as usize;
        self.slash.as_mut()?.selected = next;
        Some(slash_row_key(next))
    }

    /// Enter/Tab while the menu is open: apply the highlighted entry (the
    /// first when none was moved to); nothing matching closes the menu.
    pub(crate) fn slash_confirm(&mut self, s: &mut State) {
        let Some(slash) = self.slash.clone() else {
            return;
        };
        match self
            .slash_entries(&slash.filter)
            .into_iter()
            .nth(slash.selected)
        {
            Some(entry) => self.apply_slash(entry.pick, s),
            None => self.slash = None,
        }
    }
}

fn slash_row_key(index: usize) -> String {
    format!("slash-{index}")
}

/// The entries split into their (contiguous) sections, in order, each entry
/// with its flat index.
fn slash_groups(entries: &[SlashEntry]) -> Vec<(&'static str, Vec<(usize, &SlashEntry)>)> {
    let mut out: Vec<(&'static str, Vec<(usize, &SlashEntry)>)> = Vec::new();
    for (i, e) in entries.iter().enumerate() {
        match out.last_mut() {
            Some((section, items)) if *section == e.section => items.push((i, e)),
            _ => out.push((e.section, vec![(i, e)])),
        }
    }
    out
}

const SLASH_W: f32 = 300.;

/// The open slash menu (300pt, under the caret; flips above when there's no
/// room): a kit command list without its own query field. The filter is the
/// text typed after the slash; the block's field keeps the keyboard, and a
/// press on a row leaves it there.
pub(crate) fn slash_view(
    _s: &State,
    cx: &mut rune::Cx<State>,
    t: &Theme,
    e: &Editor,
    layer: &mut Vec<El>,
) {
    let Some(slash) = &e.slash else { return };
    if e.editing() != Some(slash.block.as_str()) {
        return;
    }
    let Some([cx_x, cx_y, _, cx_h]) = cx.caret(&format!("text-{}", slash.block)) else {
        return;
    };
    let entries = e.slash_entries(&slash.filter);
    let groups = slash_groups(&entries);
    let [vw, vh] = window_size(cx);
    // Rows are 48pt (36pt tile + padding), section headings ~28pt: the
    // estimate until the menu has been laid out once.
    let estimate = (12. + entries.len() as f32 * 48. + groups.len() as f32 * 28.).min(360.);
    let h = cx
        .layout_size("slash-menu")
        .map_or(estimate, |[_, h]| h.min(360.));
    let below = cx_y + cx_h + 6.;
    let left = cx_x.min(vw - SLASH_W - 12.).max(0.);
    let top = if below + h > vh - 12. {
        (cx_y - 6. - h).max(12.)
    } else {
        below
    };

    let oid = e.object_id.clone();
    let mut list = col().padding(rems(0.25)).items_stretch();
    if entries.is_empty() {
        list = list.child(
            row()
                .padding(12.)
                .child(text("No matches").font(ty::ui(12.)).color(t.muted)),
        );
    }
    for (section, items) in groups {
        list = list.child(cmd_heading(t, &section.to_uppercase()));
        for (i, entry) in items {
            let selected = slash.selected == i;
            let pick = entry.pick.clone();
            let (press_oid, hover_oid) = (oid.clone(), oid.clone());
            list = list.child(
                cmd_item(t, slash_row_key(i), selected)
                    .label(entry.label.clone())
                    .child(slash_row(t, entry))
                    .on_press(move |s: &mut State| {
                        let pick = pick.clone();
                        with_editor(s, &press_oid, |e, s| e.apply_slash(pick, s));
                    })
                    .on_hover_change(move |s: &mut State, inside: bool, _: &mut Tx| {
                        if inside {
                            with_editor(s, &hover_oid, |e, _| {
                                if let Some(slash) = e.slash.as_mut() {
                                    slash.selected = i;
                                }
                            });
                        }
                    }),
            );
        }
    }
    let menu = cmd_panel(t)
        .key("slash-menu")
        .w(SLASH_W)
        .padding(2.)
        .box_shadow(shadow::menu())
        // The block's field keeps the keyboard: its keys drive the list.
        .preserve_focus_on_press(true)
        .child(cmd_scroll(list, 348.))
        .absolute(left, top)
        .transition(roostr_ui::motion::appear(-4.))
        .instant_changes();
    layer.push(menu);
}

/// One slash row's body: the preview tile, the name and its description.
fn slash_row(t: &Theme, e: &SlashEntry) -> El {
    row()
        .grow(1.)
        .shrink()
        .min_w(0.)
        .center_y()
        .gap(10.)
        .child(preview_tile(
            t,
            e.preview,
            e.cls,
            [36., 36.],
            t.bg,
            None,
            false,
        ))
        .child(
            col()
                .shrink()
                .min_w(0.)
                .child(
                    text(e.label.clone())
                        .font(ty::ui(13.).weight(550.))
                        .color(t.fg)
                        .truncate()
                        .a11y_hidden(),
                )
                .child(
                    text(e.desc.clone())
                        .font(ty::ui(11.))
                        .color(t.muted)
                        .truncate()
                        .a11y_hidden(),
                ),
        )
}

/// The rendered style preview tile (`.preview` + `pv-*`), `size` = [w, h].
/// `ink` overrides the class's text colour; `strike` crosses the text out.
pub(crate) fn preview_tile(
    t: &Theme,
    label: &str,
    cls: Preview,
    size: [f32; 2],
    bg: Color,
    ink: Option<Color>,
    strike: bool,
) -> El {
    let (pt, weight, color, italic, mono) = match cls {
        Preview::P => (12., 400., t.muted, false, false),
        Preview::H1 => (16., 750., t.fg, false, false),
        Preview::H2 => (14., 650., t.fg, false, false),
        Preview::H3 => (12., 600., t.fg, false, false),
        Preview::List => (11., 400., t.muted, false, false),
        Preview::Check => (14., 400., t.accent, false, false),
        Preview::Quote => (12., 400., t.fg, true, false),
        Preview::Code => (10., 400., t.muted, false, true),
        Preview::Callout => (12., 400., t.fg, false, false),
        Preview::Table => (16., 400., t.muted, false, false),
        Preview::Div => (12., 400., t.fg, false, false),
        Preview::Link => (12., 400., t.fg, false, false),
        Preview::Rel => (14., 400., t.accent, false, false),
    };
    let font = if mono { ty::mono(pt) } else { ty::ui(pt) }.weight(weight);
    let font = TextStyle { italic, ..font };
    let mut glyphs = text(label.to_string())
        .font(font)
        .color(ink.unwrap_or(color))
        .a11y_hidden();
    if strike {
        glyphs = glyphs.strike(0, label.len(), None);
    }
    let tile = el()
        .size(size[0], size[1])
        .center()
        .radius(6.)
        .clip_rounded(6.)
        .fill(if cls == Preview::Callout { t.hover } else { bg });
    let tile = if italic {
        // `.italic().border_l(3).border_color(accent)`: every side takes the accent.
        tile.border_sides([1., 1., 1., 3.], t.accent)
    } else {
        tile.border(1., t.border)
    };
    tile.child(glyphs)
}

// ───────────────────────────────────────────── kit command-list look

/// The window's size in points (the kit root fills it).
pub(crate) fn window_size(cx: &rune::Cx<State>) -> [f32; 2] {
    cx.layout_size("kit-root").unwrap_or([f32::MAX, f32::MAX])
}

/// gpui-kit's `Command` surface: popover fill, radius 10 with its clip, a
/// 1pt border, a column of rows. Callers set width, padding and shadow.
pub(crate) fn cmd_panel(t: &Theme) -> El {
    col()
        .items_stretch()
        .fill(t.kit.popover)
        .radius(radius::KIT_LG)
        .clip_rounded(radius::KIT_LG)
        .border(1., t.kit.border)
        .padding(1.)
}

/// The command list's scroll viewport, at most `max_h` tall.
pub(crate) fn cmd_scroll(list: El, max_h: f32) -> El {
    col()
        .role("listbox")
        .w_full()
        .max_h(max_h)
        .min_h(0.)
        .shrink()
        .scroll(rune::ScrollSpec {
            stick_bottom: false,
        })
        .child(list)
}

/// A group heading (the kit's 12px medium muted label).
pub(crate) fn cmd_heading(t: &Theme, label: &str) -> El {
    row().w_full().px(rems(0.5)).py(rems(0.375)).child(
        text(label.to_string())
            .font(ty::ui(rems(0.75)).medium())
            .color(t.kit.muted_foreground),
    )
}

/// A rule between groups.
pub(crate) fn cmd_separator(t: &Theme) -> El {
    el().w_full()
        .py(4.)
        .child(el().w_full().h(1.).fill(t.kit.border))
        .a11y_hidden()
}

/// A kit command row keyed `key`: padding 7 × 5.25, the inner radius, the
/// accent fill while highlighted, shrinking while held as kit menu items do.
/// Callers add the body and handlers.
pub(crate) fn cmd_item(t: &Theme, key: String, highlighted: bool) -> El {
    let r = roostr_ui::motion::pressable(row())
        .key(key)
        .role("option")
        .checked(highlighted)
        .w_full()
        .center_y()
        .gap(rems(0.5))
        .px(rems(0.5))
        .py(rems(0.375))
        .radius(radius::KIT_LG - rems(0.25) - 1.)
        .cursor(Cursor::Pointer);
    if highlighted { r.fill(t.kit.accent) } else { r }
}

/// The kit command search field over `edit` (12.25px on a 17.5 line), keyed
/// `key`. Callers bind `on_edit` and the keys.
pub(crate) fn cmd_field(t: &Theme, edit: &rune::TextEdit, key: &str, placeholder: &str) -> El {
    rune::text_input(edit, TextStyle::new(rems(0.875), ty::NORMAL).lh(rems(1.25)))
        .single_line()
        .key(key.to_string())
        .grow(1.)
        .shrink()
        .min_w(0.)
        .color(t.kit.foreground)
        .placeholder(placeholder.to_string(), t.kit.muted_foreground)
        .label(placeholder.to_string())
        .caret(t.kit.caret, t.kit.selection)
        .no_focus_ring()
}

/// The kit command search bar: a magnifier and `field`, a rule under them.
pub(crate) fn cmd_search(t: &Theme, field: El) -> El {
    row()
        .px(rems(0.75))
        .pb(1.)
        .border_sides([0., 0., 1., 0.], t.kit.border)
        .child(
            row()
                .w_full()
                .h(rems(2.))
                .gap(6.)
                .center_y()
                .cursor(Cursor::Text)
                .child(
                    roostr_ui::glyph(roostr_ui::Glyph::KitSearch, rems(1.))
                        .color(t.kit.muted_foreground),
                )
                .child(field),
        )
}
