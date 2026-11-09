//! Object and property icons: emoji or image icons (`roostr_core::ui::emoji_or_image_icon`,
//! `icon_text`, `data_url_image`), and the property-format line icons (`prop_icon`,
//! `prop_line_icon`).
//!
//! Images load through [`Images`], which an app keeps in its state (made in `AppSpec::setup`
//! with Rune's waker): `data:image/…` URLs decode here; web addresses load through the
//! browser on the web, and on the desktop through bytes the app fetches itself (Rune has no
//! HTTP client): drain [`Images::take_wanted`] in the app's poll, fetch each in the background
//! and hand the bytes to [`Images::insert`].

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::Arc;

use base64::Engine as _;
use rune::{El, Image, ImageCache, SurfaceFit, TextStyle, Waker, el, row, text};

use crate::icons::Glyph;
use crate::theme::{radius, ty};

/// The line icon for a relation format (`PropIcon` `line`).
pub fn prop_icon(format: &str) -> Glyph {
    match format {
        "checkbox" => Glyph::PropCheckbox,
        "date" => Glyph::PropDate,
        "number" => Glyph::PropNumber,
        "status" | "select" => Glyph::PropStatus,
        "tag" => Glyph::PropTag,
        "url" => Glyph::PropUrl,
        "email" => Glyph::PropEmail,
        "phone" => Glyph::PropPhone,
        "object" => Glyph::PropObject,
        "file" => Glyph::PropFile,
        _ => Glyph::PropText,
    }
}

/// A property row's leading icon (`PropertiesPane` `lineIcon`): well-known keys first, then the
/// format. (Credential secret fields: callers pass `Glyph::PropKey`.)
pub fn prop_line_icon(key: &str, format: &str) -> Glyph {
    match key {
        "served_by" => Glyph::PropComputer,
        "repo_path" => Glyph::PropFolder,
        "chat_color" => Glyph::PropColor,
        "credentials" | "session_cookie" | "key" => Glyph::PropKey,
        "agent" | "account" => Glyph::PropPerson,
        "skills" => Glyph::PropSkill,
        "tools" => Glyph::PropTool,
        "check_first" => Glyph::PropSearch,
        "prompt" => Glyph::PropBrain,
        "model" => Glyph::PropCpu,
        "error" => Glyph::PropWarning,
        "capability" | "service" => Glyph::PropBox,
        "session_host" => Glyph::PropGlobe,
        "checked_at" => Glyph::PropClock,
        _ => prop_icon(format),
    }
}

/// An icon that is an image (a web address or a `data:image/` URL) rather than an emoji.
pub fn is_image_icon(icon: &str) -> bool {
    icon.starts_with("http://") || icon.starts_with("https://") || icon.starts_with("data:image/")
}

/// The MIME type and bytes of a `data:` URL (base64 or percent-encoded); `None` when malformed.
pub fn data_url_bytes(uri: &str) -> Option<(String, Vec<u8>)> {
    let (meta, data) = uri.strip_prefix("data:")?.split_once(',')?;
    let mime = meta.split(';').next()?.to_owned();
    let bytes = if meta.ends_with(";base64") {
        base64::engine::general_purpose::STANDARD
            .decode(data.trim())
            .ok()?
    } else {
        percent_encoding::percent_decode_str(data).collect()
    };
    Some((mime, bytes))
}

/// The image formats a `data:image/…` icon may carry (as GPUI's `ImageFormat`s).
fn supported(mime: &str) -> bool {
    matches!(
        mime,
        "image/png"
            | "image/jpeg"
            | "image/jpg"
            | "image/webp"
            | "image/gif"
            | "image/svg+xml"
            | "image/bmp"
            | "image/tiff"
            | "image/x-icon"
            | "image/vnd.microsoft.icon"
    )
}

/// Images by URI, decoded once and shared (`data_url_image`'s cache and GPUI's image loader).
pub struct Images {
    cache: ImageCache,
    /// The device scale SVG icons rasterize at.
    scale: f32,
    entries: RefCell<HashMap<String, Option<Image>>>,
    wanted: RefCell<Vec<String>>,
}

impl Images {
    /// The cache, waking the app through `waker` (from `AppSpec::setup`) as images decode.
    pub fn new(waker: Waker, scale: f32) -> Images {
        Images {
            cache: ImageCache::new(waker),
            scale,
            entries: RefCell::default(),
            wanted: RefCell::default(),
        }
    }

