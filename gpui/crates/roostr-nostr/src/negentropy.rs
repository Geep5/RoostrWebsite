//! Negentropy range-based set reconciliation, protocol V1 (0x61), byte-exact
//! with the reference implementation (hoytech/negentropy `js/Negentropy.js`)
//! that NIP-77 carries over NEG-OPEN / NEG-MSG; a port of negentropy.ts.
//!
//! Item = (timestamp, 32-byte id), sorted by timestamp then id bytes.
//! Fingerprint = first 16 bytes of sha256(sum of ids as little-endian
//! 256-bit integers mod 2^256 || varint(count)).
//!
//! The replica only initiates; the responder side lets tests reconcile two
//! in-process peers against the reference transcripts.

use std::cmp::Ordering;
use std::collections::HashSet;
use std::fmt;

use sha2::{Digest, Sha256};

pub const PROTOCOL_VERSION: u8 = 0x61;
const ID_SIZE: usize = 32;
const FINGERPRINT_SIZE: usize = 16;
const BUCKETS: usize = 16;
/// The reserved "infinity" timestamp (encoded as 0 on the wire).
const INFINITY: u64 = u64::MAX;

const MODE_SKIP: u64 = 0;
const MODE_FINGERPRINT: u64 = 1;
const MODE_ID_LIST: u64 = 2;

/// Why a set or a message was refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NegentropyError {
    /// An id that is not 32 bytes (64 hex characters).
    BadId,
    /// A timestamp of 2^64-1, which the protocol reserves.
    BadTimestamp,
    DuplicateItem,
    /// A frame limit below 4096 bytes (0 means unlimited).
    FrameSizeLimitTooSmall,
    AlreadyInitiated,
    /// The first byte is not a protocol version (0x60..=0x6f).
    InvalidVersion(u8),
    /// The peer speaks another protocol version.
    UnsupportedVersion(u8),
    ParseEndsPrematurely,
    VarintOverflow,
    BoundKeyTooLong,
    UnexpectedMode(u64),
}

impl fmt::Display for NegentropyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BadId => f.write_str("negentropy: bad id size for added item"),
            Self::BadTimestamp => f.write_str("negentropy: bad timestamp"),
            Self::DuplicateItem => f.write_str("negentropy: duplicate item inserted"),
            Self::FrameSizeLimitTooSmall => f.write_str("negentropy: frameSizeLimit too small"),
            Self::AlreadyInitiated => f.write_str("negentropy: already initiated"),
            Self::InvalidVersion(v) => {
                write!(f, "negentropy: invalid protocol version byte {v:#04x}")
            }
            Self::UnsupportedVersion(v) => {
                write!(
                    f,
                    "negentropy: unsupported protocol version requested: {}",
                    v.wrapping_sub(0x60)
                )
            }
            Self::ParseEndsPrematurely => f.write_str("negentropy: parse ends prematurely"),
            Self::VarintOverflow => f.write_str("negentropy: varint overflow"),
            Self::BoundKeyTooLong => f.write_str("negentropy: bound key too long"),
            Self::UnexpectedMode(m) => write!(f, "negentropy: unexpected mode {m}"),
        }
    }
}

impl std::error::Error for NegentropyError {}

/// One element of a set.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Item {
    pub timestamp: u64,
    pub id: [u8; ID_SIZE],
}

/// A range boundary: a timestamp and an id prefix of 0..=32 bytes.
#[derive(Clone, Copy)]
struct Bound {
    timestamp: u64,
    id: [u8; ID_SIZE],
    len: usize,
}

impl Bound {
    const ZERO: Bound = Bound {
        timestamp: 0,
        id: [0; ID_SIZE],
        len: 0,
    };
    const INFINITY: Bound = Bound {
        timestamp: INFINITY,
        id: [0; ID_SIZE],
        len: 0,
    };

    fn prefix(&self) -> &[u8] {
        &self.id[..self.len]
    }
}

impl From<&Item> for Bound {
    fn from(item: &Item) -> Self {
        Bound {
            timestamp: item.timestamp,
            id: item.id,
            len: ID_SIZE,
        }
    }
}

fn compare(item: &Item, bound: &Bound) -> Ordering {
    item.timestamp
        .cmp(&bound.timestamp)
        .then_with(|| item.id.as_slice().cmp(bound.prefix()))
}

fn push_varint(out: &mut Vec<u8>, mut n: u64) {
    let mut digits = [0u8; 10];
    let mut len = 0;
    loop {
        digits[len] = (n & 0x7f) as u8;
        len += 1;
        n >>= 7;
        if n == 0 {
            break;
        }
    }
    for i in (0..len).rev() {
        out.push(if i > 0 { digits[i] | 0x80 } else { digits[i] });
    }
}

