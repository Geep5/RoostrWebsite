//! Packs (glonOdin `docs/state-objects.md` "Packs"): a stream's records as
//! of a relay cursor in one encrypted blob. The twin of glonOdin
//! `core/pack.odin` (same plaintext bytes, pinned by
//! `core/testdata/record_vectors/pack.json`), plus the encryption, which the
//! Odin side leaves to the harness (`harness/src/pack.ts`).
//!
//! Plaintext: `varint(len(header)) || Pack_Header || payload`.
//!
//! ```proto
//! message Pack_Header {
//!   string stream = 1;
//!   uint64 cursor = 2;
//!   uint64 records = 3;               // total record count
//!   repeated Pack_Bucket buckets = 4; // exactly 256, bucket b at index b
//! }
//! message Pack_Bucket {
//!   uint64 offset = 1;  // in the payload: sum of the earlier lengths
//!   uint64 length = 2;  // compressed bytes; 0 for an empty bucket
//!   bytes  hash   = 3;  // 32 bytes: the bucket's fingerprint hash
//!   uint32 count  = 4;  // records in the bucket
//! }
//! ```
//!
//! Bucket b: the records with `sha256(object_id)[0] == b`, sorted by object
//! id, each `varint(len) || canonical Record`, the run compressed as one raw
//! DEFLATE stream ([`crate::deflate`]); an empty bucket is no bytes.
//!
//! Blob: the Blossom frame format (`harness/src/blossom.ts`): AES-256-GCM
//! over 4 MiB plaintext frames, frame i under the 12-byte big-endian nonce
//! i, each written as ciphertext || 16-byte tag; key
//! [`pack_key`] = HMAC-SHA256(stream key, "roostr-pack-v1" || cursor as
//! 8 bytes big-endian).

use std::fmt;

use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Nonce};
use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};

use crate::deflate::{deflate, inflate};
use crate::record::{Record, record_bucket, record_canonical, record_hash};
use crate::record_decode::record_decode;

pub const PACK_BUCKETS: usize = 256;
/// Plaintext bytes per Blossom frame.
pub const FRAME: u64 = 4 * 1024 * 1024;
/// AES-GCM tag after each frame's ciphertext.
pub const TAG: u64 = 16;
/// HMAC label of the pack key (with the cursor).
pub const PACK_KEY_LABEL: &[u8] = b"roostr-pack-v1";

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PackBucket {
    /// Offset of the compressed bucket in the payload.
    pub offset: u64,
    /// Compressed length; 0 for an empty bucket.
    pub length: u64,
    /// Bucket fingerprint: sha256 of its record hashes in id order.
    pub hash: [u8; 32],
    pub count: u32,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PackHeader {
    pub stream: String,
    pub cursor: u64,
    pub records: u64,
    /// Exactly [`PACK_BUCKETS`], bucket b at index b.
    pub buckets: Vec<PackBucket>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pack {
    pub plaintext: Vec<u8>,
    pub header: PackHeader,
    /// sha256 of the 256 bucket hashes (the fingerprint root).
    pub root: [u8; 32],
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PackError {
    /// Two records for one object.
    DuplicateObject(String),
    /// The header is malformed (or the bytes are not a pack).
    Header(String),
    /// A bucket's bytes do not match its header entry.
    Bucket { bucket: usize, reason: String },
    /// A frame failed AES-GCM authentication, or the blob is truncated.
    Decrypt(String),
}

impl fmt::Display for PackError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PackError::DuplicateObject(id) => write!(f, "pack: two records for object {id}"),
            PackError::Header(reason) => write!(f, "pack header: {reason}"),
            PackError::Bucket { bucket, reason } => write!(f, "pack bucket {bucket}: {reason}"),
            PackError::Decrypt(reason) => write!(f, "pack blob: {reason}"),
        }
    }
}

impl std::error::Error for PackError {}

fn varint(out: &mut Vec<u8>, mut value: u64) {
    while value >= 0x80 {
        out.push((value as u8 & 0x7f) | 0x80);
        value >>= 7;
    }
    out.push(value as u8);
}

fn uint_field(out: &mut Vec<u8>, field: u64, value: u64) {
    if value != 0 {
        varint(out, field << 3);
        varint(out, value);
    }
}

fn bytes_field(out: &mut Vec<u8>, field: u64, data: &[u8]) {
    varint(out, field << 3 | 2);
    varint(out, data.len() as u64);
    out.extend_from_slice(data);
}

