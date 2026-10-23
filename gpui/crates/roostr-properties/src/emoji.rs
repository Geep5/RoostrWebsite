//! Anytype's smile menu, compact (`EmojiPicker.svelte`, `emoji-data.ts`):
//! searchable emoji grid + random + remove. Any emoji can also be pasted
//! into the search box and picked from the first cell; with images on, a
//! pasted image URL or an uploaded image file becomes the icon.

use std::path::Path;
use std::rc::Rc;
use std::sync::LazyLock;

use base64::Engine as _;
use gpui_kit::component::button::{Button, ButtonCustomVariant, ButtonVariants as _};
use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::component::label::Label;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use regex::Regex;
use roostr_core::theme;
use roostr_core::ui::disabled;
use serde::Deserialize;

use crate::widgets::{FieldSkin, empty_note, error_alert, field, menu_shadow};

#[derive(Deserialize)]
struct EmojiData {
    groups: Vec<String>,
    /// [emoji, CLDR name, group index, extra keywords]
    emoji: Vec<(String, String, usize, String)>,
}

/// Generated from Unicode emoji-test.txt + CLDR annotations; regenerate with
/// `cargo run -p roostr-properties --example gen-emoji -- emoji-test.txt annotations.json
/// > crates/roostr-properties/src/emoji_data.json` (see `examples/gen-emoji.rs`).
static DATA: LazyLock<EmojiData> = LazyLock::new(|| {
    serde_json::from_str(include_str!("emoji_data.json")).expect("valid emoji data")
});

/// Extra search words for emoji whose CLDR name misses the obvious query.
const ALIASES: &[(&str, &str)] = &[
    ("📝", "note memo"),
    ("📄", "page"),
    ("📋", "clipboard list"),
    ("📌", "pin"),
    ("📎", "clip attach"),
    ("🍾", "champagne"),
    ("✏️", "edit"),
    ("🗂️", "files organize"),
    ("🗃️", "archive"),
    ("🗄️", "cabinet"),
    ("✅", "done task"),
    ("☑️", "todo"),
    ("🎯", "goal"),
    ("🚀", "launch ship"),
    ("🔥", "hot"),
    ("⭐", "favorite"),
    ("💡", "idea"),
    ("⚡", "zap fast"),
    ("🧠", "mind think"),
    ("💭", "thought"),
    ("🗒️", "notepad"),
    ("📅", "date"),
    ("⏰", "time"),
    ("⌛", "time"),
    ("🏢", "office work"),
    ("🏗️", "build"),
    ("💼", "work business"),
    ("🛠️", "tools build fix"),
    ("⚙️", "settings config"),
    ("🔧", "fix"),
    ("🔨", "build"),
    ("💻", "laptop code"),
    ("🖥️", "desktop"),
    ("⌨️", "type"),
    ("📱", "phone mobile"),
    ("🌐", "web internet"),
    ("🔗", "link"),
    ("🧩", "puzzle"),
    ("🤖", "bot agent ai"),
    ("🎮", "game"),
    ("🕹️", "arcade"),
    ("💰", "money"),
    ("💵", "cash money"),
    ("🪙", "bitcoin crypto"),
    ("📈", "growth stocks up"),
    ("📉", "down"),
    ("📊", "data chart"),
    ("🛒", "shopping groceries"),
    ("🛍️", "shopping"),
    ("🎁", "present"),
    ("❤️", "love"),
    ("😀", "happy"),
    ("😂", "funny lol"),
    ("😎", "cool"),
    ("🤔", "hmm"),
    ("😴", "tired"),
    ("🥳", "celebrate"),
    ("🤝", "deal"),
    ("👍", "ok approve"),
    ("👀", "look watch"),
    ("💪", "strong gym"),
    ("🙏", "thanks please"),
    ("👋", "hello bye"),
    ("🌲", "nature"),
    ("🍀", "luck"),
    ("☀️", "sunny"),
    ("🌙", "night"),
    ("🪐", "space"),
    ("🌍", "world"),
    ("☄️", "space"),
    ("☕", "cafe"),
    ("🧋", "boba"),
    ("🚗", "drive"),
    ("✈️", "travel flight"),
    ("🚲", "bike"),
    ("⛵", "boat"),
    ("🗺️", "travel"),
    ("🧳", "trip"),
    ("🎵", "song"),
    ("🎨", "art design"),
    ("📷", "photo"),
    ("🎬", "movie film"),
    ("⚽", "football"),
    ("🧘", "yoga zen meditate"),
    ("🏃", "exercise"),
    ("🏋️", "gym weights"),
    ("🔒", "secure private"),
    ("🔑", "access"),
    ("🛡️", "security"),
    ("⚠️", "caution"),
    ("❗", "important"),
    ("🧪", "experiment science"),
    ("🔬", "research"),
    ("🧬", "biology"),
    ("💊", "medicine health"),
    ("🩺", "doctor health"),
];

