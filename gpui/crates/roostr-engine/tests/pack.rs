//! Packs: byte parity with the Odin engine (plaintext, deflate) and the
//! harness (encryption, frame ranges) through
//! `glonOdin/core/testdata/record_vectors/pack.json`.

#![cfg(not(target_family = "wasm"))]

use std::path::PathBuf;

use roostr_engine::deflate::{deflate, inflate};
use roostr_engine::pack::{
    FRAME, PACK_BUCKETS, frame_range, frames_decrypt, frames_encrypt, pack_bucket_bytes,
    pack_build, pack_decrypt, pack_encrypt, pack_header_encode, pack_key, pack_read_bucket,
    pack_read_header,
};
use roostr_engine::record::{Record, bucket_hashes, record_canonical, record_decode, root_hash};
use serde_json::{Value as Json, json};
use sha2::{Digest, Sha256};

fn vectors() -> Json {
    let path = PathBuf::from(env!("GLON_ODIN_ROOT")).join("core/testdata/record_vectors/pack.json");
    let text = std::fs::read_to_string(&path).unwrap_or_else(|error| panic!("{}: {error}", path.display()));
    serde_json::from_str(&text).expect("pack.json")
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn unhex(text: &str) -> Vec<u8> {
    (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&text[i..i + 2], 16).expect("hex"))
        .collect()
}

fn records(root: &Json) -> Vec<Record> {
    root["records"]
        .as_array()
        .expect("records")
        .iter()
        .map(|record| Record::from_json(record).expect("record"))
        .collect()
}

/// A deflate case's input: {text} | {repeat, times} | {lcg, length, alphabet}.
fn deflate_input(spec: &Json) -> Vec<u8> {
    if let Some(text) = spec["text"].as_str() {
        return text.as_bytes().to_vec();
    }
    if let Some(unit) = spec["repeat"].as_str() {
        return unit.repeat(spec["times"].as_u64().unwrap() as usize).into_bytes();
    }
    let mut x = spec["lcg"].as_u64().unwrap();
    let alphabet = spec["alphabet"].as_u64().unwrap();
    (0..spec["length"].as_u64().unwrap())
        .map(|_| {
            x = x
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            ((x >> 33) % alphabet) as u8
        })
        .collect()
}

#[test]
fn deflate_matches_odin() {
    let root = vectors();
    for case in root["deflate"].as_array().expect("deflate cases") {
        let name = case["name"].as_str().unwrap();
        let input = deflate_input(&case["input"]);
        let out = deflate(&input);
        let expected = &case["expected"];
        if let Some(bytes) = expected["output"].as_str() {
            assert_eq!(hex(&out), bytes, "{name}: output");
        }
        assert_eq!(hex(&Sha256::digest(&out)), expected["outputSha256"], "{name}: sha256");
        assert_eq!(out.len() as u64, expected["outputLength"].as_u64().unwrap(), "{name}: length");
        assert_eq!(inflate(&out).as_deref(), Some(&input[..]), "{name}: inflates back");
    }
}

#[test]
fn pack_plaintext_matches_odin() {
    let root = vectors();
    let records = records(&root);
    let expected = &root["expected"];
    let pack = pack_build(&records, root["stream"].as_str().unwrap(), root["cursor"].as_u64().unwrap())
        .expect("pack builds");
    assert_eq!(hex(&pack.plaintext), expected["plaintext"], "plaintext");
    assert_eq!(hex(&pack.root), expected["root"], "root");
    assert_eq!(pack.root, root_hash(&bucket_hashes(&records)), "root is the fingerprint root");

    let (header, payload_start) = pack_read_header(&pack.plaintext).expect("header");
    assert_eq!(header, pack.header);
    assert_eq!(pack_header_encode(&header), pack_header_encode(&pack.header));
    assert_eq!(payload_start as u64, expected["payloadStart"].as_u64().unwrap());
    assert_eq!(header.stream, expected["header"]["stream"]);
    assert_eq!(header.cursor, expected["header"]["cursor"].as_u64().unwrap());
    assert_eq!(header.records, expected["header"]["records"].as_u64().unwrap());
    let empty: [u8; 32] = Sha256::digest([]).into();
    assert_eq!(hex(&empty), expected["emptyBucketHash"]);
    for (index, bucket) in header.buckets.iter().enumerate() {
        let key = index.to_string();
        if bucket.count == 0 {
            assert!(expected["header"]["buckets"].get(&key).is_none(), "bucket {index}");
            assert_eq!((bucket.hash, bucket.length), (empty, 0), "bucket {index}: empty");
            continue;
        }
        let want = &expected["header"]["buckets"][&key];
        assert_eq!(
            json!({"offset": bucket.offset, "length": bucket.length, "hash": hex(&bucket.hash), "count": bucket.count}),
            *want,
            "bucket {index}: header"
        );
        let data = pack_bucket_bytes(&pack.plaintext, &header, payload_start, index).expect("bytes");
        assert_eq!(hex(&inflate(data).unwrap()), expected["buckets"][&key]["raw"], "bucket {index}: raw");
        let got = pack_read_bucket(data, &header, index).expect("bucket reads");
        let ids: Vec<&str> = got.iter().map(|record| record.object_id.as_str()).collect();
        assert_eq!(json!(ids), expected["buckets"][&key]["ids"], "bucket {index}: ids");
        for record in &got {
            let original = records.iter().find(|r| r.object_id == record.object_id).unwrap();
            assert_eq!(record_canonical(record), record_canonical(original), "{}", record.object_id);
        }
    }
}

