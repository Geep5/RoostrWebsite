//! Files on Blossom (BUD-01/02), served by RoostrRelay on its own origin.
//! Every host agrees on the blob format, so a file added anywhere opens here:
//!
//!   fresh random 32-byte key per file; AES-256-GCM over 4 MiB plaintext
//!   frames; frame i's nonce = 12-byte big-endian i; each frame's output is
//!   ciphertext || 16-byte tag; the blob is their concatenation. The last
//!   frame may be shorter; a zero-length file is one empty frame (the tag).
//!
//! With a daemon the harness uploads; this device's own replica uploads
//! here, so discovery, upload, download and the blob cipher live here.

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};
use std::time::Duration;

use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Nonce};
use anyhow::{anyhow, bail};
use roostr_core::http;
use sha2::{Digest, Sha256};

/// Plaintext bytes per frame.
pub const BLOB_FRAME_BYTES: usize = 4 * 1024 * 1024;
const TAG_BYTES: usize = 16;
/// Roostr's relay, used when no relay advertises Blossom.
pub const DEFAULT_BLOSSOM_SERVER: &str = "https://roostr-relay.fly.dev";
/// Largest blob a download will read.
const MAX_DOWNLOAD_BYTES: u64 = 2 * 1024 * 1024 * 1024;
/// Largest relay information document read.
const MAX_DOCUMENT_BYTES: u64 = 10 * 1024 * 1024;

fn frame_nonce(index: usize) -> [u8; 12] {
    let mut nonce = [0u8; 12];
    nonce[4..].copy_from_slice(&(index as u64).to_be_bytes());
    nonce
}

fn aes_key(key: &[u8]) -> anyhow::Result<Aes256Gcm> {
    if key.len() != 32 {
        bail!("a blob key is 32 bytes");
    }
    Aes256Gcm::new_from_slice(key).map_err(|_| anyhow!("a blob key is 32 bytes"))
}

/// Lowercase hex sha256.
pub fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

/// Encrypt a whole file into the framed blob format.
pub fn encrypt_blob(plain: &[u8], key: &[u8]) -> anyhow::Result<Vec<u8>> {
    let aes = aes_key(key)?;
    let frames = plain.len().div_ceil(BLOB_FRAME_BYTES).max(1);
    let mut out = Vec::with_capacity(plain.len() + frames * TAG_BYTES);
    for i in 0..frames {
        let start = (i * BLOB_FRAME_BYTES).min(plain.len());
        let end = ((i + 1) * BLOB_FRAME_BYTES).min(plain.len());
        let sealed = aes
            .encrypt(Nonce::from_slice(&frame_nonce(i)), &plain[start..end])
            .map_err(|_| anyhow!("the file could not be encrypted (frame {i})"))?;
        out.extend_from_slice(&sealed);
    }
    Ok(out)
}

/// Decrypt a framed blob; any tampered, truncated or reordered frame fails.
pub fn decrypt_blob(blob: &[u8], key: &[u8]) -> anyhow::Result<Vec<u8>> {
    let aes = aes_key(key)?;
    let stride = BLOB_FRAME_BYTES + TAG_BYTES;
    let frames = blob.len().div_ceil(stride).max(1);
    if blob.len() < frames * TAG_BYTES || blob.len() - (frames - 1) * stride < TAG_BYTES {
        bail!("not an encrypted Roostr blob (truncated frame)");
    }
    let mut out = Vec::with_capacity(blob.len() - frames * TAG_BYTES);
    for (i, frame) in blob.chunks(stride).enumerate().take(frames) {
        let plain = aes
            .decrypt(Nonce::from_slice(&frame_nonce(i)), frame)
            .map_err(|_| {
                anyhow!("the file could not be decrypted (frame {i} failed authentication)")
            })?;
        out.extend_from_slice(&plain);
    }
    Ok(out)
}

/// Discovered servers, keyed by the relay list joined by " ".
static SERVERS: LazyLock<Mutex<HashMap<String, String>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// The https (or http, for ws:) origin of a relay URL.
fn relay_origin(relay: &str) -> Option<String> {
    let mut url = url::Url::parse(relay).ok()?;
    let scheme = if url.scheme() == "ws" {
        "http"
    } else {
        "https"
    };
    url.set_scheme(scheme).ok()?;
    let origin = url.origin();
    origin.is_tuple().then(|| origin.ascii_serialization())
}

async fn advertises_blossom(origin: &str) -> bool {
    let Ok(res) = http::get(
        origin,
        &[("Accept", "application/nostr+json")],
        Some(Duration::from_secs(5)),
        MAX_DOCUMENT_BYTES,
    )
    .await
    else {
        return false;
    };
    let Some(doc) = res
        .is_success()
        .then(|| res.json::<serde_json::Value>().ok())
        .flatten()
    else {
        return false;
    };
    doc.get("supported_features")
        .and_then(|f| f.as_array())
        .is_some_and(|f| f.iter().any(|v| v.as_str() == Some("blossom")))
}