static PICTOGRAPHIC: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\p{Extended_Pictographic}").expect("valid"));
static IMAGE_URL: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^https?://\S+$").expect("valid"));

/// Raster icons are scaled to fit this many pixels.
const ICON_PX: u32 = 256;
/// An icon (as a data URL) may not be larger than this.
const MAX_ICON_BYTES: usize = 200_000;
/// Cells per grid row: 320px wide, 8px padding, 32px cells with 2px gaps.
const PER_ROW: usize = 9;

type PickFn = Rc<dyn Fn(&str, &mut Window, &mut App)>;
type CloseFn = Rc<dyn Fn(&mut Window, &mut App)>;

/// Emoji (and optional image) icon picker body; host puts it in a Popover.
#[derive(IntoElement)]
pub struct EmojiPicker {
    id: ElementId,
    with_image: bool,
    on_pick: Option<PickFn>,
    on_close: Option<CloseFn>,
}

impl EmojiPicker {
    /// A picker keyed by `id` (its search and scroll persist while rendered under that id).
    pub fn new(id: impl Into<ElementId>) -> Self {
        Self {
            id: id.into(),
            with_image: false,
            on_pick: None,
            on_close: None,
        }
    }

    /// Also accept an image: a pasted image URL, or an uploaded image file (Anytype's Upload tab).
    pub fn with_image(mut self, with_image: bool) -> Self {
        self.with_image = with_image;
        self
    }

    /// Emoji text, or an image URL / data URI when with_image; "" removes the icon.
    pub fn on_pick(mut self, f: impl Fn(&str, &mut Window, &mut App) + 'static) -> Self {
        self.on_pick = Some(Rc::new(f));
        self
    }

    /// Escape was pressed in the picker.
    pub fn on_close(mut self, f: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.on_close = Some(Rc::new(f));
        self
    }
}

impl RenderOnce for EmojiPicker {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let state = window.use_keyed_state(self.id.clone(), cx, |window, cx| {
            EmojiState::new(window, cx)
        });
        state.update(cx, |s, _| {
            s.with_image = self.with_image;
            s.on_pick = self.on_pick.clone();
            s.on_close = self.on_close.clone();
        });
        state
    }
}

enum Row {
    Header(usize),
    /// Indices into `DATA.emoji`.
    Cells(Vec<usize>),
    /// The pasted emoji / image URL as the first cell.
    Pasted,
    None,
}

struct EmojiState {
    query: Entity<InputState>,
    with_image: bool,
    on_pick: Option<PickFn>,
    on_close: Option<CloseFn>,
    rows: Vec<Row>,
    /// Row index of each group header (browse mode only).
    headers: Vec<(usize, usize)>,
    list: ListState,
    upload_error: String,
    uploading: bool,
    _sub: Subscription,
    placeholder: &'static str,
}