/// Encode `n` as the protocol's big-endian base-128 varint.
pub fn varint(n: u64) -> Vec<u8> {
    let mut out = Vec::with_capacity(10);
    push_varint(&mut out, n);
    out
}

struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn remaining(&self) -> usize {
        self.buf.len() - self.pos
    }

    fn byte(&mut self) -> Result<u8, NegentropyError> {
        let b = *self
            .buf
            .get(self.pos)
            .ok_or(NegentropyError::ParseEndsPrematurely)?;
        self.pos += 1;
        Ok(b)
    }

    fn bytes(&mut self, n: usize) -> Result<&'a [u8], NegentropyError> {
        if self.remaining() < n {
            return Err(NegentropyError::ParseEndsPrematurely);
        }
        let out = &self.buf[self.pos..self.pos + n];
        self.pos += n;
        Ok(out)
    }

    fn varint(&mut self) -> Result<u64, NegentropyError> {
        let mut res: u64 = 0;
        loop {
            let b = self.byte()?;
            res = res
                .checked_mul(128)
                .and_then(|r| r.checked_add(u64::from(b & 0x7f)))
                .ok_or(NegentropyError::VarintOverflow)?;
            if b & 0x80 == 0 {
                return Ok(res);
            }
        }
    }
}

/// A sorted, immutable item set (the reference's `NegentropyStorageVector`).
#[derive(Clone, Debug, Default)]
pub struct NegentropyStorage {
    items: Vec<Item>,
}

impl NegentropyStorage {
    /// Sort and seal `items`; duplicates are refused.
    pub fn new(items: impl IntoIterator<Item = Item>) -> Result<Self, NegentropyError> {
        let mut items: Vec<Item> = items.into_iter().collect();
        if items.iter().any(|i| i.timestamp == INFINITY) {
            return Err(NegentropyError::BadTimestamp);
        }
        items.sort_unstable();
        if items.windows(2).any(|w| w[0] == w[1]) {
            return Err(NegentropyError::DuplicateItem);
        }
        Ok(Self { items })
    }

    /// [`NegentropyStorage::new`] from `(created_at, hex id)` pairs (the
    /// held events of a stream).
    pub fn from_hex<'a>(
        items: impl IntoIterator<Item = (u64, &'a str)>,
    ) -> Result<Self, NegentropyError> {
        let parsed = items
            .into_iter()
            .map(|(timestamp, id)| {
                let id: [u8; ID_SIZE] = hex::decode(id)
                    .ok()
                    .and_then(|b| b.try_into().ok())
                    .ok_or(NegentropyError::BadId)?;
                Ok(Item { timestamp, id })
            })
            .collect::<Result<Vec<_>, _>>()?;
        Self::new(parsed)
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn items(&self) -> &[Item] {
        &self.items
    }

    /// First index in `begin..end` whose item is not below `bound`.
    fn lower_bound(&self, begin: usize, end: usize, bound: &Bound) -> usize {
        begin
            + self.items[begin..end].partition_point(|item| compare(item, bound) == Ordering::Less)
    }

    /// Fingerprint of `begin..end`.
    pub fn fingerprint(&self, begin: usize, end: usize) -> [u8; FINGERPRINT_SIZE] {
        let mut limbs = [0u64; 4];
        for item in &self.items[begin..end] {
            let mut carry = false;
            for (i, limb) in limbs.iter_mut().enumerate() {
                let word =
                    u64::from_le_bytes(item.id[i * 8..i * 8 + 8].try_into().expect("8 bytes"));
                let (sum, c1) = limb.overflowing_add(word);
                let (sum, c2) = sum.overflowing_add(u64::from(carry));
                *limb = sum;
                carry = c1 || c2;
            }
        }
        let mut input = Vec::with_capacity(ID_SIZE + 10);
        for limb in limbs {
            input.extend_from_slice(&limb.to_le_bytes());
        }
        push_varint(&mut input, (end - begin) as u64);
        let digest = Sha256::digest(&input);
        digest[..FINGERPRINT_SIZE].try_into().expect("16 bytes")
    }
}

/// One step of a reconciliation.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ReconcileStep {
    /// The next message to send; `None` when the initiator is done.
    pub output: Option<Vec<u8>>,
    /// Ids (hex) this side holds that the other lacks (initiator only).
    pub have_ids: Vec<String>,
    /// Ids (hex) the other side holds that this side lacks (initiator only).
    pub need_ids: Vec<String>,
}

