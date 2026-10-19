//! Mailbox delivery and claims on the replica: concurrent deliveries record
//! one receiver copy, concurrent claims let exactly one worker process the
//! message, and a processed message is never claimed again. A claim is a
//! read/plan/commit transaction, so overlapping calls must not interleave.

#![cfg(not(target_family = "wasm"))]

mod backend_support;
mod support;

use backend_support::{params, signed_in, sqlite};
use futures::future::join_all;
use roostr_core::Backend;
use serde_json::{Value, json};
use support::{ENGINE_TURN, block_on};

async fn create(backend: &roostr_replica::ReplicaBackend, name: &str) -> String {
    backend
        .mutate(
            "create",
            params(json!({ "name": name, "type_key": "note" })),
        )
        .await
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string()
}

#[test]
fn concurrent_deliveries_and_claims_execute_a_mailbox_message_once() {
    let _turn = ENGINE_TURN.lock();
    block_on(async {
        let dir = tempfile::tempdir().unwrap();
        let keys = roostr_nostr::Keys::generate();
        let backend = signed_in(dir.path(), sqlite(dir.path()).await, &[], &keys).await;
        let sender = create(&backend, "Sender").await;
        let receiver = create(&backend, "Receiver").await;
        let message = json!({
            "id": "concurrent-message", "exchangeId": "concurrent-exchange",
            "sender": { "objectId": sender, "agentId": "" },
            "recipients": [{ "objectId": receiver, "agentId": "" }],
            "text": "Record this once", "replyTo": "", "sentAt": 100, "title": "Concurrent delivery",
            "requestReply": false, "historical": false, "operation": "", "author": "",
        });
        backend
            .mutate(
                "message_send",
                params(json!({ "object_id": sender, "message": message })),
            )
            .await
            .unwrap();
        let deliveries = (0..8).map(|_| {
            backend.mutate(
                "message_deliver",
                params(json!({
                    "sender_object_id": sender,
                    "message_id": "concurrent-message",
                    "recipient_object_id": receiver,
                })),
            )
        });
        for delivered in join_all(deliveries).await {
            delivered.unwrap();
        }
        let mailbox = backend.object(&receiver).await.unwrap().mailbox.unwrap();
        let texts: Vec<&str> = mailbox
            .iter()
            .map(|row| row.message.text.as_str())
            .collect();
        assert_eq!(texts, ["Record this once"]);

        let claims = (0..12).map(|index| {
            backend.mutate(
                "message_processing",
                params(json!({
                    "object_id": receiver,
                    "message_id": "concurrent-message",
                    "status": "processing",
                    "owner": format!("worker-{index}"),
                })),
            )
        });
        let claimed = join_all(claims)
            .await
            .into_iter()
            .map(Result::unwrap)
            .filter(|result| result.get("claimed") == Some(&Value::Bool(true)))
            .count();
        assert_eq!(claimed, 1);

        let received = backend.object(&receiver).await.unwrap();
        let owner = received.mailbox.unwrap()[0].processing.owner.clone();
        backend
            .mutate(
                "message_processing",
                params(json!({
                    "object_id": receiver, "message_id": "concurrent-message",
                    "status": "processed", "owner": owner,
                })),
            )
            .await
            .unwrap();
        let repeated = backend
            .mutate(
                "message_processing",
                params(json!({
                    "object_id": receiver, "message_id": "concurrent-message",
                    "status": "processing", "owner": "late-worker",
                })),
            )
            .await
            .unwrap();
        assert_eq!(repeated.get("claimed"), Some(&Value::Bool(false)));
        backend.stop();
    });
}