impl EmojiState {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let query = cx.new(|cx| InputState::new(window, cx));
        let sub = cx.subscribe(&query, |this: &mut Self, _, ev: &InputEvent, cx| {
            if matches!(ev, InputEvent::Change) {
                this.rebuild(cx);
                cx.notify();
            }
        });
        let focus = query.clone();
        window.defer(cx, move |window, cx| {
            focus.update(cx, |s, cx| s.focus(window, cx))
        });
        let mut this = Self {
            query,
            with_image: false,
            on_pick: None,
            on_close: None,
            rows: Vec::new(),
            headers: Vec::new(),
            list: ListState::new(0, ListAlignment::Top, px(200.)),
            upload_error: String::new(),
            uploading: false,
            _sub: sub,
            placeholder: "",
        };
        this.rebuild(cx);
        this
    }

    fn q(&self, cx: &App) -> String {
        self.query.read(cx).value().trim().to_string()
    }

    fn is_emoji_query(q: &str) -> bool {
        !q.is_empty() && PICTOGRAPHIC.is_match(q)
    }

    fn is_url_query(&self, q: &str) -> bool {
        self.with_image && IMAGE_URL.is_match(q)
    }

    fn rebuild(&mut self, cx: &App) {
        let raw = self.q(cx);
        let q = raw.to_lowercase();
        let data = &*DATA;
        let mut rows = Vec::new();
        let mut headers = Vec::new();
        let pasted = self.is_url_query(&raw) || Self::is_emoji_query(&raw);
        if pasted {
            rows.push(Row::Pasted);
        }
        if q.is_empty() || Self::is_emoji_query(&raw) {
            for (gi, _) in data.groups.iter().enumerate() {
                headers.push((rows.len(), gi));
                rows.push(Row::Header(gi));
                let items: Vec<usize> = data
                    .emoji
                    .iter()
                    .enumerate()
                    .filter(|(_, e)| e.2 == gi)
                    .map(|(i, _)| i)
                    .collect();
                rows.extend(items.chunks(PER_ROW).map(|c| Row::Cells(c.to_vec())));
            }
        } else {
            let items: Vec<usize> = data
                .emoji
                .iter()
                .enumerate()
                .filter(|(_, (e, name, _, keywords))| {
                    name.contains(&q)
                        || keywords.contains(&q)
                        || ALIASES
                            .iter()
                            .find(|(a, _)| a == e)
                            .is_some_and(|(_, words)| words.contains(&q))
                })
                .map(|(i, _)| i)
                .collect();
            if items.is_empty() {
                rows.push(Row::None);
            }
            rows.extend(items.chunks(PER_ROW).map(|c| Row::Cells(c.to_vec())));
        }
        self.rows = rows;
        self.headers = headers;
        self.list.reset(self.rows.len());
    }
}

impl EmojiState {
    fn pick(&self, value: &str, window: &mut Window, cx: &mut App) {
        if let Some(f) = &self.on_pick {
            f(value, window, cx);
        }
    }

    fn random(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let n = DATA.emoji.len();
        let i = (uuid::Uuid::new_v4().as_u128() % n as u128) as usize;
        let e = DATA.emoji[i].0.clone();
        self.pick(&e, window, cx);
    }

