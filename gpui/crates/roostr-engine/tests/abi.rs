//! The ABI's lifecycle through the native binding: domain errors, request
//! bounds and resets leave the engine usable, and schema messages a client
//! names without its own protobuf reader (a thread's participants) decode
//! through the engine's codec.

#![cfg(not(target_family = "wasm"))]

use futures::executor::block_on;
use roostr_engine::{Engine, EngineError, MessageKind, QueryUpdate, REQUEST_LIMIT};
use serde_json::{Value, json};

fn engine() -> Engine {
    Engine::native().expect("native engine")
}

fn domain(result: Result<impl std::fmt::Debug, EngineError>, context: &str) -> String {
    match result {
        Err(EngineError::Domain(message)) => message,
        other => panic!("{context}: expected a domain error, got {other:?}"),
    }
}

fn empty_replay_answers(engine: &Engine) {
    assert_eq!(
        block_on(engine.call("replay", json!({"changes": []}), None)).unwrap(),
        Value::Null,
        "the engine still answers"
    );
}

#[test]
fn domain_errors_leave_the_engine_usable() {
    let engine = engine();
    assert_eq!(
        domain(
            block_on(engine.call("not-a-method", json!({}), None)),
            "unknown method"
        ),
        "unknown core method"
    );
    empty_replay_answers(&engine);
}

#[test]
fn request_bytes_and_parser_depth_are_bounded() {
    let engine = engine();
    let oversized = json!({"data": "x".repeat(REQUEST_LIMIT)});
    assert!(
        matches!(
            block_on(engine.call("replay", oversized, None)),
            Err(EngineError::RequestTooLarge(_))
        ),
        "a request past 16 MiB is refused"
    );
    let mut nested = json!({});
    for _ in 0..130 {
        nested = json!({"next": nested});
    }
    let message = domain(block_on(engine.call("replay", nested, None)), "nesting");
    assert!(
        message.contains("nesting exceeds 128"),
        "parser depth: {message}"
    );
    empty_replay_answers(&engine);
}

#[test]
fn a_reset_bumps_the_generation_and_queries_still_answer() {
    let engine = engine();
    let body = json!({});
    let query =
        || block_on(engine.query_objects(&[], &body, 1_704_067_200_000)).expect("empty query");
    let first = query();
    assert_eq!((first.total, first.records.len()), (0, 0));
    let previous = engine.generation();
    block_on(engine.reset_all());
    assert_eq!(engine.generation(), previous + 1);
    assert_eq!(query(), first);
    let cached = block_on(engine.query(QueryUpdate {
        upserts: &[],
        removed: &[],
        reset: false,
        body: Some(&body),
        now_ms: 0,
    }))
    .unwrap();
    assert_eq!(cached.total, 0, "a reset releases the query cache");
}

/// A client names a thread's participants from its root bytes.
#[test]
fn conversation_round_trips_through_the_codec() {
    let engine = engine();
    let bytes = block_on(engine.encode_message(
        MessageKind::Conversation,
        &json!({
            "id": "__thread__50706675",
            "kind": "a2a",
            "title": "Pricing research",
            "participants": ["agent-scout", "agent-analyst"],
            "createdAt": 1_789_700_000_000_i64,
            "openedBy": "device-a",
            "aboutMessageId": "c8609781",
            "closed": false,
        }),
    ))
    .unwrap();
    let conversation = block_on(engine.decode_message(MessageKind::Conversation, &bytes)).unwrap();
    assert_eq!(conversation["kind"], "a2a");
    assert_eq!(conversation["title"], "Pricing research");
    assert_eq!(
        conversation["participants"],
        json!(["agent-scout", "agent-analyst"])
    );
    assert_eq!(conversation["aboutMessageId"], "c8609781");
    assert_eq!(conversation["closed"], false);
}

/// Bad input is refused, not guessed at.
#[test]
fn malformed_message_requests_are_refused() {
    let engine = engine();
    for payload in [
        json!({"action": "decode", "type": "conversation", "bytes": "!!!not base64"}),
        json!({"action": "decode", "type": "descriptor", "bytes": ""}),
        json!({"action": "decode", "type": "installation", "bytes": ""}),
        json!({"action": "frobnicate", "type": "conversation"}),
    ] {
        domain(
            block_on(engine.call("codec", payload.clone(), None)),
            &payload.to_string(),
        );
    }
}
