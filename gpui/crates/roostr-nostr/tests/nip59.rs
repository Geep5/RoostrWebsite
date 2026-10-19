//! Gift wraps: one made by nostr-tools (the Svelte site's `wrapEvent`,
//! `nip59_fixture.json`) must open here, and ours must open there's way.

use roostr_nostr::nip59::{self, JoinRequest, Nip59Error, Rumor, SpaceInvite, TWO_DAYS_S};
use roostr_nostr::{Event, EventTemplate, Keys, kinds, nip44, now_s};
use serde_json::Value;

fn fixture() -> Value {
    let path = format!(
        "{}/tests/fixtures/nip59_fixture.json",
        env!("CARGO_MANIFEST_DIR")
    );
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

#[test]
fn opens_a_nostr_tools_gift_wrap() {
    let fx = fixture();
    let recipient = Keys::from_secret_hex(fx["recipientSecret"].as_str().unwrap()).unwrap();
    let wrap: Event = serde_json::from_value(fx["wrap"].clone()).unwrap();
    assert!(wrap.verify());
    assert_eq!(wrap.kind, kinds::GIFT_WRAP);
    assert_eq!(wrap.tag("p"), Some(recipient.pubkey()));
    let rumor = nip59::unwrap(&wrap, &recipient).unwrap();
    assert_eq!(rumor.pubkey, fx["senderPubkey"].as_str().unwrap());
    assert_eq!(rumor.kind, fx["rumor"]["kind"].as_u64().unwrap() as u32);
    assert_eq!(rumor.content, fx["rumor"]["content"].as_str().unwrap());
    assert!(rumor.tags.is_empty());
    let invite = SpaceInvite::parse(&rumor).unwrap();
    assert_eq!(invite.space, "space-fixture");
    assert_eq!(invite.name, "Fixture ✓");
    assert_eq!(invite.key, "ab".repeat(32));
    assert_eq!(invite.key_id, 3);
    assert_eq!(JoinRequest::parse(&rumor), None);

    let stranger = Keys::generate();
    assert!(matches!(
        nip59::unwrap(&wrap, &stranger),
        Err(Nip59Error::Nip44(nip44::Nip44Error::Mac))
    ));
}

#[test]
fn wraps_round_trip_with_randomized_times() {
    let sender = Keys::generate();
    let recipient = Keys::generate();
    let request = JoinRequest {
        space: "space-1".into(),
    };
    let before = now_s();
    let wrap = request.wrap(&sender, recipient.pubkey()).unwrap();
    assert_eq!(wrap.kind, 1059);
    assert_eq!(
        wrap.tags,
        vec![vec!["p".to_string(), recipient.pubkey().to_string()]]
    );
    assert_ne!(
        wrap.pubkey,
        sender.pubkey(),
        "the wrap is signed by a one-off key"
    );
    assert!(wrap.verify());
    assert!(wrap.created_at <= now_s() && wrap.created_at + TWO_DAYS_S + 1 >= before);

    let rumor = nip59::unwrap(&wrap, &recipient).unwrap();
    assert_eq!(rumor.pubkey, sender.pubkey());
    assert_eq!(rumor.kind, kinds::JOIN_REQUEST_RUMOR);
    assert_eq!(rumor.content, r#"{"t":"join-request","space":"space-1"}"#);
    assert_eq!(
        rumor.id,
        roostr_nostr::event_id(
            &rumor.pubkey,
            rumor.created_at,
            rumor.kind,
            &rumor.tags,
            &rumor.content
        )
    );
    assert_eq!(JoinRequest::parse(&rumor), Some(request));

    let invite = SpaceInvite {
        space: "s".into(),
        name: "Ünï".into(),
        key: "cd".repeat(32),
        key_id: 7,
    };
    let rumor = nip59::unwrap(
        &invite.wrap(&sender, recipient.pubkey()).unwrap(),
        &recipient,
    )
    .unwrap();
    assert_eq!(SpaceInvite::parse(&rumor), Some(invite));
    assert_eq!(
        rumor.content,
        format!(
            r#"{{"t":"space-invite","space":"s","name":"Ünï","key":"{}","keyId":7}}"#,
            "cd".repeat(32)
        )
    );
}

#[test]
fn refuses_forged_seals() {
    let sender = Keys::generate();
    let impostor = Keys::generate();
    let recipient = Keys::generate();
    // A rumor claiming another author than the seal's signer.
    let rumor = Rumor::new(
        &impostor,
        EventTemplate {
            kind: kinds::SPACE_INVITE_RUMOR,
            created_at: now_s(),
            tags: vec![],
            content: "{}".into(),
        },
    );
    let seal = nip59::seal(&rumor, &sender, recipient.pubkey(), now_s()).unwrap();
    let wrap = nip59::wrap_seal(&seal, recipient.pubkey(), now_s()).unwrap();
    assert_eq!(
        nip59::unwrap(&wrap, &recipient),
        Err(Nip59Error::RumorPubkeyMismatch)
    );

    // A seal whose signature does not verify.
    let mut bad = nip59::seal(
        &Rumor::new(&sender, EventTemplate::default()),
        &sender,
        recipient.pubkey(),
        now_s(),
    )
    .unwrap();
    bad.sig = "00".repeat(64);
    let wrap = nip59::wrap_seal(&bad, recipient.pubkey(), now_s()).unwrap();
    assert_eq!(
        nip59::unwrap(&wrap, &recipient),
        Err(Nip59Error::BadSealSignature)
    );

    let not_a_wrap = sender.sign(EventTemplate {
        kind: 1,
        ..EventTemplate::default()
    });
    assert_eq!(
        nip59::unwrap(&not_a_wrap, &recipient),
        Err(Nip59Error::NotWrap(1))
    );
}

#[test]
fn invite_payload_checks_follow_sync_ts() {
    let sender = Keys::generate();
    let rumor = |content: &str| {
        Rumor::new(
            &sender,
            EventTemplate {
                kind: kinds::SPACE_INVITE_RUMOR,
                created_at: 1,
                tags: vec![],
                content: content.into(),
            },
        )
    };
    let key = "ab".repeat(32);
    assert!(
        SpaceInvite::parse(&rumor(&format!(
            r#"{{"t":"space-invite","space":"s","key":"{key}","keyId":1}}"#
        )))
        .is_some()
    );
    assert!(
        SpaceInvite::parse(&rumor(&format!(
            r#"{{"t":"space-invite","space":"s","key":"{key}","keyId":2.0}}"#
        )))
        .is_some()
    );
    for bad in [
        format!(r#"{{"t":"space-join","space":"s","key":"{key}","keyId":1}}"#),
        format!(r#"{{"t":"space-invite","space":"","key":"{key}","keyId":1}}"#),
        format!(
            r#"{{"t":"space-invite","space":"s","key":"{}","keyId":1}}"#,
            key.to_uppercase()
        ),
        format!(r#"{{"t":"space-invite","space":"s","key":"{key}","keyId":0}}"#),
        format!(r#"{{"t":"space-invite","space":"s","key":"{key}","keyId":1.5}}"#),
        format!(r#"{{"t":"space-invite","space":"s","key":"{key}","keyId":9007199254740992}}"#),
        format!(r#"{{"t":"space-invite","space":"s","key":"{key}"}}"#),
        "not json".to_string(),
    ] {
        assert_eq!(SpaceInvite::parse(&rumor(&bad)), None, "{bad}");
    }
}