    fn upload(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if roostr_core::io::WEB {
            self.upload_error = roostr_core::io::NO_FILE_PICKER.to_string();
            cx.notify();
            return;
        }
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Choose".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = paths.await else { return };
            let Some(path) = paths.into_iter().next() else { return };
            let _ = this.update(cx, |this, cx| {
                this.upload_error.clear();
                this.uploading = true;
                cx.notify();
            });
            let result = cx.background_spawn(async move { icon_from_file(&path) }).await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.uploading = false;
                match result {
                    Ok(icon) if icon.len() > MAX_ICON_BYTES => {
                        this.upload_error = format!(
                            "that image is too big for an icon ({} KB after resizing; up to {} KB) - try a simpler image",
                            (icon.len() as f64 / 1000.).round(),
                            MAX_ICON_BYTES / 1000
                        );
                    }
                    Ok(icon) => this.pick(&icon, window, cx),
                    Err(error) => this.upload_error = error.to_string(),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// One emoji: a kit ghost [`Button`] (32px, 19px glyph, named in its
    /// tooltip). Off the tab order: the grid is thousands of cells.
    fn cell(&self, i: usize, cx: &mut Context<Self>) -> AnyElement {
        let (emoji, name, _, _) = &DATA.emoji[i];
        let value = emoji.clone();
        cell_button(("emoji", i), cx)
            .accessibility_label(name.clone())
            .tooltip(name.clone())
            .on_click(cx.listener(move |this, _, window, cx| this.pick(&value, window, cx)))
            .child(div().text_size(px(19.)).child(emoji.clone()))
            .into_any_element()
    }

    fn render_row(&self, ix: usize, cx: &mut Context<Self>) -> AnyElement {
        let Some(row) = self.rows.get(ix) else {
            return div().into_any_element();
        };
        match row {
            Row::Header(gi) => group_header(&DATA.groups[*gi], ix == 0).into_any_element(),
            Row::Cells(cells) => div()
                .flex()
                .gap(px(2.))
                .pb(px(2.))
                .children(cells.iter().map(|&i| self.cell(i, cx)))
                .into_any_element(),
            Row::None => empty_note("No matches — paste any emoji instead.")
                .p(px(8.))
                .text_size(px(12.))
                .into_any_element(),
            Row::Pasted => {
                let q = self.q(cx);
                let value = q.clone();
                let url = self.is_url_query(&q);
                let tip = if url {
                    "Use image".to_string()
                } else {
                    format!("Use {q}")
                };
                div()
                    .pb(px(2.))
                    .child(
                        cell_button("pasted", cx)
                            .accessibility_label(tip.clone())
                            .tooltip(tip)
                            .when(!url, |b| {
                                b.border_1()
                                    .border_dashed()
                                    .border_color(theme::accent())
                                    .child(div().text_size(px(19.)).child(q.clone()))
                            })
                            .when(url, |b| {
                                b.child(
                                    img(q.to_string())
                                        .size(px(26.))
                                        .rounded(theme::radius::sm())
                                        .object_fit(ObjectFit::Cover),
                                )
                            })
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.pick(&value, window, cx)
                            })),
                    )
                    .into_any_element()
            }
        }
    }
}

/// A grid cell: kit [`Button`] (hover fill), 32px square.
fn cell_button(id: impl Into<ElementId>, cx: &App) -> Button {
    Button::new(id)
        .custom(
            ButtonCustomVariant::new(cx)
                .foreground(theme::fg())
                .hover(theme::hover())
                .active(theme::hover()),
        )
        .tab_stop(false)
        .size(px(32.))
        .p_0()
        .rounded(theme::radius::sm())
        .cursor_pointer()
}

/// A group's caption (kit [`Label`]): uppercase 10.5px semibold muted on
/// the panel, so it can also cover rows as the sticky header.
fn group_header(label: &str, first: bool) -> Label {
    Label::new(label.to_uppercase())
        .w_full()
        .bg(theme::panel())
        .text_color(theme::muted())
        .text_size(px(10.5))
        .font_weight(FontWeight::SEMIBOLD)
        .px(px(2.))
        .pt(px(if first { 0. } else { 6. }))
        .pb(px(3.))
}

