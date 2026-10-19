//! Plain GETs to the wider web (Blossom servers, relay NIP-11 documents,
//! oEmbed endpoints): ureq on the desktop (blocking, so run through
//! [`crate::io::spawn`]), fetch in the browser (where the server's CORS
//! policy decides).

use std::time::Duration;

use anyhow::Result;
use serde::de::DeserializeOwned;

/// A whole answer.
#[derive(Clone, Debug)]
pub struct Response {
    pub status: u16,
    /// The status line's reason phrase ("Not Found"), "" when unknown.
    pub reason: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Response {
    pub fn is_success(&self) -> bool {
        (200..300).contains(&self.status)
    }

    /// A header, by case-insensitive name.
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    pub fn json<T: DeserializeOwned>(&self) -> Result<T> {
        Ok(serde_json::from_slice(&self.body)?)
    }
}

/// GET `url` with `headers`. Any HTTP status is an `Ok` answer; `limit`
/// caps the body on the desktop (the browser buffers what it is sent).
pub async fn get(
    url: &str,
    headers: &[(&str, &str)],
    timeout: Option<Duration>,
    limit: u64,
) -> Result<Response> {
    #[cfg(not(target_family = "wasm"))]
    {
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .timeout_global(timeout)
            .build()
            .into();
        let mut request = agent.get(url);
        for (name, value) in headers {
            request = request.header(*name, *value);
        }
        let mut response = request.call()?;
        let status = response.status().as_u16();
        let reason = response
            .status()
            .canonical_reason()
            .unwrap_or("")
            .to_string();
        let headers = response
            .headers()
            .iter()
            .filter_map(|(n, v)| Some((n.as_str().to_string(), v.to_str().ok()?.to_string())))
            .collect();
        let body = response
            .body_mut()
            .with_config()
            .limit(limit)
            .read_to_vec()?;
        Ok(Response {
            status,
            reason,
            headers,
            body,
        })
    }
    #[cfg(target_family = "wasm")]
    {
        let _ = limit;
        let timeout_ms = timeout.map(|t| t.as_millis().min(u32::MAX as u128) as u32);
        crate::web::request("GET", url, headers, None, timeout_ms).await
    }
}

/// PUT `body` to `url` (a Blossom upload). Any HTTP status is an `Ok` answer.
pub async fn put(url: &str, headers: &[(&str, &str)], body: &[u8], limit: u64) -> Result<Response> {
    #[cfg(not(target_family = "wasm"))]
    {
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .build()
            .into();
        let mut request = agent.put(url);
        for (name, value) in headers {
            request = request.header(*name, *value);
        }
        let mut response = request.send(body)?;
        let status = response.status().as_u16();
        let reason = response
            .status()
            .canonical_reason()
            .unwrap_or("")
            .to_string();
        let headers = response
            .headers()
            .iter()
            .filter_map(|(n, v)| Some((n.as_str().to_string(), v.to_str().ok()?.to_string())))
            .collect();
        let body = response
            .body_mut()
            .with_config()
            .limit(limit)
            .read_to_vec()?;
        Ok(Response {
            status,
            reason,
            headers,
            body,
        })
    }
    #[cfg(target_family = "wasm")]
    {
        let _ = limit;
        crate::web::request("PUT", url, headers, Some(body), None).await
    }
}
