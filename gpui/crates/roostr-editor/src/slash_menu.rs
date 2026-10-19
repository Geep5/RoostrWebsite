//! Anytype's add-menu (`SlashMenu.svelte`): opens at the caret when "/" is
//! typed, sectioned items with a rendered style preview + description,
//! filtered live by the text typed after the slash (name + aliases). The
//! editor keeps keyboard focus (typing goes on into the block) and owns
//! navigation keys.

use gpui_kit::prelude::FluentBuilder as _;
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

    /// The open slash menu (300px, at the caret; flips above when there's no room).
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
        let filtered = self.slash_entries(&slash.filter);
        let viewport = window.viewport_size();
        let w = px(300.);
        let h = px((12. + filtered.len() as f32 * 46.).min(360.));
        let left = anchor.x.min(viewport.width - w - px(12.));
        let top = if anchor.y + h > viewport.height - px(12.) {
            (anchor.y - h - px(28.)).max(px(12.))
        } else {
            anchor.y
        };
        let mut list = div()
            .id("slash-menu")
            .w(w)
            .max_h(px(360.))
            .overflow_y_scroll()
            .bg(theme::panel())
            .border_1()
            .border_color(theme::border())
            .rounded(px(10.))
            .p(px(6.))
            .shadow(roostr_core::ui::menu_shadow())
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation());
        if filtered.is_empty() {
            list = list.child(
                div()
                    .p(px(12.))
                    .text_size(px(12.))
                    .text_color(theme::muted())
                    .child("No matches"),
            );
        }
        let mut last_section = "";
        for (i, e) in filtered.into_iter().enumerate() {
            if e.section != last_section {
                last_section = e.section;
                list = list.child(section_label(e.section));
            }
            let selected = i == slash.sel;
            let pick = e.pick.clone();
            list = list.child(
                div()
                    .id(("slash-item", i))
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .w_full()
                    .px(px(8.))
                    .py(px(5.))
                    .rounded(px(8.))
                    .cursor_pointer()
                    .when(selected, |d| d.bg(theme::hover()))
                    .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                        if *hovered && let Some(s) = this.slash.as_mut() {
                            s.sel = i;
                            cx.notify();
                        }
                    }))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.apply_slash(pick.clone(), window, cx)
                    }))
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
                                    .child(e.label),
                            )
                            .child(
                                div()
                                    .text_size(px(11.))
                                    .text_color(theme::muted())
                                    .child(e.desc),
                            ),
                    ),
            );
        }
        Some(
            deferred(anchored().position(point(left, top)).child(list))
                .with_priority(2)
                .into_any_element(),
        )
    }
}

/// `.section` header: 10px uppercase, letter-spaced, muted.
pub(crate) fn section_label(text: &'static str) -> Div {
    div()
        .text_size(px(10.))
        .text_color(theme::muted())
        .px(px(8.))
        .pt(px(8.))
        .pb(px(4.))
        .child(text.to_uppercase())
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
