//! The identity's kind-0 profile (profile.ts): a small JSON object (`name`,
//! `display_name`, `picture` as a data URI, anything else other clients
//! set), synced through the relays like any metadata event so every device
//! shows the same avatar.

use std::time::Duration;

use serde_json::{Map, Value};

use crate::event::{Event, EventTemplate};
use crate::filter::Filter;
use crate::keys::Keys;
use crate::relay::{Relay, RelayError, publish_any, query_many};
use crate::{kinds, time};

/// A kind-0 content object; fields other clients set are kept.
pub type Profile = Map<String, Value>;

/// The name to show: `display_name`, else `name` (sync.ts join requests).
pub fn display_name(profile: &Profile) -> Option<&str> {
    ["display_name", "name"]
        .into_iter()
        .filter_map(|key| profile.get(key)?.as_str())
        .find(|s| !s.is_empty())
}

/// The newest kind-0 among `events` by `pubkey`, parsed; `None` when there
/// is none (or its content is not a JSON object).
pub fn latest(events: &[Event], pubkey: &str) -> Option<Profile> {
    let newest = events
        .iter()
        .filter(|e| e.kind == kinds::METADATA && e.pubkey == pubkey)
        .max_by_key(|e| e.created_at)?;
    match serde_json::from_str(&newest.content) {
        Ok(Value::Object(profile)) => Some(profile),
        _ => None,
    }
}

/// The relays' latest profile of `pubkey`; `Ok(None)` when they hold none.
pub async fn fetch(
    relays: &[Relay],
    pubkey: &str,
    timeout: Duration,
) -> Result<Option<Profile>, RelayError> {
    let filter = Filter::new().kinds([kinds::METADATA]).authors([pubkey]);
    let events = query_many(relays, &[filter], timeout).await?;
    Ok(latest(&events, pubkey))
}

/// `{...current, ...patch}` with empty values (`null`, `""`) removed, so a
/// patch clears a field by sending it empty.
pub fn merge(current: Profile, patch: Profile) -> Profile {
    let mut next = current;
    next.extend(patch);
    next.retain(|_, v| !(v.is_null() || v.as_str() == Some("")));
    next
}

/// The kind-0 event for `profile`.
pub fn template(profile: &Profile, created_at: u64) -> EventTemplate {
    EventTemplate {
        kind: kinds::METADATA,
        created_at,
        tags: Vec::new(),
        content: Value::Object(profile.clone()).to_string(),
    }
}

/// profile.ts `saveProfile`: merge `patch` over the relays' latest profile
/// (`cached` when no relay answers) and publish it until one relay accepts.
/// Returns what was published.
pub async fn save(
    relays: &[Relay],
    keys: &Keys,
    patch: Profile,
    cached: Profile,
    timeout: Duration,
) -> Result<Profile, RelayError> {
    let current = match fetch(relays, keys.pubkey(), timeout).await {
        Ok(found) => found.unwrap_or_default(),
        Err(_) => cached,
    };
    let next = merge(current, patch);
    let event = keys.sign(template(&next, time::now_s()));
    publish_any(relays, &event, timeout).await?;
    Ok(next)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn object(value: Value) -> Profile {
        match value {
            Value::Object(map) => map,
            _ => unreachable!(),
        }
    }

    #[test]
    fn merge_keeps_other_fields_and_drops_empty_ones() {
        let current = object(json!({"name": "a", "about": "x", "picture": "data:old"}));
        let next = merge(
            current,
            object(json!({"picture": "data:new", "about": "", "nip05": null})),
        );
        assert_eq!(
            Value::Object(next),
            json!({"name": "a", "picture": "data:new"})
        );
    }

    #[test]
    fn latest_picks_the_newest_of_the_author() {
        let keys = Keys::generate();
        let older = keys.sign(template(&object(json!({"name": "old"})), 10));
        let newer = keys.sign(template(
            &object(json!({"name": "new", "display_name": "New"})),
            20,
        ));
        let other = Keys::generate().sign(template(&object(json!({"name": "else"})), 30));
        let profile = latest(&[older, other, newer], keys.pubkey()).unwrap();
        assert_eq!(display_name(&profile), Some("New"));
    }
}