/// The `Pack_Header` protobuf.
pub fn pack_header_encode(header: &PackHeader) -> Vec<u8> {
    let mut out = Vec::new();
    if !header.stream.is_empty() {
        bytes_field(&mut out, 1, header.stream.as_bytes());
    }
    uint_field(&mut out, 2, header.cursor);
    uint_field(&mut out, 3, header.records);
    let mut entry = Vec::new();
    for bucket in &header.buckets {
        entry.clear();
        uint_field(&mut entry, 1, bucket.offset);
        uint_field(&mut entry, 2, bucket.length);
        bytes_field(&mut entry, 3, &bucket.hash);
        uint_field(&mut entry, 4, u64::from(bucket.count));
        bytes_field(&mut out, 4, &entry);
    }
    out
}

/// sha256 of the bucket hashes: the stream's fingerprint root.
pub fn pack_root(header: &PackHeader) -> [u8; 32] {
    let mut hasher = Sha256::new();
    for bucket in &header.buckets {
        hasher.update(bucket.hash);
    }
    hasher.finalize().into()
}

/// The pack plaintext for `records` (one per object, tombstoned objects
/// included), byte-identical to the Odin engine's `pack_build`.
pub fn pack_build(records: &[Record], stream: &str, cursor: u64) -> Result<Pack, PackError> {
    let mut order: Vec<&Record> = records.iter().collect();
    order.sort_by(|a, b| a.object_id.as_bytes().cmp(b.object_id.as_bytes()));
    if let Some(pair) = order.windows(2).find(|pair| pair[0].object_id == pair[1].object_id) {
        return Err(PackError::DuplicateObject(pair[0].object_id.clone()));
    }
    let mut per_bucket: Vec<Vec<&Record>> = vec![Vec::new(); PACK_BUCKETS];
    for record in order {
        per_bucket[usize::from(record_bucket(&record.object_id))].push(record);
    }
    let mut header = PackHeader {
        stream: stream.to_owned(),
        cursor,
        records: records.len() as u64,
        buckets: Vec::with_capacity(PACK_BUCKETS),
    };
    let mut payload = Vec::new();
    let mut raw = Vec::new();
    for bucket in &per_bucket {
        raw.clear();
        let mut hasher = Sha256::new();
        for record in bucket {
            let encoded = record_canonical(record);
            varint(&mut raw, encoded.len() as u64);
            raw.extend_from_slice(&encoded);
            hasher.update(Sha256::digest(&encoded));
        }
        let mut entry = PackBucket {
            offset: payload.len() as u64,
            length: 0,
            hash: hasher.finalize().into(),
            count: bucket.len() as u32,
        };
        if !bucket.is_empty() {
            let compressed = deflate(&raw);
            entry.length = compressed.len() as u64;
            payload.extend_from_slice(&compressed);
        }
        header.buckets.push(entry);
    }
    let encoded = pack_header_encode(&header);
    let mut plaintext = Vec::with_capacity(encoded.len() + payload.len() + 10);
    varint(&mut plaintext, encoded.len() as u64);
    plaintext.extend_from_slice(&encoded);
    plaintext.extend_from_slice(&payload);
    let root = pack_root(&header);
    Ok(Pack {
        plaintext,
        header,
        root,
    })
}

struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn varint(&mut self) -> Option<u64> {
        let mut result = 0u64;
        let mut shift = 0u32;
        while let Some(&b) = self.data.get(self.pos) {
            self.pos += 1;
            if shift == 63 && b > 1 {
                return None;
            }
            result |= u64::from(b & 0x7f) << shift;
            if b < 0x80 {
                return Some(result);
            }
            shift += 7;
            if shift >= 64 {
                return None;
            }
        }
        None
    }

    fn bytes(&mut self) -> Option<&'a [u8]> {
        let size = usize::try_from(self.varint()?).ok()?;
        let end = self.pos.checked_add(size)?;
        let out = self.data.get(self.pos..end)?;
        self.pos = end;
        Some(out)
    }

    fn skip(&mut self, wire: u64) -> Option<()> {
        match wire {
            0 => self.varint().map(drop),
            1 | 5 => {
                let size = if wire == 1 { 8 } else { 4 };
                self.data.get(self.pos..self.pos + size)?;
                self.pos += size;
                Some(())
            }
            2 => self.bytes().map(drop),
            _ => None,
        }
    }
}

fn header_error(reason: &str) -> PackError {
    PackError::Header(reason.to_owned())
}