    /// The image for `uri`, loading it the first time: `None` for a malformed `data:` URL, an
    /// unsupported format, or (desktop) a web address whose bytes haven't been
    /// [`inserted`](Self::insert) yet. A pending image draws nothing until it decodes.
    pub fn image(&self, uri: &str, size: f32) -> Option<Image> {
        if let Some(hit) = self.entries.borrow().get(uri) {
            return hit.clone();
        }
        let loaded = if uri.starts_with("data:") {
            data_url_bytes(uri)
                .filter(|(mime, _)| supported(mime))
                .map(|(mime, bytes)| {
                    self.cache
                        .load_mime(Arc::<[u8]>::from(bytes), &mime, size, self.scale)
                })
        } else if cfg!(target_arch = "wasm32") {
            Some(self.cache.url(uri, size, self.scale))
        } else {
            let mut wanted = self.wanted.borrow_mut();
            if !wanted.iter().any(|w| w == uri) {
                wanted.push(uri.to_owned());
            }
            return None;
        };
        self.entries
            .borrow_mut()
            .insert(uri.to_owned(), loaded.clone());
        loaded
    }

    /// Web addresses views asked for that need fetching (desktop), each once.
    pub fn take_wanted(&self) -> Vec<String> {
        std::mem::take(&mut self.wanted.borrow_mut())
    }

    /// The fetched bytes of `uri` (any format `ImageCache::load` reads).
    pub fn insert(&self, uri: &str, bytes: Vec<u8>) {
        let image = self.cache.load(Arc::<[u8]>::from(bytes));
        self.entries
            .borrow_mut()
            .insert(uri.to_owned(), Some(image));
    }

    /// `uri` could not be fetched: draw nothing for it from now on.
    pub fn failed(&self, uri: &str) {
        self.entries.borrow_mut().insert(uri.to_owned(), None);
    }

    /// An image compiled into the app (`include_bytes!`), decoded once and kept under `key`:
    /// SVG (recognised by its content) rasterized to fit `size` points at the cache's scale,
    /// anything else as [`ImageCache::load`] reads it. A pending image draws nothing.
    pub fn bundled(&self, key: &str, bytes: &'static [u8], size: f32) -> Image {
        if let Some(Some(hit)) = self.entries.borrow().get(key) {
            return hit.clone();
        }
        let head = &bytes[..bytes.len().min(256)];
        let image = if head.windows(4).any(|w| w == b"<svg") {
            self.cache.svg(bytes, size, self.scale)
        } else {
            self.cache.load(bytes)
        };
        self.entries
            .borrow_mut()
            .insert(key.to_owned(), Some(image.clone()));
        image
    }
}

/// An object/space icon: an image for a web address or `data:image/` URL (rounded 4, cover),
/// else the emoji/glyph text at 0.9 × `size` on a `size` line ([`emoji_text`]), centred in a
/// `size` square.
pub fn emoji_or_image_icon(icon: &str, size: f32, images: &Images) -> El {
    if is_image_icon(icon) {
        let frame = el().size(size, size);
        return match images.image(icon, size) {
            Some(image) => frame
                .radius(radius::XS)
                .pixels_fit(image.pixels(), SurfaceFit::Fill),
            None => frame,
        };
    }
    row()
        .size(size, size)
        .center()
        .child(emoji_text(icon, ty::ui(size * 0.9).lh(size)))
}

/// A row icon: an emoji/glyph as text in the row's `style` ([`emoji_text`]), or a `size`
/// square image for a web address or `data:image/` URL.
pub fn icon_text(icon: &str, size: f32, style: TextStyle, images: &Images) -> El {
    if is_image_icon(icon) {
        return emoji_or_image_icon(icon, size, images);
    }
    emoji_text(icon, style)
}

/// `icon` as text in `style`, a colour emoji at the size macOS draws it in text of that size.
///
/// Text in the system font falls back to `.Apple Color Emoji UI`, which CoreText (and so GPUI)
/// draws optically enlarged: 1.25 × the text size up to 16 pt, `12 + size / 2` up to 24 pt,
/// the text size from there; the glyph advances about 1.1 × that plus a point. Rune draws
/// the plain Apple Color Emoji face at 1 em, a fifth smaller than every emoji Roostr showed
/// before, so an emoji icon is drawn here at the enlarged size in a box of its advance and
/// `style`'s line height, its ink centred on the line as CoreText's. Other text, and every
/// icon off macOS (the web's Twemoji is drawn at 1 em in a browser too), is plain `style` text.
pub fn emoji_text(icon: &str, style: TextStyle) -> El {
    if !cfg!(target_os = "macos") || !is_color_emoji(icon) {
        return text(icon.to_owned()).font(style);
    }
    let (ink, advance) = (emoji_ink(style.size), emoji_advance(style.size));
    let line = if style.line_height > 0. {
        style.line_height
    } else {
        style.size * 1.25
    };
    // CoreText's ink starts 0.075 em into the advance; Apple Color Emoji's own at 0.
    row()
        .w(advance)
        .h(line)
        .pl(style.size * 0.075)
        .center_y()
        .child(text(icon.to_owned()).font(TextStyle {
            size: ink,
            line_height: line,
            ..style
        }))
}