#[test]
fn pack_encryption_matches_harness() {
    let root = vectors();
    let encrypted = &root["encrypted"];
    let stream_key = unhex(root["streamKey"].as_str().unwrap());
    let cursor = root["cursor"].as_u64().unwrap();
    let plaintext = unhex(root["expected"]["plaintext"].as_str().unwrap());
    assert_eq!(hex(&pack_key(&stream_key, cursor)), encrypted["packKey"], "pack key");
    let cipher = pack_encrypt(&plaintext, &stream_key, cursor);
    assert_eq!(hex(&cipher), encrypted["cipher"], "cipher");
    assert_eq!(hex(&Sha256::digest(&cipher)), encrypted["cipherSha256"], "cipher sha256");
    assert_eq!(pack_decrypt(&cipher, &stream_key, cursor).unwrap(), plaintext);
    assert!(pack_decrypt(&cipher, &stream_key, cursor + 1).is_err(), "another cursor's key fails");

    // Lite: one bucket through its frame range.
    let (header, payload_start) = pack_read_header(&plaintext).unwrap();
    let key = pack_key(&stream_key, cursor);
    for (index, bucket) in header.buckets.iter().enumerate().filter(|(_, b)| b.count > 0) {
        let range = frame_range(payload_start as u64 + bucket.offset, bucket.length, plaintext.len() as u64);
        let frames = frames_decrypt(&cipher[range.cipher_start as usize..range.cipher_end as usize], &key, range.first)
            .unwrap();
        let data = &frames[range.skip as usize..(range.skip + bucket.length) as usize];
        assert_eq!(pack_read_bucket(data, &header, index).unwrap().len(), bucket.count as usize);
    }
}

#[test]
fn frame_ranges_match_harness() {
    let root = vectors();
    for case in root["frames"].as_array().expect("frame cases") {
        let range = frame_range(
            case["offset"].as_u64().unwrap(),
            case["length"].as_u64().unwrap(),
            case["total"].as_u64().unwrap(),
        );
        assert_eq!(
            json!({"firstFrame": range.first, "lastFrame": range.last, "cipherStart": range.cipher_start, "cipherEnd": range.cipher_end, "skip": range.skip}),
            case["expected"],
            "{case}"
        );
    }
}

#[test]
fn frames_span_several_and_refuse_tampering() {
    let key = [7u8; 32];
    let mut x = 1u64;
    let plain: Vec<u8> = (0..2 * FRAME + 5)
        .map(|_| {
            x = x.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
            (x >> 56) as u8
        })
        .collect();
    let cipher = frames_encrypt(&plain, &key);
    assert_eq!(cipher.len() as u64, 2 * FRAME + 5 + 3 * 16);
    assert_eq!(frames_decrypt(&cipher, &key, 0).unwrap(), plain);
    let range = frame_range(FRAME + 3, 10, plain.len() as u64);
    let middle = frames_decrypt(&cipher[range.cipher_start as usize..range.cipher_end as usize], &key, range.first).unwrap();
    assert_eq!(&middle[range.skip as usize..range.skip as usize + 10], &plain[FRAME as usize + 3..FRAME as usize + 13]);
    let mut tampered = cipher.clone();
    tampered[FRAME as usize + 40] ^= 1;
    assert!(frames_decrypt(&tampered, &key, 0).is_err());
    assert!(frames_decrypt(&cipher[..cipher.len() - 20], &key, 0).is_err(), "truncated");
    assert_eq!(frames_decrypt(&frames_encrypt(&[], &key), &key, 0).unwrap(), Vec::<u8>::new());
}

#[test]
fn pack_refuses_bad_input() {
    let root = vectors();
    let mut records = records(&root);
    records.push(records[0].clone());
    assert!(pack_build(&records, "s", 1).is_err(), "two records for one object");
    records.pop();
    let pack = pack_build(&records, "s", 1).unwrap();
    let (header, start) = pack_read_header(&pack.plaintext).unwrap();
    assert!(pack_read_header(&pack.plaintext[..start - 1]).is_err(), "truncated header");
    let index = (0..PACK_BUCKETS).find(|&i| header.buckets[i].count > 0).unwrap();
    let data = pack_bucket_bytes(&pack.plaintext, &header, start, index).unwrap();
    let mut tampered = data.to_vec();
    let middle = tampered.len() / 2;
    tampered[middle] ^= 0x20;
    assert!(pack_read_bucket(&tampered, &header, index).is_err(), "tampered bucket");
    let mut wrong = header.clone();
    wrong.buckets[index].hash[0] ^= 1;
    assert!(pack_read_bucket(data, &wrong, index).is_err(), "hash mismatch");
}

#[test]
fn records_decode_back() {
    let root = vectors();
    for record in records(&root) {
        let bytes = record_canonical(&record);
        let back = record_decode(&bytes).expect("decodes");
        assert_eq!(record_canonical(&back), bytes, "{}", record.object_id);
        assert!(record_decode(&bytes[..bytes.len() - 1]).is_none(), "{}: truncated", record.object_id);
    }
}
