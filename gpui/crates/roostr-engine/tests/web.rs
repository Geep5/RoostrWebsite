//! Browser smoke test: the site's `engine.wasm`, loaded and verified through
//! the binding, decodes the codec fixtures exactly as the native library does.
//!
//! The engine is fetched from `ROOSTR_ENGINE_TEST_BASE` (build time; default
//! `http://127.0.0.1:8787/`), which must serve RoostrWebsite `static/`
//! (`engine.wasm`, `engine-core.json`) with CORS. Run with
//! `wasm-bindgen-test-runner` as the wasm32 test runner, in a browser.

#![cfg(target_family = "wasm")]

use roostr_engine::{Engine, EngineError, SealRequest, WebSource};
use serde_json::{Value, json};
use wasm_bindgen_test::*;

wasm_bindgen_test_configure!(run_in_browser);

const CODEC_FIXTURES: &str =
    include_str!(concat!(env!("GLON_ODIN_ROOT"), "/core/codec_fixtures.json"));
const WIRE_FIXTURES: &str =
    include_str!(concat!(env!("GLON_ODIN_ROOT"), "/core/wire_fixtures.json"));

fn unhex(text: &str) -> Vec<u8> {
    (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&text[i..i + 2], 16).expect("hex"))
        .collect()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

async fn engine() -> Engine {
    let base = option_env!("ROOSTR_ENGINE_TEST_BASE").unwrap_or("http://127.0.0.1:8787/");
    Engine::load(&WebSource::under(base))
        .await
        .expect("engine loads and verifies")
}

#[wasm_bindgen_test]
async fn codec_fixtures_in_the_browser() {
    let engine = engine().await;
    assert_eq!(
        engine.source_fingerprint().len(),
        64,
        "fingerprint from engine-core.json"
    );
    let fixtures: Vec<Value> = serde_json::from_str(CODEC_FIXTURES).unwrap();
    assert!(!fixtures.is_empty());
    for fixture in &fixtures {
        let name = fixture["name"].as_str().unwrap();
        let id = fixture["id"].as_str().unwrap();
        let bytes_hex = fixture["bytesHex"].as_str().unwrap();
        let decoded = engine.decode_change(&unhex(bytes_hex)).await.unwrap();
        assert_eq!(decoded.id, id, "{name}: decoded id");
        assert_eq!(
            engine.change_id(&decoded).await.unwrap(),
            id,
            "{name}: content address"
        );
        assert_eq!(
            hex(&engine.encode_change(&decoded).await.unwrap()),
            bytes_hex,
            "{name}: re-encode"
        );
        if let Some(malformed) = fixture["malformedBytesHex"].as_str() {
            assert!(
                matches!(
                    engine.decode_change(&unhex(malformed)).await,
                    Err(EngineError::Domain(_))
                ),
                "{name}"
            );
        }
    }
    assert_eq!(
        engine.call("nope", json!({}), None).await,
        Err(EngineError::Domain("unknown core method".into()))
    );
}

/// Seals byte-exactly with a fixed nonce, and with a random one through the
/// `rand_bytes` import, then opens what it sealed.
#[wasm_bindgen_test]
async fn wire_seal_uses_the_odin_runtime() {
    let engine = engine().await;
    let fixtures: Value = serde_json::from_str(WIRE_FIXTURES).unwrap();
    let fixture = &fixtures["seal"][0];
    let change_text = fixture["change"].as_str().unwrap();
    let bytes =
        base64::Engine::decode(&base64::engine::general_purpose::STANDARD, change_text).unwrap();
    let conversation_key = fixture["conversationKey"].as_str().unwrap();
    let mut request = SealRequest {
        change: &bytes,
        conversation_key,
        secret: fixture["secret"].as_str(),
        space: None,
        object_id: fixture["objectId"].as_str().unwrap(),
        base: None,
        nonce: fixture["nonce"].as_str(),
    };
    let sealed = engine.seal(&request).await.unwrap();
    assert_eq!(serde_json::to_value(&sealed).unwrap(), fixture["expected"]);
    request.nonce = None;
    let random = engine.seal(&request).await.unwrap();
    assert_ne!(
        random.parts[0].content, sealed.parts[0].content,
        "a fresh nonce from crypto.getRandomValues"
    );
    assert_eq!(
        engine
            .open(&random.parts[0].content, conversation_key)
            .await
            .unwrap(),
        change_text
    );
}
