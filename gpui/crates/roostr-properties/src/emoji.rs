//! Anytype's smile menu, compact (`EmojiPicker.svelte`, `emoji-data.ts`):
//! searchable emoji grid + random + remove. Any emoji can also be pasted
//! into the search box and picked from the first cell; with images on, a
//! pasted image URL or an uploaded image file becomes the icon.

use std::collections::HashMap;
use std::path::Path;
use std::rc::Rc;
use std::sync::LazyLock;

use base64::Engine as _;
use regex::Regex;
use roostr_core::State;
use roostr_ui::ty::Weights as _;
use roostr_ui::{
    Button, ButtonCustomVariant, Input, InputState, ScrollArea, Theme, disabled, radius, ty,
};
use rune::{Cursor, El, col, el, row, text};
use serde::Deserialize;

use crate::widgets::{FieldSkin, empty_note, error_alert, field, icon_el, later, menu_shadow};

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

/// Indices into `DATA.emoji` of each group, in group order.
static BY_GROUP: LazyLock<Vec<Vec<usize>>> = LazyLock::new(|| {
    let data = &*DATA;
    let mut groups = vec![Vec::new(); data.groups.len()];
    for (i, e) in data.emoji.iter().enumerate() {
        if let Some(g) = groups.get_mut(e.2) {
            g.push(i);
        }
    }
    groups
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

type PickFn = Rc<dyn Fn(&mut State, &str)>;
type CloseFn = Rc<dyn Fn(&mut State)>;

/// Emoji (and optional image) icon picker panel; the host anchors it under the icon.
pub struct EmojiPicker {
    key: String,
    with_image: bool,
    on_pick: Option<PickFn>,
    on_close: Option<CloseFn>,
}

impl EmojiPicker {
    /// A picker keyed by `key` (its search persists while it stays open under that key).
    pub fn new(key: impl Into<String>) -> Self {
        Self {
            key: key.into(),
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
    pub fn on_pick(mut self, f: impl Fn(&mut State, &str) + 'static) -> Self {
        self.on_pick = Some(Rc::new(f));
        self
    }

    /// Escape was pressed in the picker, or a press landed outside it.
    pub fn on_close(mut self, f: impl Fn(&mut State) + 'static) -> Self {
        self.on_close = Some(Rc::new(f));
        self
    }
}

/// One open picker: its search field and upload progress.
struct EmojiState {
    query: InputState,
    upload_error: String,
    uploading: bool,
}

impl EmojiState {
    /// A fresh picker, its search focused.
    fn new() -> Self {
        let mut query = InputState::new();
        query.focus();
        Self {
            query,
            upload_error: String::new(),
            uploading: false,
        }
    }
}

/// Every open picker by key; an entry goes when its picker closes or picks.
#[derive(Default)]
struct Pickers(HashMap<String, EmojiState>);

fn state_mut<'a>(s: &'a mut State, key: &str) -> &'a mut EmojiState {
    s.slot::<Pickers>()
        .0
        .entry(key.to_string())
        .or_insert_with(EmojiState::new)
}

fn is_emoji_query(q: &str) -> bool {
    !q.is_empty() && PICTOGRAPHIC.is_match(q)
}

fn is_url_query(with_image: bool, q: &str) -> bool {
    with_image && IMAGE_URL.is_match(q)
}

/// The emoji a search finds: name, CLDR keywords or Roostr's aliases contain `q` (lowercase).
fn search(q: &str) -> Vec<usize> {
    DATA.emoji
        .iter()
        .enumerate()
        .filter(|(_, (e, name, _, keywords))| {
            name.contains(q)
                || keywords.contains(q)
                || ALIASES
                    .iter()
                    .find(|(a, _)| a == e)
                    .is_some_and(|(_, words)| words.contains(q))
        })
        .map(|(i, _)| i)
        .collect()
}

/// Drops the picker's state and hands `value` to the host.
fn pick(s: &mut State, key: &str, on_pick: &Option<PickFn>, value: &str) {
    s.slot::<Pickers>().0.remove(key);
    if let Some(f) = on_pick {
        f(s, value);
    }
}

fn random(s: &mut State, key: &str, on_pick: &Option<PickFn>) {
    let n = DATA.emoji.len();
    let i = (uuid::Uuid::new_v4().as_u128() % n as u128) as usize;
    let e = DATA.emoji[i].0.clone();
    pick(s, key, on_pick, &e);
}

/// Picks a file through the host's file dialog and turns it into an icon data URL.
fn upload(s: &mut State, key: String, on_pick: Option<PickFn>) {
    let picker = s.pick_file.as_ref().map(|p| p.0.clone());
    let Some(picker) = picker.filter(|_| !roostr_core::io::WEB) else {
        state_mut(s, &key).upload_error = roostr_core::io::NO_FILE_PICKER.to_string();
        return;
    };
    let chosen = picker(s);
    s.rt.spawn(async move |cx| {
        let Some(path) = chosen.await else { return };
        let open = cx.update(|s| match s.slot::<Pickers>().0.get_mut(&key) {
            Some(me) => {
                me.upload_error.clear();
                me.uploading = true;
                true
            }
            None => false,
        });
        if !open {
            return;
        }
        let result = cx.background(async move { icon_from_file(&path) }).await;
        cx.update(|s| {
            let Some(me) = s.slot::<Pickers>().0.get_mut(&key) else {
                return;
            };
            me.uploading = false;
            match result {
                Ok(icon) if icon.len() > MAX_ICON_BYTES => {
                    me.upload_error = format!(
                        "that image is too big for an icon ({} KB after resizing; up to {} KB) - try a simpler image",
                        (icon.len() as f64 / 1000.).round(),
                        MAX_ICON_BYTES / 1000
                    );
                }
                Ok(icon) => pick(s, &key, &on_pick, &icon),
                Err(error) => me.upload_error = error.to_string(),
            }
        });
    })
    .detach();
}

/// A grid cell: 32px square, hover fill, radius sm, named in its tooltip.
/// Off the tab order: the grid is thousands of cells.
fn cell_el(t: &Theme, key: String, child: El) -> El {
    row()
        .key(key)
        .size(32.0, 32.0)
        .center()
        .radius(radius::SM)
        .hover(t.hover)
        .press(t.hover)
        .cursor(Cursor::Pointer)
        .child(child)
}

/// A group's caption: uppercase 10.5px semibold muted on the panel, so it
/// can also cover rows as the sticky header.
fn group_header(t: &Theme, label: &str, first: bool) -> El {
    el().w_full()
        .fill(t.panel)
        .px(2.0)
        .pt(if first { 0.0 } else { 6.0 })
        .pb(3.0)
        .child(
            text(label.to_uppercase())
                .font(ty::ui(10.5).semibold())
                .color(t.muted),
        )
}

impl EmojiPicker {
    pub fn build(self, s: &State, _cx: &mut rune::Cx<State>, t: &Theme, layer: &mut Vec<El>) -> El {
        let key = self.key;
        let fallback;
        let me = match s.get::<Pickers>().and_then(|p| p.0.get(&key)) {
            Some(me) => me,
            None => {
                let open = key.clone();
                later(s, move |s| {
                    state_mut(s, &open);
                });
                fallback = EmojiState::new();
                &fallback
            }
        };
        let with_image = self.with_image;
        let on_pick = self.on_pick;
        let on_close = self.on_close;
        let raw = me.query.value().trim().to_string();
        let q = raw.to_lowercase();
        let url = is_url_query(with_image, &raw);
        let emoji_query = is_emoji_query(&raw);

        let pick_el = |k: String, value: String, child: El, tip: String| {
            let (pk, f) = (key.clone(), on_pick.clone());
            cell_el(t, k, child)
                .help(tip.clone())
                .label(tip)
                .on_press(move |s: &mut State| pick(s, &pk, &f, &value))
        };
        let cells_row = |items: &[usize]| {
            row().gap(2.0).pb(2.0).children(items.iter().map(|&i| {
                let (emoji, name, _, _) = &DATA.emoji[i];
                pick_el(
                    format!("emoji-{i}"),
                    emoji.clone(),
                    roostr_ui::emoji_text(emoji, ty::ui(19.0)),
                    name.clone(),
                )
            }))
        };

        let mut list = col().w_full();
        if url || emoji_query {
            let tip = if url {
                "Use image".to_string()
            } else {
                format!("Use {raw}")
            };
            let cell = if url {
                pick_el(
                    "pasted".into(),
                    raw.clone(),
                    icon_el(s, &raw, 26.0)
                        .radius(radius::SM)
                        .clip_rounded(radius::SM),
                    tip,
                )
            } else {
                pick_el(
                    "pasted".into(),
                    raw.clone(),
                    text(raw.clone()).font(ty::ui(19.0)),
                    tip,
                )
                .border(1.0, t.accent)
                .border_dash(4.0, 3.0)
            };
            list = list.child(row().pb(2.0).child(cell));
        }
        if q.is_empty() || emoji_query {
            for (gi, group) in DATA.groups.iter().enumerate() {
                list = list.child(
                    col()
                        .w_full()
                        .child(group_header(t, group, gi == 0).sticky_top(0.0))
                        .children(BY_GROUP[gi].chunks(PER_ROW).map(&cells_row)),
                );
            }
        } else {
            let items = search(&q);
            if items.is_empty() {
                list = list.child(
                    empty_note(t, "No matches — paste any emoji instead.", 12.0).padding(8.0),
                );
            }
            list = list.children(items.chunks(PER_ROW).map(&cells_row));
        }

        let placeholder = if with_image {
            "Search, paste an emoji or an image URL…"
        } else {
            "Search or paste an emoji…"
        };
        let lens_key = key.clone();
        let input = Input::new("query", &me.query)
            .placeholder(placeholder)
            .on_edit(move |s: &mut State| &mut state_mut(s, &lens_key).query);
        let top_button = |k: &'static str, label: &'static str, title: &'static str| {
            Button::new(k)
                .custom(ButtonCustomVariant::new(t).foreground(t.muted))
                .square(28.0)
                .p(0.0)
                .radius(radius::TAB)
                .text_color(t.muted)
                .pointer()
                .accessibility_label(title)
                .tooltip(title)
                .child(text(label).font(ty::ui(16.0)).color(t.muted))
        };
        let mut top = row()
            .items_center()
            .gap(6.0)
            .padding(8.0)
            .border_sides([0.0, 0.0, 1.0, 0.0], t.border)
            .child(
                field(t, input, me.query.is_focused(), FieldSkin::Emoji)
                    .grow(1.0)
                    .shrink(),
            )
            .child({
                let (k, f) = (key.clone(), on_pick.clone());
                top_button("random", "🎲", "Random")
                    .on_press(move |s: &mut State| random(s, &k, &f))
                    .build(t)
            });
        if with_image {
            let (k, f) = (key.clone(), on_pick.clone());
            top = top.child(
                disabled(
                    top_button(
                        "upload",
                        "⬆️",
                        "Upload an image (PNG, JPG, WebP, GIF or SVG)",
                    ),
                    me.uploading,
                )
                .on_press(move |s: &mut State| {
                    if !state_mut(s, &k).uploading {
                        upload(s, k.clone(), f.clone());
                    }
                })
                .build(t),
            );
        }
        top = top.child({
            let (k, f) = (key.clone(), on_pick.clone());
            top_button("remove", "×", "Remove icon")
                .on_press(move |s: &mut State| pick(s, &k, &f, ""))
                .build(t)
        });

        let mut panel = col()
            .key(key.clone())
            .w(320.0)
            .fill(t.panel)
            .border(1.0, t.border)
            .radius(radius::PANE)
            .box_shadow(menu_shadow())
            .clip_rounded(radius::PANE)
            .child(top);
        if !me.upload_error.is_empty() {
            panel = panel.child(
                el().px(8.0)
                    .py(6.0)
                    .border_sides([0.0, 0.0, 1.0, 0.0], t.border)
                    .child(error_alert(t, "upload-error", me.upload_error.clone())),
            );
        }
        panel = panel.child(
            col().h(300.0).padding(8.0).child(
                ScrollArea::vertical("emoji-list", list)
                    .grow()
                    .w_full()
                    .build(t),
            ),
        );
        let close_key = key.clone();
        roostr_ui::overlay::dismissable(
            panel,
            &key,
            move |s: &mut State| {
                s.slot::<Pickers>().0.remove(&close_key);
                if let Some(f) = &on_close {
                    f(s);
                }
            },
            layer,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aliases_find_emoji() {
        let pin = DATA.emoji.iter().position(|e| e.0 == "📌").expect("📌");
        assert!(search("pin").contains(&pin));
        let bot = DATA.emoji.iter().position(|e| e.0 == "🤖").expect("🤖");
        assert!(search("agent").contains(&bot));
    }

    #[test]
    fn pasted_queries() {
        assert!(is_emoji_query("🦀"));
        assert!(!is_emoji_query("crab"));
        assert!(is_url_query(true, "https://example.com/a.png"));
        assert!(!is_url_query(false, "https://example.com/a.png"));
    }

    #[test]
    fn every_emoji_has_a_group() {
        let n: usize = BY_GROUP.iter().map(Vec::len).sum();
        assert_eq!(n, DATA.emoji.len());
    }
}
