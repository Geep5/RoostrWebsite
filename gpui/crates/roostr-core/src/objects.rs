//! Object and type icons and names (`src/lib/icons.ts`, `typeGlyph` in
//! `src/lib/create.ts`): an object shows its own emoji, else its type's.

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kind_names() {
        assert_eq!(kind_name("relation"), "Property");
        assert_eq!(kind_name("vanish_log"), "Vanish log");
        assert_eq!(kind_name(""), "");
    }
}
