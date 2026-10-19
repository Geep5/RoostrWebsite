//! NIP-11 relay information documents: `GET` the relay's http(s) origin
//! with `Accept: application/nostr+json` (blossom.ts `blossomServer`).

use std::time::Duration;

use serde::Deserialize;

/// The fields Roostr reads; everything else is ignored.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct RelayInfo {
    pub name: Option<String>,
    pub description: Option<String>,
    pub pubkey: Option<String>,
    pub software: Option<String>,
    pub version: Option<String>,
    pub supported_nips: Vec<u64>,
    /// Non-standard feature names; RoostrRelay lists `"blossom"`.
    pub supported_features: Vec<String>,
}

impl RelayInfo {
    pub fn supports_feature(&self, feature: &str) -> bool {
        self.supported_features.iter().any(|f| f == feature)
    }
}

/// Largest information document read.
const MAX_DOCUMENT_BYTES: u64 = 1024 * 1024;

/// The http(s) origin of a relay URL: `ws:` becomes `http:`, anything else
/// `https:` (blossom.ts); `None` when it does not parse.
pub fn http_origin(relay: &str) -> Option<String> {
    let (scheme, rest) = relay.split_once("://")?;
    let scheme = if scheme.eq_ignore_ascii_case("ws") {
        "http"
    } else {
        "https"
    };
    let authority = rest
        .split(['/', '?', '#'])
        .next()
        .filter(|a| !a.is_empty())?;
    let authority = authority
        .rsplit_once('@')
        .map_or(authority, |(_, host)| host);
    let authority = authority.to_ascii_lowercase();
    // The URL origin omits the scheme's default port.
    let host = match (
        scheme,
        authority.strip_suffix(":443"),
        authority.strip_suffix(":80"),
    ) {
        ("https", Some(host), _) | ("http", _, Some(host)) => host.to_string(),
        _ => authority,
    };
    (!host.is_empty()).then(|| format!("{scheme}://{host}"))
}

/// Fetch `relay`'s information document.
pub async fn fetch(relay: &str, timeout: Duration) -> Result<RelayInfo, String> {
    let origin = http_origin(relay).ok_or_else(|| format!("{relay}: not a relay URL"))?;
    let text = get(&origin, timeout).await?;
    serde_json::from_str(&text).map_err(|e| format!("{origin}: not a NIP-11 document: {e}"))
}

#[cfg(not(target_family = "wasm"))]
async fn get(url: &str, timeout: Duration) -> Result<String, String> {
    let url = url.to_string();
    blocking::unblock(move || {
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .timeout_global(Some(timeout))
            .build()
            .into();
        let mut response = agent
            .get(&url)
            .header("Accept", "application/nostr+json")
            .call()
            .map_err(|e| format!("{url}: {e}"))?;
        response
            .body_mut()
            .with_config()
            .limit(MAX_DOCUMENT_BYTES)
            .read_to_string()
            .map_err(|e| format!("{url}: {e}"))
    })
    .await
}

#[cfg(target_family = "wasm")]
async fn get(url: &str, timeout: Duration) -> Result<String, String> {
    use wasm_bindgen::{JsCast, JsValue};
    use wasm_bindgen_futures::JsFuture;

    let _ = MAX_DOCUMENT_BYTES; // the browser buffers what it is sent
    let fail = |e: JsValue| {
        let detail = e
            .dyn_ref::<js_sys::Error>()
            .map(|e| String::from(e.message()))
            .or_else(|| e.as_string())
            .unwrap_or_else(|| "fetch failed".into());
        format!("{url}: {detail}")
    };
    let init = web_sys::RequestInit::new();
    init.set_method("GET");
    let headers = web_sys::Headers::new().map_err(fail)?;
    headers
        .set("Accept", "application/nostr+json")
        .map_err(fail)?;
    init.set_headers(&headers);
    let ms = timeout.as_millis().min(u32::MAX as u128) as u32;
    init.set_signal(Some(&web_sys::AbortSignal::timeout_with_u32(ms)));
    let request = web_sys::Request::new_with_str_and_init(url, &init).map_err(fail)?;
    let global = js_sys::global();
    let promise = if let Some(window) = global.dyn_ref::<web_sys::Window>() {
        window.fetch_with_request(&request)
    } else if let Some(worker) = global.dyn_ref::<web_sys::WorkerGlobalScope>() {
        worker.fetch_with_request(&request)
    } else {
        return Err(format!("{url}: no fetch in this context"));
    };
    let response: web_sys::Response = JsFuture::from(promise)
        .await
        .map_err(fail)?
        .unchecked_into();
    if !response.ok() {
        return Err(format!("{url}: HTTP {}", response.status()));
    }
    let text = JsFuture::from(response.text().map_err(fail)?)
        .await
        .map_err(fail)?;
    text.as_string()
        .ok_or_else(|| format!("{url}: unreadable body"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn origins_follow_blossom_ts() {
        assert_eq!(
            http_origin("wss://roostr-relay.fly.dev").as_deref(),
            Some("https://roostr-relay.fly.dev")
        );
        assert_eq!(
            http_origin("wss://Relay.Example:443/path?x").as_deref(),
            Some("https://relay.example")
        );
        assert_eq!(
            http_origin("ws://127.0.0.1:7777/").as_deref(),
            Some("http://127.0.0.1:7777")
        );
        assert_eq!(
            http_origin("ws://localhost:80").as_deref(),
            Some("http://localhost")
        );
        assert_eq!(
            http_origin("wss://relay.example:8443").as_deref(),
            Some("https://relay.example:8443")
        );
        assert_eq!(http_origin("not a url"), None);
        assert_eq!(http_origin("wss://"), None);
    }

    #[test]
    fn reads_supported_features() {
        let info: RelayInfo = serde_json::from_str(
            r#"{"name":"RoostrRelay","supported_nips":[1,11,77],"supported_features":["blossom"],"limitation":{"max_message_length":1048576}}"#,
        )
        .unwrap();
        assert!(info.supports_feature("blossom"));
        assert_eq!(info.supported_nips, vec![1, 11, 77]);
    }
}
