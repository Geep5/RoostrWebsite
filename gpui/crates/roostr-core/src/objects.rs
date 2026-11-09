//! Object and type icons and names (`src/lib/icons.ts`, `typeGlyph` in
//! `src/lib/create.ts`): an object shows its own emoji, else its type's.

use std::collections::HashMap;
use std::sync::{Arc, LazyLock};

use base64::Engine as _;
use parking_lot::Mutex;

use crate::store::Store;

/// Built-in glyph per type key (Anytype bundle equivalents as emoji).
pub const TYPE_GLYPHS: &[(&str, &str)] = &[
    ("page", "📄"),
    ("note", "📝"),
    ("task", "✅"),
    ("person", "👤"),
    ("project", "🔨"),
    ("bookmark", "🔖"),
    ("query", "🔍"),
    ("set", "🔍"),
    ("collection", "🗂️"),
    ("peer", "◉"),
    // An agent with no emoji of its own shows the robot, like the Agent type.
    ("agent", "🤖"),
    ("channel", "◍"),
    ("chat", "💬"),
    ("credential", "🔑"),
];

/// The built-in glyph for `type_key`, "" when it has none.
pub fn builtin_glyph(type_key: &str) -> &'static str {
    TYPE_GLYPHS
        .iter()
        .find(|(k, _)| *k == type_key)
        .map_or("", |(_, g)| g)
}

/// The icon a type shows (`typeIcon`): its type object's emoji (the one of
/// `space` wins over other spaces' copies), else the built-in glyph, else "".
pub fn type_icon(store: &Store, type_key: &str, space: &str) -> String {
    let mut first = None;
    for def in store
        .types
        .iter()
        .filter(|t| t.key == type_key && !t.icon.is_empty())
    {
        if def.space == space {
            return def.icon.clone();
        }
        first.get_or_insert(def);
    }
    match first {
        Some(def) => def.icon.clone(),
        None => builtin_glyph(type_key).to_string(),
    }
}

/// `typeGlyph`: the type icon, else `▨`.
pub fn type_glyph(store: &Store, type_key: &str, space: &str) -> String {
    let icon = type_icon(store, type_key, space);
    if icon.is_empty() { "▨".into() } else { icon }
}

/// `objectIcon`: the object's own emoji/image, else its type's icon, else `•`.
pub fn object_icon(store: &Store, icon: &str, type_key: &str, space: &str) -> String {
    if !icon.is_empty() {
        return icon.to_string();
    }
    let icon = type_icon(store, type_key, space);
    if icon.is_empty() { "•".into() } else { icon }
}

/// The emoji a property shows beside its name: its own, else - for an object
/// property - the icon of what it links: the type it is limited to, or, with
/// no limit, the first object it links (`linked`). None: the format's glyph.
pub fn relation_emoji(
    store: &Store,
    rel: &crate::RelationDef,
    linked: Option<&str>,
    space: &str,
) -> Option<String> {
    if let Some(own) = rel.icon_emoji.as_deref().filter(|e| !e.is_empty()) {
        return Some(own.to_string());
    }
    if rel.format != "object" {
        return None;
    }
    let limited = rel
        .object_types
        .iter()
        .flatten()
        .find_map(|id| store.types.iter().find(|t| &t.id == id))
        .map(|t| type_icon(store, &t.key, space));
    let icon = limited.or_else(|| {
        let s = store.summary(linked?)?;
        Some(object_icon(store, &s.icon, &s.type_key, space))
    })?;
    (!icon.is_empty() && icon != "•" && !icon.starts_with("data:") && !icon.contains("://"))
        .then_some(icon)
}

/// A type key as a name, for kinds with no type object (`kindName`):
/// "relation" is "Property", "channel" is "Space", "vanish_log" is "Vanish log".
pub fn kind_name(type_key: &str) -> String {
    match type_key {
        "relation" => "Property".into(),
        "channel" => "Space".into(),
        _ => {
            let mut chars = type_key.chars();
            let head: String = chars
                .next()
                .map(|c| c.to_uppercase().collect())
                .unwrap_or_default();
            format!("{head}{}", chars.as_str()).replace('_', " ")
        }
    }
}

/// An icon that is an image (a web address or a `data:image/` URL) rather than an emoji.
pub fn is_image_icon(icon: &str) -> bool {
    icon.starts_with("http://") || icon.starts_with("https://") || icon.starts_with("data:image/")
}

/// The image formats a `data:` icon or avatar may carry.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ImageFormat {
    Png,
    Jpeg,
    Webp,
    Gif,
    Svg,
    Bmp,
    Tiff,
    Ico,
}

/// A `data:image/…` URL's image: its format and encoded bytes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DataImage {
    pub format: ImageFormat,
    pub bytes: Vec<u8>,
}

static DATA_IMAGES: LazyLock<Mutex<HashMap<String, Option<Arc<DataImage>>>>> =
    LazyLock::new(Default::default);

/// A `data:image/…` URL (base64 or percent-encoded) decoded; cached by URL.
/// `None` for anything else, or an image format no renderer takes.
pub fn data_url_image(uri: &str) -> Option<Arc<DataImage>> {
    if let Some(hit) = DATA_IMAGES.lock().get(uri) {
        return hit.clone();
    }
    let decoded = (|| {
        let (meta, data) = uri.strip_prefix("data:")?.split_once(',')?;
        let format = match meta.split(';').next()? {
            "image/png" => ImageFormat::Png,
            "image/jpeg" | "image/jpg" => ImageFormat::Jpeg,
            "image/webp" => ImageFormat::Webp,
            "image/gif" => ImageFormat::Gif,
            "image/svg+xml" => ImageFormat::Svg,
            "image/bmp" => ImageFormat::Bmp,
            "image/tiff" => ImageFormat::Tiff,
            "image/x-icon" | "image/vnd.microsoft.icon" => ImageFormat::Ico,
            _ => return None,
        };
        let bytes = if meta.ends_with(";base64") {
            base64::engine::general_purpose::STANDARD
                .decode(data.trim())
                .ok()?
        } else {
            percent_encoding::percent_decode_str(data).collect()
        };
        Some(Arc::new(DataImage { format, bytes }))
    })();
    DATA_IMAGES.lock().insert(uri.to_string(), decoded.clone());
    decoded
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kind_names() {
        assert_eq!(kind_name("relation"), "Property");
        assert_eq!(kind_name("vanish_log"), "Vanish log");
        assert_eq!(kind_name(""), "");
    }

    #[test]
    fn data_urls_decode_base64_and_percent_encoded_images() {
        let png = data_url_image("data:image/png;base64,iVBORw0K").unwrap();
        assert_eq!(png.format, ImageFormat::Png);
        assert_eq!(png.bytes, b"\x89PNG\r\n"[..6].to_vec());
        let svg = data_url_image("data:image/svg+xml,%3Csvg%2F%3E").unwrap();
        assert_eq!(svg.format, ImageFormat::Svg);
        assert_eq!(svg.bytes, b"<svg/>");
        assert!(data_url_image("data:text/plain,hi").is_none());
        assert!(data_url_image("https://example.com/a.png").is_none());
    }
}