/// The Blossom server for these relays: the origin of the first relay whose
/// NIP-11 document lists the "blossom" feature, else
/// [`DEFAULT_BLOSSOM_SERVER`]. Cached per relay list for the process life.
pub async fn blossom_server(relays: &[String]) -> String {
    let key = relays.join(" ");
    if let Some(server) = SERVERS.lock().ok().and_then(|m| m.get(&key).cloned()) {
        return server;
    }
    let mut server = DEFAULT_BLOSSOM_SERVER.to_string();
    for origin in relays.iter().filter_map(|relay| relay_origin(relay)) {
        if advertises_blossom(&origin).await {
            server = origin;
            break;
        }
    }
    if let Ok(mut map) = SERVERS.lock() {
        map.insert(key, server.clone());
    }
    server
}

/// PUT `blob` (sha256 hex `sha256`) to `server`'s `/upload` with a BUD-11
/// `authorization`; resolves to the descriptor's URL once the server
/// stored exactly these bytes (blossom.ts `uploadBlob`).
pub async fn upload_blob(
    server: &str,
    blob: &[u8],
    sha256: &str,
    authorization: &str,
) -> anyhow::Result<String> {
    let res = http::put(
        &format!("{server}/upload"),
        &[
            ("Authorization", authorization),
            ("X-SHA-256", sha256),
            ("Content-Type", "application/octet-stream"),
        ],
        blob,
        MAX_DOCUMENT_BYTES,
    )
    .await
    .map_err(|e| anyhow!("Upload to Blossom failed: {e}"))?;
    if !res.is_success() {
        let reason = res
            .header("X-Reason")
            .filter(|v| !v.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| format!("{} {}", res.status, res.reason).trim().to_string());
        bail!("Upload to Blossom failed: {reason}");
    }
    let descriptor: serde_json::Value = res.json()?;
    let stored = descriptor["sha256"].as_str().unwrap_or_default();
    if stored != sha256 {
        bail!("Blossom stored {stored}, not the uploaded {sha256}");
    }
    Ok(descriptor["url"].as_str().unwrap_or_default().to_string())
}