fn decode_bucket(data: &[u8]) -> Option<PackBucket> {
    let mut bucket = PackBucket::default();
    let mut hash_seen = false;
    let mut r = Reader { data, pos: 0 };
    while r.pos < data.len() {
        let tag = r.varint()?;
        match (tag >> 3, tag & 7) {
            (1, 0) => bucket.offset = r.varint()?,
            (2, 0) => bucket.length = r.varint()?,
            (3, 2) => {
                bucket.hash = r.bytes()?.try_into().ok()?;
                hash_seen = true;
            }
            (4, 0) => bucket.count = u32::try_from(r.varint()?).ok()?,
            (_, wire) => r.skip(wire)?,
        }
    }
    hash_seen.then_some(bucket)
}

/// The header at the start of a pack plaintext (a prefix holding it is
/// enough: a Lite device reads it from frame 0) and where the payload
/// starts. Checks 256 buckets with 32-byte hashes, contiguous offsets, and
/// counts summing to `records`.
pub fn pack_read_header(data: &[u8]) -> Result<(PackHeader, usize), PackError> {
    let mut r = Reader { data, pos: 0 };
    let encoded = r.bytes().ok_or_else(|| header_error("truncated"))?;
    let payload_start = r.pos;
    let mut header = PackHeader::default();
    let mut h = Reader {
        data: encoded,
        pos: 0,
    };
    while h.pos < encoded.len() {
        let tag = h.varint().ok_or_else(|| header_error("truncated"))?;
        let ok = match (tag >> 3, tag & 7) {
            (1, 2) => h
                .bytes()
                .and_then(|b| String::from_utf8(b.to_vec()).ok())
                .map(|stream| header.stream = stream),
            (2, 0) => h.varint().map(|v| header.cursor = v),
            (3, 0) => h.varint().map(|v| header.records = v),
            (4, 2) => h.bytes().and_then(decode_bucket).map(|b| header.buckets.push(b)),
            (_, wire) => h.skip(wire),
        };
        ok.ok_or_else(|| header_error("malformed field"))?;
    }
    if header.buckets.len() != PACK_BUCKETS {
        return Err(header_error("not 256 buckets"));
    }
    let (mut next, mut total) = (0u64, 0u64);
    for bucket in &header.buckets {
        if bucket.offset != next || (bucket.length == 0) != (bucket.count == 0) {
            return Err(header_error("buckets not contiguous"));
        }
        next += bucket.length;
        total += u64::from(bucket.count);
    }
    if total != header.records {
        return Err(header_error("bucket counts do not sum to records"));
    }
    Ok((header, payload_start))
}

/// Bucket `bucket`'s compressed bytes inside a whole plaintext.
pub fn pack_bucket_bytes<'a>(
    plaintext: &'a [u8],
    header: &PackHeader,
    payload_start: usize,
    bucket: usize,
) -> Result<&'a [u8], PackError> {
    let entry = &header.buckets[bucket];
    let start = payload_start as u64 + entry.offset;
    let end = start + entry.length;
    usize::try_from(start)
        .ok()
        .zip(usize::try_from(end).ok())
        .and_then(|(start, end)| plaintext.get(start..end))
        .ok_or_else(|| PackError::Bucket {
            bucket,
            reason: "plaintext too short".into(),
        })
}

/// The records of bucket `bucket` from its compressed bytes, checked against
/// the header: count, ids strictly increasing and in this bucket, and the
/// bucket hash (over each record's canonical re-encoding).
pub fn pack_read_bucket(data: &[u8], header: &PackHeader, bucket: usize) -> Result<Vec<Record>, PackError> {
    let fail = |reason: &str| PackError::Bucket {
        bucket,
        reason: reason.to_owned(),
    };
    let entry = &header.buckets[bucket];
    if data.len() as u64 != entry.length {
        return Err(fail("length differs from the header"));
    }
    if entry.count == 0 {
        return Ok(Vec::new());
    }
    let raw = inflate(data).ok_or_else(|| fail("not a DEFLATE stream"))?;
    let mut r = Reader { data: &raw, pos: 0 };
    let mut out: Vec<Record> = Vec::with_capacity(entry.count as usize);
    let mut hasher = Sha256::new();
    while r.pos < raw.len() {
        let encoded = r.bytes().ok_or_else(|| fail("truncated record"))?;
        if out.len() >= entry.count as usize {
            return Err(fail("more records than the header says"));
        }
        let record = record_decode(encoded).ok_or_else(|| fail("malformed record"))?;
        if usize::from(record_bucket(&record.object_id)) != bucket {
            return Err(fail("record in the wrong bucket"));
        }
        if out
            .last()
            .is_some_and(|last| last.object_id.as_bytes() >= record.object_id.as_bytes())
        {
            return Err(fail("records not sorted by id"));
        }
        hasher.update(record_hash(&record));
        out.push(record);
    }
    if out.len() != entry.count as usize {
        return Err(fail("fewer records than the header says"));
    }
    if <[u8; 32]>::from(hasher.finalize()) != entry.hash {
        return Err(fail("hash differs from the header"));
    }
    Ok(out)
}