impl Render for EmojiState {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let placeholder = if self.with_image {
            "Search, paste an emoji or an image URL…"
        } else {
            "Search or paste an emoji…"
        };
        if self.placeholder != placeholder {
            self.placeholder = placeholder;
            self.query
                .update(cx, |s, cx| s.set_placeholder(placeholder, window, cx));
        }
        let this = cx.entity().downgrade();
        let list = list(self.list.clone(), move |ix, _, cx| {
            this.upgrade().map_or_else(
                || div().into_any_element(),
                |e| e.update(cx, |s, cx| s.render_row(ix, cx)),
            )
        })
        .size_full();
        // The sticky group header: the group the top row belongs to.
        let top = self.list.logical_scroll_top();
        let sticky = (top.item_ix > 0 || top.offset_in_item > px(0.))
            .then(|| {
                self.headers
                    .iter()
                    .rev()
                    .find(|(row, _)| *row <= top.item_ix)
                    .map(|(_, g)| *g)
            })
            .flatten();
        let top_variant = ButtonCustomVariant::new(cx).foreground(theme::muted());
        let top_button = |id: &'static str, label: &'static str, title: &'static str| {
            Button::new(id)
                .custom(top_variant)
                .flex_none()
                .size(px(28.))
                .p_0()
                .rounded(theme::radius::tab())
                .text_color(theme::muted())
                .cursor_pointer()
                .accessibility_label(title)
                .tooltip(title)
                .child(div().text_size(px(16.)).child(label))
        };
        div()
            .id("emoji-picker")
            .w(px(320.))
            .flex()
            .flex_col()
            .bg(theme::panel())
            .border_1()
            .border_color(theme::border())
            .rounded(theme::radius::pane())
            .shadow(menu_shadow())
            .overflow_hidden()
            .text_color(theme::fg())
            .on_key_down(cx.listener(|this, ev: &KeyDownEvent, window, cx| {
                if ev.keystroke.key == "escape"
                    && let Some(f) = this.on_close.clone()
                {
                    cx.stop_propagation();
                    f(window, cx);
                }
            }))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .p(px(8.))
                    .border_b_1()
                    .border_color(theme::border())
                    .child(field(&self.query, FieldSkin::Emoji, window, cx).flex_1())
                    .child(
                        top_button("random", "🎲", "Random")
                            .on_click(cx.listener(|this, _, window, cx| this.random(window, cx))),
                    )
                    .when(self.with_image, |d| {
                        d.child(
                            disabled(
                                top_button(
                                    "upload",
                                    "⬆️",
                                    "Upload an image (PNG, JPG, WebP, GIF or SVG)",
                                ),
                                self.uploading,
                            )
                            .on_click(cx.listener(
                                |this, _, window, cx| {
                                    if !this.uploading {
                                        this.upload(window, cx)
                                    }
                                },
                            )),
                        )
                    })
                    .child(
                        top_button("remove", "×", "Remove icon")
                            .on_click(cx.listener(|this, _, window, cx| this.pick("", window, cx))),
                    ),
            )
            .when(!self.upload_error.is_empty(), |d| {
                d.child(
                    div()
                        .px(px(8.))
                        .py(px(6.))
                        .border_b_1()
                        .border_color(theme::border())
                        .child(error_alert("upload-error", self.upload_error.clone())),
                )
            })
            .child(
                div()
                    .relative()
                    .h(px(300.))
                    .p(px(8.))
                    .child(list)
                    .when_some(sticky, |d, g| {
                        d.child(
                            div()
                                .absolute()
                                .top(px(0.))
                                .left(px(8.))
                                .right(px(8.))
                                .child(group_header(&DATA.groups[g], false)),
                        )
                    }),
            )
    }
}

/// An uploaded file as an icon data URL: an SVG as drawn, a raster image
/// scaled to fit 256px as PNG (`iconFromFile`).
fn icon_from_file(path: &Path) -> anyhow::Result<String> {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    let bytes = std::fs::read(path).map_err(|e| anyhow::anyhow!("could not read the file: {e}"))?;
    let b64 = |b: &[u8]| base64::engine::general_purpose::STANDARD.encode(b);
    if name.ends_with(".svg") {
        let svg = String::from_utf8_lossy(&bytes);
        if !Regex::new(r"(?i)<svg[\s>]").expect("valid").is_match(&svg) {
            anyhow::bail!("that file isn't an SVG image");
        }
        return Ok(format!("data:image/svg+xml;base64,{}", b64(svg.as_bytes())));
    }
    let Ok(image) = image::load_from_memory(&bytes) else {
        anyhow::bail!("choose an image file (PNG, JPG, WebP, GIF or SVG)");
    };
    let (w, h) = (image.width(), image.height());
    let scale = (ICON_PX as f64 / w.max(h) as f64).min(1.);
    let (nw, nh) = (
        ((w as f64 * scale).round() as u32).max(1),
        ((h as f64 * scale).round() as u32).max(1),
    );
    let resized = if scale < 1. {
        image.resize_exact(nw, nh, image::imageops::FilterType::Lanczos3)
    } else {
        image
    };
    let mut out = std::io::Cursor::new(Vec::new());
    resized.write_to(&mut out, image::ImageFormat::Png)?;
    Ok(format!("data:image/png;base64,{}", b64(out.get_ref())))
}