/// One side of a reconciliation over a sealed set.
pub struct Negentropy {
    storage: NegentropyStorage,
    frame_size_limit: usize,
    initiator: bool,
    last_timestamp_in: u64,
    last_timestamp_out: u64,
}

impl Negentropy {
    /// `frame_size_limit` in bytes (0 = unlimited, else at least 4096); hex
    /// on the wire doubles it.
    pub fn new(
        storage: NegentropyStorage,
        frame_size_limit: usize,
    ) -> Result<Self, NegentropyError> {
        if frame_size_limit != 0 && frame_size_limit < 4096 {
            return Err(NegentropyError::FrameSizeLimitTooSmall);
        }
        Ok(Self {
            storage,
            frame_size_limit,
            initiator: false,
            last_timestamp_in: 0,
            last_timestamp_out: 0,
        })
    }

    /// The initiator's first message.
    pub fn initiate(&mut self) -> Result<Vec<u8>, NegentropyError> {
        if self.initiator {
            return Err(NegentropyError::AlreadyInitiated);
        }
        self.initiator = true;
        self.last_timestamp_out = 0;
        let mut out = vec![PROTOCOL_VERSION];
        self.split_range(0, self.storage.len(), Bound::INFINITY, &mut out);
        Ok(out)
    }

    /// Process the other side's message. The responder always answers; the
    /// initiator's `output` is `None` once the sets are reconciled.
    pub fn reconcile(&mut self, query: &[u8]) -> Result<ReconcileStep, NegentropyError> {
        let mut step = ReconcileStep::default();
        let mut query = Reader { buf: query, pos: 0 };
        self.last_timestamp_in = 0;
        self.last_timestamp_out = 0;

        let mut full = vec![PROTOCOL_VERSION];

        let version = query.byte()?;
        if !(0x60..=0x6f).contains(&version) {
            return Err(NegentropyError::InvalidVersion(version));
        }
        if version != PROTOCOL_VERSION {
            if self.initiator {
                return Err(NegentropyError::UnsupportedVersion(version));
            }
            step.output = Some(full);
            return Ok(step);
        }

        let storage_size = self.storage.len();
        let mut prev_bound = Bound::ZERO;
        let mut prev_index = 0;
        let mut skip = false;

        while query.remaining() != 0 {
            let mut o = Vec::new();
            let curr_bound = self.decode_bound(&mut query)?;
            let mode = query.varint()?;
            let lower = prev_index;
            let mut upper = self
                .storage
                .lower_bound(prev_index, storage_size, &curr_bound);

            match mode {
                MODE_SKIP => skip = true,
                MODE_FINGERPRINT => {
                    let theirs = query.bytes(FINGERPRINT_SIZE)?;
                    if theirs != self.storage.fingerprint(lower, upper) {
                        self.flush_skip(&mut skip, &prev_bound, &mut o);
                        self.split_range(lower, upper, curr_bound, &mut o);
                    } else {
                        skip = true;
                    }
                }
                MODE_ID_LIST => {
                    let count = query.varint()?;
                    let mut theirs: Vec<&[u8]> = Vec::new();
                    for _ in 0..count {
                        let id = query.bytes(ID_SIZE)?;
                        if self.initiator {
                            theirs.push(id);
                        }
                    }
                    if self.initiator {
                        skip = true;
                        let mut remaining: HashSet<&[u8]> = theirs.iter().copied().collect();
                        for item in &self.storage.items[lower..upper] {
                            if !remaining.remove(item.id.as_slice()) {
                                step.have_ids.push(hex::encode(item.id));
                            }
                        }
                        // Insertion order, first occurrence (a JS Set's iteration order).
                        for id in theirs {
                            if remaining.remove(id) {
                                step.need_ids.push(hex::encode(id));
                            }
                        }
                    } else {
                        self.flush_skip(&mut skip, &prev_bound, &mut o);
                        let mut ids = Vec::new();
                        let mut listed = 0u64;
                        let mut end_bound = curr_bound;
                        for i in lower..upper {
                            if self.exceeded(full.len() + ids.len()) {
                                end_bound = Bound::from(&self.storage.items[i]);
                                upper = i; // the remaining range gets its own fingerprint
                                break;
                            }
                            ids.extend_from_slice(&self.storage.items[i].id);
                            listed += 1;
                        }
                        self.encode_bound(&mut o, &end_bound);
                        push_varint(&mut o, MODE_ID_LIST);
                        push_varint(&mut o, listed);
                        o.extend_from_slice(&ids);
                        full.extend_from_slice(&o);
                        o.clear();
                    }
                }
                other => return Err(NegentropyError::UnexpectedMode(other)),
            }

            if self.exceeded(full.len() + o.len()) {
                // Frame full: stop here and fingerprint everything left.
                let remaining = self.storage.fingerprint(upper, storage_size);
                self.encode_bound(&mut full, &Bound::INFINITY);
                push_varint(&mut full, MODE_FINGERPRINT);
                full.extend_from_slice(&remaining);
                break;
            }
            full.extend_from_slice(&o);
            prev_index = upper;
            prev_bound = curr_bound;
        }

        step.output = (!(full.len() == 1 && self.initiator)).then_some(full);
        Ok(step)
    }

