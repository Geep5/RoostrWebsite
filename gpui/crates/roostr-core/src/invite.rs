//! Space join links (`src/lib/invite.ts`): a public, key-free URL naming the
//! space, its owner and the relays to knock on. Blob is `r2.` +
//! base64url(JSON), no padding.

use base64::Engine as _;
use base64::alphabet::URL_SAFE;
use base64::engine::{DecodePaddingMode, GeneralPurpose, GeneralPurposeConfig};
use serde::Serialize;
use serde_json::Value as Json;

const B64: GeneralPurpose = GeneralPurpose::new(
    &URL_SAFE,
    GeneralPurposeConfig::new()
        .with_encode_padding(false)
        .with_decode_padding_mode(DecodePaddingMode::Indifferent),
);

/// `SpaceJoinLink`; serializes as `{v:2, t:"space-join", space, name?, owner, relays}`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SpaceJoinLink {
    /// Channel (space) object id.
    pub space: String,
    /// Space display name, for the join page.
    pub name: Option<String>,
    /// Owner npub: where the join request is addressed.
    pub owner: String,
    /// Relays where the owner listens.
    pub relays: Vec<String>,
}

#[derive(Serialize)]
struct Wire<'a> {
    v: u8,
    t: &'static str,
    space: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<&'a str>,
    owner: &'a str,
    relays: &'a [String],
}

impl Serialize for SpaceJoinLink {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        Wire {
            v: 2,
            t: "space-join",
            space: &self.space,
            name: self.name.as_deref(),
            owner: &self.owner,
            relays: &self.relays,
        }
        .serialize(s)
    }
}

pub fn encode_join_link(link: &SpaceJoinLink) -> String {
    format!(
        "r2.{}",
        B64.encode(serde_json::to_string(link).expect("serializable"))
    )
}

/// Tolerant decode: whitespace is stripped; anything that isn't a well-formed
/// `r2.` join link is `None` (key-carrying r1 links are retired).
pub fn decode_join_link(blob: &str) -> Option<SpaceJoinLink> {
    let clean: String = blob.chars().filter(|c| !c.is_whitespace()).collect();
    let bytes = B64.decode(clean.strip_prefix("r2.")?).ok()?;
    let json: Json = serde_json::from_str(&String::from_utf8_lossy(&bytes)).ok()?;
    if json.get("t")?.as_str()? != "space-join" {
        return None;
    }
    let nonempty = |k: &str| {
        json.get(k)
            .and_then(Json::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    };
    Some(SpaceJoinLink {
        space: nonempty("space")?,
        name: json.get("name").and_then(Json::as_str).map(str::to_string),
        owner: nonempty("owner")?,
        relays: json
            .get("relays")
            .and_then(Json::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(Json::as_str)
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default(),
    })
}

pub fn join_url(link: &SpaceJoinLink) -> String {
    format!("https://roostr.space/j#{}", encode_join_link(link))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let link = SpaceJoinLink {
            space: "sp-1".into(),
            name: Some("Équipe 🐓".into()),
            owner: "npub1xyz".into(),
            relays: vec!["wss://relay.example".into()],
        };
        let blob = encode_join_link(&link);
        assert!(
            blob.starts_with("r2.")
                && !blob.contains('=')
                && !blob.contains('+')
                && !blob.contains('/')
        );
        let wrapped = format!(" {}\n{} ", &blob[..10], &blob[10..]);
        assert_eq!(decode_join_link(&wrapped), Some(link.clone()));
        let json = serde_json::to_value(&link).unwrap();
        assert_eq!(json["v"], 2);
        assert_eq!(json["t"], "space-join");
        assert!(join_url(&link).starts_with("https://roostr.space/j#r2."));
        let nameless = SpaceJoinLink { name: None, ..link };
        assert!(!serde_json::to_string(&nameless).unwrap().contains("name"));
        assert_eq!(
            decode_join_link(&encode_join_link(&nameless)),
            Some(nameless)
        );
        assert_eq!(decode_join_link("r1.abc"), None);
        assert_eq!(decode_join_link("r2.!!"), None);
    }
}