/// The ink size of a colour emoji in macOS text of `size` (see [`emoji_text`]).
fn emoji_ink(size: f32) -> f32 {
    (size * 1.25).min(12. + size / 2.).max(size)
}

/// The advance of a colour emoji in macOS text of `size`, whole points as CoreText's.
fn emoji_advance(size: f32) -> f32 {
    (emoji_ink(size) * 1.1 + 1.2).floor()
}

/// Whether `s` is colour emoji only: every character an emoji presentation character, a
/// pictograph outside the text planes, or a part of an emoji sequence (variation selector 16,
/// zero-width joiner, keycap, tag), with at least one that asks for colour. A pictograph with
/// a text form (`⛏`, `☀` without U+FE0F) stays text, as the system font draws it.
pub fn is_color_emoji(s: &str) -> bool {
    use icu_properties::CodePointSetData;
    use icu_properties::props::{EmojiPresentation, ExtendedPictographic};
    let presentation = CodePointSetData::new::<EmojiPresentation>();
    let pictographic = CodePointSetData::new::<ExtendedPictographic>();
    let keycap = s.contains('\u{20E3}');
    let mut colour = false;
    for c in s.chars() {
        let wide = presentation.contains(c)
            || c == '\u{FE0F}'
            || c == '\u{20E3}'
            || (c as u32 >= 0x1F000 && pictographic.contains(c));
        colour |= wide;
        let part = matches!(c, '\u{200D}' | '\u{FE0E}' | '\u{E0020}'..='\u{E007F}')
            || pictographic.contains(c)
            || (keycap && matches!(c, '0'..='9' | '#' | '*'));
        if !wide && !part {
            return false;
        }
    }
    colour
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_and_keys_pick_line_icons() {
        assert_eq!(prop_icon("select"), Glyph::PropStatus);
        assert_eq!(prop_icon("whatever"), Glyph::PropText);
        assert_eq!(prop_line_icon("model", "text"), Glyph::PropCpu);
        assert_eq!(prop_line_icon("x", "date"), Glyph::PropDate);
    }

    #[test]
    fn colour_emoji_are_told_from_text_glyphs() {
        for e in [
            "👑",
            "🔍",
            "✅",
            "✋",
            "🇺🇸",
            "👋🏽",
            "👨\u{200D}👩\u{200D}👧",
            "❤\u{FE0F}",
            "1\u{FE0F}\u{20E3}",
            "🗺",
        ] {
            assert!(is_color_emoji(e), "{e}");
        }
        for t in ["⛏", "☀", "#", "A", "", "👑 Crown", "→"] {
            assert!(!is_color_emoji(t), "{t}");
        }
    }

    /// CoreText's `.Apple Color Emoji UI` sizes, measured with `CTLine` in the system font.
    #[test]
    fn emoji_sizes_follow_coretext() {
        for (size, ink, advance) in [
            (11., 13.75, 16.),
            (12., 15., 17.),
            (13., 16.25, 19.),
            (15., 18.75, 21.),
            (16., 20., 23.),
            (17., 20.5, 23.),
            (20., 22., 25.),
            (24., 24., 27.),
            (30., 30., 34.),
        ] {
            assert_eq!(
                (emoji_ink(size), emoji_advance(size)),
                (ink, advance),
                "{size}"
            );
        }
    }

    #[test]
    fn data_urls_decode_base64_and_percent() {
        let (mime, bytes) = data_url_bytes("data:image/svg+xml;base64,PHN2Zz48L3N2Zz4=").unwrap();
        assert_eq!(
            (mime.as_str(), bytes.as_slice()),
            ("image/svg+xml", b"<svg></svg>".as_slice())
        );
        let (_, bytes) = data_url_bytes("data:image/svg+xml,%3Csvg%3E").unwrap();
        assert_eq!(bytes, b"<svg>");
        assert!(data_url_bytes("nope").is_none());
        assert!(
            is_image_icon("https://x/y.png")
                && is_image_icon("data:image/png;base64,")
                && !is_image_icon("🐓")
        );
    }

    #[test]
    fn desktop_web_icons_wait_for_their_bytes() {
        let images = Images::new(Arc::new(|| {}), 2.0);
        if cfg!(target_arch = "wasm32") {
            return;
        }
        assert!(images.image("https://example.com/a.png", 16.0).is_none());
        assert!(images.image("https://example.com/a.png", 16.0).is_none());
        assert_eq!(images.take_wanted(), ["https://example.com/a.png"]);
        assert!(images.take_wanted().is_empty());
        assert!(images.image("data:image/png;base64,AAAA", 16.0).is_some());
        assert!(images.image("data:text/plain,hi", 16.0).is_none());
    }
}