    fn flush_skip(&mut self, skip: &mut bool, prev_bound: &Bound, o: &mut Vec<u8>) {
        if !*skip {
            return;
        }
        *skip = false;
        self.encode_bound(o, prev_bound);
        push_varint(o, MODE_SKIP);
    }

    fn split_range(&mut self, lower: usize, upper: usize, upper_bound: Bound, o: &mut Vec<u8>) {
        let count = upper - lower;
        if count < BUCKETS * 2 {
            self.encode_bound(o, &upper_bound);
            push_varint(o, MODE_ID_LIST);
            push_varint(o, count as u64);
            for item in &self.storage.items[lower..upper] {
                o.extend_from_slice(&item.id);
            }
            return;
        }
        let per_bucket = count / BUCKETS;
        let with_extra = count % BUCKETS;
        let mut curr = lower;
        for i in 0..BUCKETS {
            let size = per_bucket + usize::from(i < with_extra);
            let fingerprint = self.storage.fingerprint(curr, curr + size);
            curr += size;
            let next = if curr == upper {
                upper_bound
            } else {
                minimal_bound(&self.storage.items[curr - 1], &self.storage.items[curr])
            };
            self.encode_bound(o, &next);
            push_varint(o, MODE_FINGERPRINT);
            o.extend_from_slice(&fingerprint);
        }
    }

    fn exceeded(&self, n: usize) -> bool {
        self.frame_size_limit != 0 && n > self.frame_size_limit - 200
    }

    fn decode_bound(&mut self, r: &mut Reader<'_>) -> Result<Bound, NegentropyError> {
        let raw = r.varint()?;
        let mut timestamp = if raw == 0 { INFINITY } else { raw - 1 };
        if self.last_timestamp_in == INFINITY || timestamp == INFINITY {
            self.last_timestamp_in = INFINITY;
            timestamp = INFINITY;
        } else {
            timestamp = timestamp
                .checked_add(self.last_timestamp_in)
                .filter(|t| *t != INFINITY)
                .ok_or(NegentropyError::VarintOverflow)?;
            self.last_timestamp_in = timestamp;
        }
        let len = usize::try_from(r.varint()?).map_err(|_| NegentropyError::BoundKeyTooLong)?;
        if len > ID_SIZE {
            return Err(NegentropyError::BoundKeyTooLong);
        }
        let mut bound = Bound {
            timestamp,
            id: [0; ID_SIZE],
            len,
        };
        bound.id[..len].copy_from_slice(r.bytes(len)?);
        Ok(bound)
    }

    fn encode_bound(&mut self, o: &mut Vec<u8>, bound: &Bound) {
        if bound.timestamp == INFINITY {
            self.last_timestamp_out = INFINITY;
            push_varint(o, 0);
        } else {
            // An infinite bound always ends a message, so time never runs backwards here.
            let delta = bound.timestamp.saturating_sub(self.last_timestamp_out);
            self.last_timestamp_out = bound.timestamp;
            push_varint(o, delta + 1);
        }
        push_varint(o, bound.len as u64);
        o.extend_from_slice(bound.prefix());
    }
}

/// The shortest bound that separates `prev` from `curr`.
fn minimal_bound(prev: &Item, curr: &Item) -> Bound {
    if curr.timestamp != prev.timestamp {
        return Bound {
            timestamp: curr.timestamp,
            id: [0; ID_SIZE],
            len: 0,
        };
    }
    let shared = prev
        .id
        .iter()
        .zip(&curr.id)
        .take_while(|(a, b)| a == b)
        .count();
    let len = (shared + 1).min(ID_SIZE);
    let mut bound = Bound {
        timestamp: curr.timestamp,
        id: [0; ID_SIZE],
        len,
    };
    bound.id[..len].copy_from_slice(&curr.id[..len]);
    bound
}
