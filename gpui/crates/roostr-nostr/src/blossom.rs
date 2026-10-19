//! The Nostr side of Blossom (blossom.ts): which server holds a vault's
//! files, and the BUD-11 authorization an upload carries. The blob format
//! itself (AES-256-GCM frames) lives with the file views.

use std::collections::HashMap;
use std::sync::LazyLock;
use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use parking_lot::Mutex;

use crate::event::{Event, EventTemplate};
use crate::{kinds, nip11};

/// Roostr's relay, used when no relay advertises Blossom.
pub const DEFAULT_SERVER: &str = "https://roostr-relay.fly.dev";
/// Upload authorizations expire this long after signing.
pub const AUTH_TTL_S: u64 = 600;
/// How long one relay's information document may take.
const DISCOVERY_TIMEOUT: Duration = Duration::from_secs(5);

/// The answer per relay list, for the process's life (blossom.ts caches
/// per page).
static SERVERS: LazyLock<Mutex<HashMap<String, String>>> = LazyLock::new(Default::default);

/// The Blossom server for these relays: the https origin of the first relay
/// whose NIP-11 document lists the `"blossom"` feature, else
/// [`DEFAULT_SERVER`].
pub async fn server(relays: &[String]) -> String {
    let key = relays.join(" ");
    if let Some(found) = SERVERS.lock().get(&key).cloned() {
        return found;
    }
    let mut found = DEFAULT_SERVER.to_string();
    for relay in relays {
        let Some(origin) = nip11::http_origin(relay) else {
            continue;
        };
        match nip11::fetch(relay, DISCOVERY_TIMEOUT).await {
            Ok(info) if info.supports_feature("blossom") => {
                found = origin;
                break;
            }
            Ok(_) => {}
            Err(error) => log::debug!("blossom discovery: {error}"),
        }
    }
    SERVERS.lock().insert(key, found.clone());
    found
}

/// BUD-11 upload authorization for one blob (`sha256` lowercase hex), valid
/// for [`AUTH_TTL_S`]; sign it with [`crate::Keys::sign`].
pub fn upload_auth_template(sha256: &str, name: &str, now: u64) -> EventTemplate {
    EventTemplate {
        kind: kinds::BLOSSOM_AUTH,
        created_at: now,
        tags: vec![
            vec!["t".into(), "upload".into()],
            vec!["x".into(), sha256.to_string()],
            vec!["expiration".into(), (now + AUTH_TTL_S).to_string()],
        ],
        content: format!("Upload {name}"),
    }
}

/// The `Authorization` header: `"Nostr "` + base64 of the signed event's
/// JSON (UTF-8: names may be in any script).
pub fn authorization_header(event: &Event) -> String {
    format!("Nostr {}", BASE64.encode(event.to_json()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Keys;

    #[test]
    fn upload_authorization_matches_blossom_ts() {
        // scripts/blossom.test.ts: kind 24242 with t=upload, x=<sha256>, a
        // ten-minute expiration and the file name.
        let sha = "ab".repeat(32);
        let template = upload_auth_template(&sha, "Résumé.pdf", 1_760_000_000);
        let tag = |pair: [&str; 2]| pair.map(str::to_string).to_vec();
        assert_eq!(
            template,
            EventTemplate {
                kind: 24242,
                created_at: 1_760_000_000,
                tags: vec![
                    tag(["t", "upload"]),
                    tag(["x", &sha]),
                    tag(["expiration", "1760000600"]),
                ],
                content: "Upload Résumé.pdf".into(),
            }
        );
        let keys = Keys::generate();
        let event = keys.sign(template);
        assert!(event.verify());
        let header = authorization_header(&event);
        let decoded = BASE64
            .decode(header.strip_prefix("Nostr ").unwrap())
            .unwrap();
        let back: Event = serde_json::from_slice(&decoded).unwrap();
        assert_eq!(back, event);
        assert!(back.verify());
    }
}
