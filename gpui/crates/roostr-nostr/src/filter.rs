//! NIP-01 subscription filters, serialized as relays expect them
//! (`{"ids":…,"authors":…,"kinds":…,"#h":…,"since":…,"until":…,"limit":…}`)
//! and matched against events the way nostr-tools' `matchFilter` does.

use std::collections::BTreeMap;

use serde::de::Error as _;
use serde::ser::SerializeMap;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value;

use crate::event::Event;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Filter {
    pub ids: Option<Vec<String>>,
    pub authors: Option<Vec<String>>,
    pub kinds: Option<Vec<u32>>,
    /// Single-letter tag queries: `'h'` → `"#h"`.
    pub tags: BTreeMap<char, Vec<String>>,
    pub since: Option<u64>,
    pub until: Option<u64>,
    pub limit: Option<u64>,
}

impl Filter {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn ids<S: Into<String>>(mut self, ids: impl IntoIterator<Item = S>) -> Self {
        self.ids = Some(ids.into_iter().map(Into::into).collect());
        self
    }

    pub fn authors<S: Into<String>>(mut self, authors: impl IntoIterator<Item = S>) -> Self {
        self.authors = Some(authors.into_iter().map(Into::into).collect());
        self
    }

    pub fn kinds(mut self, kinds: impl IntoIterator<Item = u32>) -> Self {
        self.kinds = Some(kinds.into_iter().collect());
        self
    }

    /// `#<letter>`: events carrying a `letter` tag whose first value is one
    /// of `values`.
    pub fn tag<S: Into<String>>(
        mut self,
        letter: char,
        values: impl IntoIterator<Item = S>,
    ) -> Self {
        self.tags
            .insert(letter, values.into_iter().map(Into::into).collect());
        self
    }

    pub fn since(mut self, since: u64) -> Self {
        self.since = Some(since);
        self
    }

    pub fn until(mut self, until: u64) -> Self {
        self.until = Some(until);
        self
    }

    pub fn limit(mut self, limit: u64) -> Self {
        self.limit = Some(limit);
        self
    }

    /// Whether `event` satisfies every condition (`limit` aside).
    pub fn matches(&self, event: &Event) -> bool {
        if self
            .ids
            .as_ref()
            .is_some_and(|ids| !ids.contains(&event.id))
        {
            return false;
        }
        if self
            .authors
            .as_ref()
            .is_some_and(|a| !a.contains(&event.pubkey))
        {
            return false;
        }
        if self
            .kinds
            .as_ref()
            .is_some_and(|k| !k.contains(&event.kind))
        {
            return false;
        }
        if self.since.is_some_and(|s| event.created_at < s)
            || self.until.is_some_and(|u| event.created_at > u)
        {
            return false;
        }
        self.tags.iter().all(|(letter, values)| {
            let mut name = [0u8; 4];
            let name = letter.encode_utf8(&mut name);
            event
                .tag_values(name)
                .any(|v| values.iter().any(|want| want == v))
        })
    }
}

impl Serialize for Filter {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(None)?;
        if let Some(ids) = &self.ids {
            map.serialize_entry("ids", ids)?;
        }
        if let Some(authors) = &self.authors {
            map.serialize_entry("authors", authors)?;
        }
        if let Some(kinds) = &self.kinds {
            map.serialize_entry("kinds", kinds)?;
        }
        for (letter, values) in &self.tags {
            map.serialize_entry(&format!("#{letter}"), values)?;
        }
        if let Some(since) = self.since {
            map.serialize_entry("since", &since)?;
        }
        if let Some(until) = self.until {
            map.serialize_entry("until", &until)?;
        }
        if let Some(limit) = self.limit {
            map.serialize_entry("limit", &limit)?;
        }
        map.end()
    }
}

impl<'de> Deserialize<'de> for Filter {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        fn field<T: serde::de::DeserializeOwned, E: serde::de::Error>(
            value: Value,
            name: &str,
        ) -> Result<T, E> {
            serde_json::from_value(value).map_err(|e| E::custom(format!("filter {name}: {e}")))
        }
        let Value::Object(object) = Value::deserialize(deserializer)? else {
            return Err(D::Error::custom("a filter is a JSON object"));
        };
        let mut filter = Filter::default();
        for (key, value) in object {
            match key.as_str() {
                "ids" => filter.ids = Some(field(value, "ids")?),
                "authors" => filter.authors = Some(field(value, "authors")?),
                "kinds" => filter.kinds = Some(field(value, "kinds")?),
                "since" => filter.since = Some(field(value, "since")?),
                "until" => filter.until = Some(field(value, "until")?),
                "limit" => filter.limit = Some(field(value, "limit")?),
                tag => {
                    let mut chars = tag.chars();
                    match (chars.next(), chars.next(), chars.next()) {
                        (Some('#'), Some(letter), None) => {
                            filter.tags.insert(letter, field(value, tag)?);
                        }
                        _ => return Err(D::Error::custom(format!("unknown filter key {tag}"))),
                    }
                }
            }
        }
        Ok(filter)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(kind: u32, created_at: u64, tags: &[&[&str]]) -> Event {
        Event {
            id: "aa".repeat(32),
            pubkey: "bb".repeat(32),
            created_at,
            kind,
            tags: tags
                .iter()
                .map(|t| t.iter().map(|s| s.to_string()).collect())
                .collect(),
            content: String::new(),
            sig: String::new(),
        }
    }

    #[test]
    fn serializes_in_relay_shape() {
        let filter = Filter::new()
            .kinds([1078, 1079])
            .authors(["bb".repeat(32)])
            .tag('h', ["x"])
            .since(5)
            .limit(128);
        let json = serde_json::to_string(&filter).unwrap();
        assert_eq!(
            json,
            format!(
                r##"{{"authors":["{}"],"kinds":[1078,1079],"#h":["x"],"since":5,"limit":128}}"##,
                "bb".repeat(32)
            )
        );
        assert_eq!(serde_json::from_str::<Filter>(&json).unwrap(), filter);
        assert_eq!(serde_json::to_string(&Filter::new()).unwrap(), "{}");
    }

    #[test]
    fn matches_like_nostr_tools() {
        let e = event(1078, 100, &[&["h", "x"], &["b", "y"]]);
        assert!(Filter::new().matches(&e));
        assert!(
            Filter::new()
                .kinds([1078])
                .tag('h', ["z", "x"])
                .since(100)
                .until(100)
                .matches(&e)
        );
        assert!(!Filter::new().kinds([1079]).matches(&e));
        assert!(!Filter::new().tag('h', ["y"]).matches(&e));
        assert!(!Filter::new().tag('p', ["x"]).matches(&e));
        assert!(!Filter::new().since(101).matches(&e));
        assert!(!Filter::new().until(99).matches(&e));
        assert!(!Filter::new().ids(["cc".repeat(32)]).matches(&e));
        assert!(Filter::new().authors(["bb".repeat(32)]).matches(&e));
    }
}
