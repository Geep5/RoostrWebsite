//! Anytype's add-menu (`SlashMenu.svelte`): opens at the caret when "/" is
//! typed, sectioned items with a rendered style preview + description,
//! filtered live by the text typed after the slash (name + aliases). The
//! editor keeps keyboard focus (typing goes on into the block) and owns
//! navigation keys.

use std::rc::Rc;

use gpui_kit::component::IndexPath;
use gpui_kit::component::command::{Command, CommandGroup, CommandItem, CommandState};
use gpui_kit::*;
use roostr_core::theme;
use roostr_core::types::style;

use crate::editor::Editor;

#[derive(Clone, Debug, PartialEq)]
pub enum SlashPick {
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
pub enum Preview {
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
pub struct SlashEntry {
    pub section: &'static str,
    pub label: SharedString,
    pub desc: SharedString,
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
        label: label.to_string().into(),
        desc: desc.to_string().into(),
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
                    key.clone().into()
                } else {
                    name.clone().into()
                },
                desc: format.clone().into(),
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

    /// The open slash menu (300px, at the caret; flips above when there's no
    /// room): a kit command list without its own query field — the filter is
    /// the text typed after the slash, and the editor keeps focus and routes
    /// ↑/↓/Enter to the list's highlight.
    pub(crate) fn render_slash(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let slash = self.slash.clone()?;
        let anchor = match slash.pos {
            Some(p) => p,
            None => {
                let layouts = self.layouts.borrow();
                let layout = layouts.get(&slash.block)?;
                let caret = layout.position_for_index(slash.start + 1);
                let p = point(caret.x, caret.y + layout.line_height + px(6.));
                drop(layouts);
                if let Some(s) = self.slash.as_mut() {
                    s.pos = Some(p);
                }
                p
            }
        };
        let cmd = match slash.cmd {
            Some(cmd) => cmd,
            None => {
                let cmd = cx.new(|cx| CommandState::new(window, cx));
                if let Some(s) = self.slash.as_mut() {
                    s.cmd = Some(cmd.clone());
                }
                cmd
            }
        };
        let filtered = self.slash_entries(&slash.filter);
        let groups = slash_groups(&filtered);
        let viewport = window.viewport_size();
        let w = px(300.);
        // Rows are 48px (36px tile + padding), section headings ~28px.
        let h = px((12. + filtered.len() as f32 * 48. + groups.len() as f32 * 28.).min(360.));
        let left = anchor.x.min(viewport.width - w - px(12.));
        let top = if anchor.y + h > viewport.height - px(12.) {
            (anchor.y - h - px(28.)).max(px(12.))
        } else {
            anchor.y
        };

        let picks: Rc<Vec<Vec<SlashPick>>> = Rc::new(
            groups
                .iter()
                .map(|(_, items)| items.iter().map(|e| e.pick.clone()).collect())
                .collect(),
        );
        let editor = cx.entity().downgrade();
        let mut list = Command::new(&cmd)
            .searchable(false)
            .bordered(false)
            .max_h(px(348.))
            .on_confirm(move |ix, window, cx| {
                let Some(pick) = picks.get(ix.section).and_then(|s| s.get(ix.row)).cloned() else {
                    return;
                };
                let _ = editor.update(cx, |this, cx| this.apply_slash(pick, window, cx));
            })
            .empty(|_, _, _| {
                div()
                    .p(px(12.))
                    .text_size(px(12.))
                    .text_color(theme::muted())
                    .child("No matches")
            })
            .w(w)
            .p(px(2.))
            .bg(theme::panel())
            .border_1()
            .border_color(theme::border())
            .rounded(px(10.))
            .shadow(roostr_core::ui::menu_shadow());
        let mut i = 0;
        for (section, items) in groups {
            let mut group = CommandGroup::new().label(section.to_uppercase());
            for e in items {
                let ix = i;
                i += 1;
                let keywords = e.aliases.clone();
                group = group.item(
                    CommandItem::new()
                        .label(e.label.clone())
                        .keywords(keywords)
                        .child(move |_, _| slash_row(ix, &e)),
                );
            }
            list = list.group(group);
        }
        // Appears once per opening (keyed by where the `/` was typed), so
        // filtering re-renders it in place.
        let appear = roostr_core::motion::appear_frame(
            ElementId::Name(format!("slash-appear-{}-{}", slash.block, slash.start).into()),
            -4.,
            window,
            cx,
        );
        Some(
            deferred(
                anchored().position(point(left, top)).child(
                    appear.apply(
                        div()
                            .id("slash-menu")
                            // The editor keeps focus: the list must not take
                            // it on click (its keys belong to the block).
                            .capture_any_mouse_down(|_, window, _| window.prevent_default())
                            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                            .child(list),
                    ),
                ),
            )
            .with_priority(2)
            .into_any_element(),
        )
    }
}

/// The entries split into their (contiguous) sections, in order.
fn slash_groups(entries: &[SlashEntry]) -> Vec<(&'static str, Vec<SlashEntry>)> {
    let mut out: Vec<(&'static str, Vec<SlashEntry>)> = Vec::new();
    for e in entries {
        match out.last_mut() {
            Some((section, items)) if *section == e.section => items.push(e.clone()),
            _ => out.push((e.section, vec![e.clone()])),
        }
    }
    out
}

/// The command-list path of each entry, in flat order.
pub(crate) fn slash_paths(entries: &[SlashEntry]) -> Vec<IndexPath> {
    slash_groups(entries)
        .iter()
        .enumerate()
        .flat_map(|(section, (_, items))| {
            (0..items.len()).map(move |row| IndexPath::new(row).section(section))
        })
        .collect()
}

/// One slash row: the preview tile, the name and its description.
fn slash_row(i: usize, e: &SlashEntry) -> Stateful<Div> {
    div()
        .id(("slash-item", i))
        .flex()
        .items_center()
        .gap(px(10.))
        .w_full()
        .cursor_pointer()
        .child(preview_tile(
            e.preview,
            e.cls,
            px(36.),
            px(36.),
            theme::bg(),
        ))
        .child(
            div()
                .flex()
                .flex_col()
                .min_w_0()
                .child(
                    div()
                        .text_size(px(13.))
                        .font_weight(FontWeight(550.))
                        .text_color(theme::fg())
                        .child(e.label.clone()),
                )
                .child(
                    div()
                        .text_size(px(11.))
                        .text_color(theme::muted())
                        .child(e.desc.clone()),
                ),
        )
}

/// The rendered style preview tile (`.preview` + `pv-*`).
pub(crate) fn preview_tile(
    text: &'static str,
    cls: Preview,
    w: Pixels,
    h: Pixels,
    bg: Hsla,
) -> Div {
    let (size, weight, color, italic, mono) = match cls {
        Preview::P => (12., 400., theme::muted(), false, false),
        Preview::H1 => (16., 750., theme::fg(), false, false),
        Preview::H2 => (14., 650., theme::fg(), false, false),
        Preview::H3 => (12., 600., theme::fg(), false, false),
        Preview::List => (11., 400., theme::muted(), false, false),
        Preview::Check => (14., 400., theme::accent(), false, false),
        Preview::Quote => (12., 400., theme::fg(), true, false),
        Preview::Code => (10., 400., theme::muted(), false, true),
        Preview::Callout => (12., 400., theme::fg(), false, false),
        Preview::Table => (16., 400., theme::muted(), false, false),
        Preview::Div => (12., 400., theme::fg(), false, false),
        Preview::Link => (12., 400., theme::fg(), false, false),
        Preview::Rel => (14., 400., theme::accent(), false, false),
    };
    let mut tile = div()
        .flex_none()
        .w(w)
        .h(h)
        .flex()
        .items_center()
        .justify_center()
        .border_1()
        .border_color(theme::border())
        .rounded(px(6.))
        .bg(if cls == Preview::Callout {
            theme::hover()
        } else {
            bg
        })
        .overflow_hidden()
        .text_size(px(size))
        .font_weight(FontWeight(weight))
        .text_color(color);
    if italic {
        tile = tile.italic().border_l(px(3.)).border_color(theme::accent());
    }
    if mono {
        tile = tile.font_family(theme::fonts::MONO);
    }
    tile.child(text)
}