/// GET a blob and check it is the one named (sha256 hex, lowercase): a
/// server is never trusted with the bytes.
pub async fn download_blob(url: &str, sha256: &str) -> anyhow::Result<Vec<u8>> {
    let res = http::get(url, &[], None, MAX_DOWNLOAD_BYTES)
        .await
        .map_err(|e| anyhow!("Download from Blossom failed: {e}"))?;
    if !res.is_success() {
        let reason = res
            .header("X-Reason")
            .filter(|v| !v.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| format!("{} {}", res.status, res.reason).trim().to_string());
        bail!("Download from Blossom failed: {reason}");
    }
    let bytes = res.body;
    if sha256_hex(&bytes) != sha256 {
        bail!("Blossom returned different bytes than the file names");
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    const KEY: [u8; 32] = [7u8; 32];

    #[test]
    fn round_trips() {
        for size in [
            0,
            1,
            BLOB_FRAME_BYTES,
            BLOB_FRAME_BYTES + 1,
            9 * 1024 * 1024 + 123,
        ] {
            let plain: Vec<u8> = (0..size).map(|i| (i * 31 % 251) as u8).collect();
            let blob = encrypt_blob(&plain, &KEY).unwrap();
            let frames = size.div_ceil(BLOB_FRAME_BYTES).max(1);
            assert_eq!(blob.len(), size + frames * TAG_BYTES);
            assert_eq!(decrypt_blob(&blob, &KEY).unwrap(), plain);
        }
    }

    #[test]
    fn tampered_frame_one_fails() {
        let plain = vec![1u8; BLOB_FRAME_BYTES + 10];
        let mut blob = encrypt_blob(&plain, &KEY).unwrap();
        blob[BLOB_FRAME_BYTES + TAG_BYTES + 3] ^= 1;
        let err = decrypt_blob(&blob, &KEY).unwrap_err().to_string();
        assert_eq!(
            err,
            "the file could not be decrypted (frame 1 failed authentication)"
        );
    }

    #[test]
    fn bad_key_and_truncation() {
        assert_eq!(
            encrypt_blob(b"x", &[0; 31]).unwrap_err().to_string(),
            "a blob key is 32 bytes"
        );
        assert_eq!(
            decrypt_blob(&[0; 40], &[0; 33]).unwrap_err().to_string(),
            "a blob key is 32 bytes"
        );
        for len in [0, 15] {
            assert_eq!(
                decrypt_blob(&vec![0; len], &KEY).unwrap_err().to_string(),
                "not an encrypted Roostr blob (truncated frame)"
            );
        }
    }

    #[test]
    fn known_answer() {
        let key = [0u8; 32];
        let blob = encrypt_blob(b"hello", &key).unwrap();
        let aes = Aes256Gcm::new_from_slice(&key).unwrap();
        let expected = aes
            .encrypt(Nonce::from_slice(&[0u8; 12]), b"hello".as_slice())
            .unwrap();
        assert_eq!(blob, expected);
    }

    #[test]
    fn sha256_abc() {
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn unreachable_relays_fall_back() {
        let start = Instant::now();
        assert_eq!(
            futures::executor::block_on(blossom_server(&["ws://127.0.0.1:9".to_string()])),
            DEFAULT_BLOSSOM_SERVER
        );
        assert!(start.elapsed() < Duration::from_secs(7));
    }

    #[test]
    fn relay_origins() {
        assert_eq!(
            relay_origin("wss://r.example/x").as_deref(),
            Some("https://r.example")
        );
        assert_eq!(
            relay_origin("ws://127.0.0.1:9").as_deref(),
            Some("http://127.0.0.1:9")
        );
        assert_eq!(relay_origin("not a url"), None);
    }

    // The Svelte site's scripts/blossom.test.ts: frames are opened with
    // AES-GCM directly, so the format - not just a round trip - is pinned.

    fn pattern(size: usize) -> Vec<u8> {
        (0..size).map(|i| (i * 31 + 7) as u8).collect()
    }

    fn open_frame(frame: &[u8], index: usize) -> Vec<u8> {
        let aes = Aes256Gcm::new_from_slice(&KEY).unwrap();
        aes.decrypt(Nonce::from_slice(&frame_nonce(index)), frame)
            .unwrap()
    }

    #[test]
    fn a_zero_length_file_is_one_empty_frame() {
        let blob = encrypt_blob(&[], &KEY).unwrap();
        assert_eq!(blob.len(), TAG_BYTES);
        assert!(open_frame(&blob, 0).is_empty());
        assert!(decrypt_blob(&blob, &KEY).unwrap().is_empty());
    }

    #[test]
    fn frames_are_sealed_under_their_big_endian_index_the_last_one_shorter() {
        let plain = pattern(2 * BLOB_FRAME_BYTES + 5);
        let blob = encrypt_blob(&plain, &KEY).unwrap();
        let stride = BLOB_FRAME_BYTES + TAG_BYTES;
        assert_eq!(blob.len(), plain.len() + 3 * TAG_BYTES);
        for (i, frame) in blob.chunks(stride).enumerate() {
            let end = ((i + 1) * BLOB_FRAME_BYTES).min(plain.len());
            assert_eq!(open_frame(frame, i), plain[i * BLOB_FRAME_BYTES..end]);
        }
        assert_eq!(decrypt_blob(&blob, &KEY).unwrap(), plain);
    }

    #[test]
    fn truncated_reordered_or_wrongly_keyed_blobs_never_decrypt() {
        let plain = pattern(BLOB_FRAME_BYTES + 100);
        let blob = encrypt_blob(&plain, &KEY).unwrap();
        let stride = BLOB_FRAME_BYTES + TAG_BYTES;
        assert!(decrypt_blob(&blob[..blob.len() - 1], &KEY).is_err());
        let short = decrypt_blob(&blob[..stride + 10], &KEY).unwrap_err();
        assert!(short.to_string().contains("truncated"), "{short}");
        let swapped = [&blob[stride..], &blob[..stride]].concat();
        assert!(decrypt_blob(&swapped, &KEY).is_err());
        let wrong_key = decrypt_blob(&blob, &[0; 32]).unwrap_err();
        assert!(wrong_key.to_string().contains("frame 0"), "{wrong_key}");
        let tiny = decrypt_blob(&[0; 5], &KEY).unwrap_err();
        assert!(tiny.to_string().contains("truncated"), "{tiny}");
    }

    #[test]
    fn every_file_gets_a_different_blob_under_a_fresh_key() {
        let plain = pattern(64);
        let other = [0x5au8; 32];
        assert_ne!(
            sha256_hex(&encrypt_blob(&plain, &KEY).unwrap()),
            sha256_hex(&encrypt_blob(&plain, &other).unwrap())
        );
        assert_eq!(
            sha256_hex(&[]),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    /// An HTTP server on 127.0.0.1 answering every request with `body`;
    /// its origin and each request's `Accept` header.
    fn serve(body: Vec<u8>) -> (String, std::sync::mpsc::Receiver<String>) {
        use std::io::{BufRead, BufReader, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let (seen, accepts) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { return };
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut accept = String::new();
                let mut line = String::new();
                while reader.read_line(&mut line).is_ok_and(|n| n > 0) && line != "\r\n" {
                    if let Some((name, value)) = line.split_once(':')
                        && name.eq_ignore_ascii_case("accept")
                    {
                        accept = value.trim().to_string();
                    }
                    line.clear();
                }
                let _ = seen.send(accept);
                let head = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = stream.write_all(head.as_bytes());
                let _ = stream.write_all(&body);
            }
        });
        (origin, accepts)
    }

    /// One upload as a Blossom server saw it.
    struct SeenUpload {
        request_line: String,
        sha256: String,
        authorization: String,
        body: Vec<u8>,
    }

    /// A server answering one request with `descriptor`, reporting what it saw.
    fn serve_upload(descriptor: String) -> (String, std::sync::mpsc::Receiver<SeenUpload>) {
        use std::io::{BufRead, BufReader, Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let Ok((mut stream, _)) = listener.accept() else {
                return;
            };
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut request_line = String::new();
            reader.read_line(&mut request_line).unwrap();
            let (mut sha256, mut authorization, mut length) = (String::new(), String::new(), 0);
            let mut line = String::new();
            while reader.read_line(&mut line).is_ok_and(|n| n > 0) && line != "\r\n" {
                if let Some((name, value)) = line.split_once(':') {
                    let value = value.trim().to_string();
                    match name.to_ascii_lowercase().as_str() {
                        "x-sha-256" => sha256 = value,
                        "authorization" => authorization = value,
                        "content-length" => length = value.parse().unwrap(),
                        _ => {}
                    }
                }
                line.clear();
            }
            let mut body = vec![0; length];
            reader.read_exact(&mut body).unwrap();
            let _ = tx.send(SeenUpload {
                request_line: request_line.trim().to_string(),
                sha256,
                authorization,
                body,
            });
            let head = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                descriptor.len()
            );
            let _ = stream.write_all(head.as_bytes());
            let _ = stream.write_all(descriptor.as_bytes());
        });
        (origin, rx)
    }

    #[test]
    fn uploads_put_the_blob_with_its_hash_and_authorization() {
        let blob = encrypt_blob(&pattern(10), &KEY).unwrap();
        let sha = sha256_hex(&blob);
        let (server, seen) = serve_upload(format!(
            r#"{{"sha256":"{sha}","url":"https://b.example/{sha}"}}"#
        ));
        let url =
            futures::executor::block_on(upload_blob(&server, &blob, &sha, "Nostr abc")).unwrap();
        assert_eq!(url, format!("https://b.example/{sha}"));
        let seen = seen.recv().unwrap();
        assert_eq!(seen.request_line, "PUT /upload HTTP/1.1");
        assert_eq!(
            (seen.sha256.as_str(), seen.authorization.as_str()),
            (sha.as_str(), "Nostr abc")
        );
        assert_eq!(seen.body, blob);

        // A server that stored other bytes is refused.
        let (server, _) = serve_upload(format!(r#"{{"sha256":"{}","url":"x"}}"#, "0".repeat(64)));
        let err = futures::executor::block_on(upload_blob(&server, &blob, &sha, "Nostr abc"))
            .unwrap_err();
        assert!(err.to_string().contains("not the uploaded"), "{err}");
    }

    #[test]
    fn downloads_are_checked_against_the_named_hash() {
        let blob = encrypt_blob(&pattern(10), &KEY).unwrap();
        let sha = sha256_hex(&blob);
        let (good, _) = serve(blob.clone());
        let url = format!("{good}/{sha}");
        assert_eq!(
            futures::executor::block_on(download_blob(&url, &sha)).unwrap(),
            blob
        );
        let (bad, _) = serve(vec![1, 2, 3]);
        let url = format!("{bad}/{sha}");
        let err = futures::executor::block_on(download_blob(&url, &sha)).unwrap_err();
        assert!(err.to_string().contains("different bytes"), "{err}");
    }

    #[test]
    fn the_server_is_the_first_relay_whose_nip11_lists_blossom() {
        let (plain, plain_seen) = serve(br#"{"supported_nips":[1]}"#.to_vec());
        let (blossom, blossom_seen) = serve(br#"{"supported_features":["blossom"]}"#.to_vec());
        let relays = [plain, blossom.clone()].map(|origin| origin.replacen("http", "ws", 1) + "/");
        assert_eq!(
            futures::executor::block_on(blossom_server(&relays)),
            blossom
        );
        for seen in [plain_seen, blossom_seen] {
            let accepts: Vec<String> = seen.try_iter().collect();
            assert_eq!(accepts, ["application/nostr+json"]);
        }
    }
}