// ── Encryption: the Blossom frame format ────────────────────────────

/// HMAC-SHA256(stream key, "roostr-pack-v1" || cursor as u64 big-endian).
pub fn pack_key(stream_key: &[u8], cursor: u64) -> [u8; 32] {
    let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(stream_key).expect("HMAC takes any key length");
    mac.update(PACK_KEY_LABEL);
    mac.update(&cursor.to_be_bytes());
    mac.finalize().into_bytes().into()
}

fn frame_nonce(index: u64) -> [u8; 12] {
    let mut nonce = [0u8; 12];
    nonce[4..].copy_from_slice(&index.to_be_bytes());
    nonce
}

/// Encrypts `plaintext` in the Blossom frame format (an empty plaintext is
/// one empty frame: the tag alone).
pub fn frames_encrypt(plaintext: &[u8], key: &[u8; 32]) -> Vec<u8> {
    let cipher = Aes256Gcm::new(key.into());
    let frames = plaintext.len().div_ceil(FRAME as usize).max(1);
    let mut out = Vec::with_capacity(plaintext.len() + frames * TAG as usize);
    for index in 0..frames {
        let start = index * FRAME as usize;
        let end = (start + FRAME as usize).min(plaintext.len());
        let sealed = cipher
            .encrypt(Nonce::from_slice(&frame_nonce(index as u64)), &plaintext[start..end])
            .expect("AES-GCM encrypts any frame");
        out.extend_from_slice(&sealed);
    }
    out
}

/// Decrypts consecutive frames starting at frame `first` (a whole blob from
/// 0, or the cipher bytes of a [`FrameRange`]). Every frame but the last
/// given must be full.
pub fn frames_decrypt(cipher: &[u8], key: &[u8; 32], first: u64) -> Result<Vec<u8>, PackError> {
    let whole = (FRAME + TAG) as usize;
    let tail = cipher.len() % whole;
    if cipher.len() < TAG as usize || (tail != 0 && tail < TAG as usize) {
        return Err(PackError::Decrypt("truncated".into()));
    }
    let aes = Aes256Gcm::new(key.into());
    let mut out = Vec::with_capacity(cipher.len());
    for (index, frame) in cipher.chunks(whole).enumerate() {
        let opened = aes
            .decrypt(Nonce::from_slice(&frame_nonce(first + index as u64)), frame)
            .map_err(|_| PackError::Decrypt(format!("frame {} failed authentication", first + index as u64)))?;
        out.extend_from_slice(&opened);
    }
    Ok(out)
}

/// The encrypted pack: [`frames_encrypt`] under [`pack_key`].
pub fn pack_encrypt(plaintext: &[u8], stream_key: &[u8], cursor: u64) -> Vec<u8> {
    frames_encrypt(plaintext, &pack_key(stream_key, cursor))
}

/// The pack plaintext from the whole blob.
pub fn pack_decrypt(cipher: &[u8], stream_key: &[u8], cursor: u64) -> Result<Vec<u8>, PackError> {
    frames_decrypt(cipher, &pack_key(stream_key, cursor), 0)
}

/// The frames holding plaintext bytes `[offset, offset + length)` of a pack
/// whose plaintext is `total` bytes, and their bytes in the blob.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameRange {
    pub first: u64,
    pub last: u64,
    /// Blob bytes `[cipher_start, cipher_end)`: an HTTP Range of
    /// `bytes=cipher_start-(cipher_end - 1)`.
    pub cipher_start: u64,
    pub cipher_end: u64,
    /// Where `offset` lands in the decrypted frames.
    pub skip: u64,
}

/// Maps a plaintext range (`length` >= 1; a bucket: `payload_start +
/// offset`, `length`) to the frames covering it: frame i holds plaintext
/// `[i * FRAME, (i + 1) * FRAME)` and sits at blob bytes
/// `[i * (FRAME + TAG), (i + 1) * (FRAME + TAG))`, the last frame shorter.
pub fn frame_range(offset: u64, length: u64, total: u64) -> FrameRange {
    let frames = total.div_ceil(FRAME).max(1);
    let cipher_total = total + frames * TAG;
    let first = offset / FRAME;
    let last = (offset + length.max(1) - 1) / FRAME;
    FrameRange {
        first,
        last,
        cipher_start: first * (FRAME + TAG),
        cipher_end: ((last + 1) * (FRAME + TAG)).min(cipher_total),
        skip: offset - first * FRAME,
    }
}
