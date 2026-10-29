//! Raw DEFLATE (RFC 1951) for pack buckets: the twin of glonOdin
//! `core/deflate.odin`, the same algorithm so both engines write the same
//! bytes for the same input (pinned by `core/testdata/record_vectors/pack.json`).
//! Any inflater reads the output; [`inflate`] uses miniz_oxide.
//!
//! - LZ77, greedy: at each position the longest match among the newest
//!   [`MAX_CHAIN`] earlier positions with the same 3-byte hash (the nearest
//!   wins a tie) within 32 KiB; a match of 3+ bytes is taken, else a literal.
//!   Every position covered is inserted into the hash chains.
//! - Blocks of [`BLOCK_TOKENS`] tokens, each with dynamic Huffman codes; an
//!   empty input is one block holding only end-of-block.
//! - Code lengths: Huffman by the two-queue method over symbols sorted by
//!   (frequency, symbol), a leaf winning ties against an internal node; when
//!   the longest code exceeds the limit (15, or 7 for the code-length code)
//!   every non-zero frequency becomes (f + 1) / 2 and the tree is rebuilt.
//!   An alphabet with fewer than two used symbols gets the lowest unused
//!   ones at frequency 1, so every code is complete.
//! - Code lengths are run-length coded with 16 (repeat previous, 3-6), 17
//!   (zeros, 3-10) and 18 (zeros, 11-138) over the literal and distance
//!   lengths as one sequence; HLIT/HDIST/HCLEN as short as allowed.

const WINDOW: usize = 32768;
const MIN_MATCH: usize = 3;
const MAX_MATCH: usize = 258;
/// Hash-chain candidates tried per position.
pub const MAX_CHAIN: usize = 64;
const HASH_BITS: u32 = 15;
/// Tokens (literals and matches) per DEFLATE block.
pub const BLOCK_TOKENS: usize = 16384;

const LENGTH_BASE: [u16; 29] = [
    3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15, 17, 19, 23, 27, 31, 35, 43, 51, 59, 67, 83, 99, 115, 131,
    163, 195, 227, 258,
];
const LENGTH_EXTRA: [u8; 29] = [
    0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 0,
];
const DIST_BASE: [u16; 30] = [
    1, 2, 3, 4, 5, 7, 9, 13, 17, 25, 33, 49, 65, 97, 129, 193, 257, 385, 513, 769, 1025, 1537,
    2049, 3073, 4097, 6145, 8193, 12289, 16385, 24577,
];
const DIST_EXTRA: [u8; 30] = [
    0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12, 13, 13,
];
const CODE_LENGTH_ORDER: [usize; 19] = [16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15];

/// A literal (`dist` 0, `value` the byte) or a match (`value` its length).
#[derive(Clone, Copy)]
struct Token {
    value: u16,
    dist: u16,
}

#[derive(Default)]
struct BitWriter {
    out: Vec<u8>,
    acc: u64,
    n: u32,
}

impl BitWriter {
    fn put(&mut self, value: u64, count: u32) {
        self.acc |= value << self.n;
        self.n += count;
        while self.n >= 8 {
            self.out.push(self.acc as u8);
            self.acc >>= 8;
            self.n -= 8;
        }
    }
}

fn hash3(data: &[u8], i: usize) -> usize {
    let v = u32::from(data[i]) << 16 | u32::from(data[i + 1]) << 8 | u32::from(data[i + 2]);
    (v.wrapping_mul(2_654_435_761) >> (32 - HASH_BITS)) as usize
}

fn length_code(length: usize) -> usize {
    LENGTH_BASE.iter().rposition(|&base| usize::from(base) <= length).unwrap_or(0)
}

fn dist_code(dist: usize) -> usize {
    DIST_BASE.iter().rposition(|&base| usize::from(base) <= dist).unwrap_or(0)
}

fn tokens(data: &[u8]) -> Vec<Token> {
    let n = data.len();
    let mut out = Vec::with_capacity(n / 4 + 16);
    let mut head = vec![-1i32; 1 << HASH_BITS];
    let mut prev = vec![-1i32; n];
    let insert = |head: &mut [i32], prev: &mut [i32], i: usize| {
        let h = hash3(data, i);
        prev[i] = head[h];
        head[h] = i as i32;
    };
    let mut i = 0;
    while i < n {
        let (mut best_len, mut best_dist) = (0, 0);
        if i + MIN_MATCH <= n {
            let max_len = MAX_MATCH.min(n - i);
            let mut cand = head[hash3(data, i)];
            let mut chain = 0;
            while cand >= 0 && i - cand as usize <= WINDOW && chain < MAX_CHAIN {
                let c = cand as usize;
                // A candidate that differs at best_len cannot beat it.
                if data[c + best_len] == data[i + best_len] {
                    let mut l = 0;
                    while l < max_len && data[c + l] == data[i + l] {
                        l += 1;
                    }
                    if l > best_len {
                        best_len = l;
                        best_dist = i - c;
                        if l == max_len {
                            break;
                        }
                    }
                }
                cand = prev[c];
                chain += 1;
            }
            insert(&mut head, &mut prev, i);
        }
        if best_len >= MIN_MATCH {
            out.push(Token {
                value: best_len as u16,
                dist: best_dist as u16,
            });
            for j in i + 1..i + best_len {
                if j + MIN_MATCH <= n {
                    insert(&mut head, &mut prev, j);
                }
            }
            i += best_len;
        } else {
            out.push(Token {
                value: u16::from(data[i]),
                dist: 0,
            });
            i += 1;
        }
    }
    out
}

/// Length-limited Huffman code lengths for `freq` (see the module docs).
fn huffman_lengths(freq: &[u32], limit: usize) -> Vec<u8> {
    let mut f: Vec<u64> = freq.iter().map(|&x| u64::from(x)).collect();
    let mut used = f.iter().filter(|&&x| x > 0).count();
    for x in f.iter_mut() {
        if used >= 2 {
            break;
        }
        if *x == 0 {
            *x = 1;
            used += 1;
        }
    }
    loop {
        let mut leaves: Vec<usize> = (0..f.len()).filter(|&s| f[s] > 0).collect();
        leaves.sort_by_key(|&s| (f[s], s));
        let n = leaves.len();
        let mut weight: Vec<u64> = leaves.iter().map(|&s| f[s]).collect();
        let mut parent = vec![usize::MAX; n];
        let (mut next_leaf, mut next_inner) = (0, n);
        let mut pick = |weight: &[u64]| {
            if next_leaf < n && (next_inner >= weight.len() || weight[next_leaf] <= weight[next_inner]) {
                next_leaf += 1;
                next_leaf - 1
            } else {
                next_inner += 1;
                next_inner - 1
            }
        };
        for _ in 0..n - 1 {
            let a = pick(&weight);
            let b = pick(&weight);
            weight.push(weight[a] + weight[b]);
            parent.push(usize::MAX);
            parent[a] = weight.len() - 1;
            parent[b] = weight.len() - 1;
        }
        let mut depth = vec![0usize; weight.len()];
        let mut longest = 0;
        for k in (0..weight.len() - 1).rev() {
            depth[k] = depth[parent[k]] + 1;
            if k < n {
                longest = longest.max(depth[k]);
            }
        }
        if longest <= limit {
            let mut lengths = vec![0u8; f.len()];
            for (k, &s) in leaves.iter().enumerate() {
                lengths[s] = depth[k] as u8;
            }
            return lengths;
        }
        for x in f.iter_mut().filter(|x| **x > 0) {
            *x = x.div_ceil(2);
        }
    }
}

/// Canonical codes (RFC 1951 3.2.2), bit-reversed for LSB-first output.
fn huffman_codes(lengths: &[u8]) -> Vec<u16> {
    let mut count = [0u32; 16];
    for &l in lengths.iter().filter(|&&l| l > 0) {
        count[usize::from(l)] += 1;
    }
    let mut next = [0u32; 16];
    let mut code = 0;
    for bits in 1..16 {
        code = (code + count[bits - 1]) << 1;
        next[bits] = code;
    }
    lengths
        .iter()
        .map(|&l| {
            if l == 0 {
                return 0;
            }
            let c = next[usize::from(l)];
            next[usize::from(l)] += 1;
            (c.reverse_bits() >> (32 - u32::from(l))) as u16
        })
        .collect()
}

/// Run-length coded code lengths: (symbol 0-18, extra bits value).
fn run_length_code(lengths: &[u8]) -> Vec<(u8, u8)> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < lengths.len() {
        let v = lengths[i];
        let mut run = lengths[i..].iter().take_while(|&&x| x == v).count();
        i += run;
        if v == 0 {
            while run >= 11 {
                let r = run.min(138);
                out.push((18, (r - 11) as u8));
                run -= r;
            }
            if run >= 3 {
                out.push((17, (run - 3) as u8));
                run = 0;
            }
        } else {
            out.push((v, 0));
            run -= 1;
            while run >= 3 {
                let r = run.min(6);
                out.push((16, (r - 3) as u8));
                run -= r;
            }
        }
        out.extend(std::iter::repeat_n((v, 0), run));
    }
    out
}

fn block(w: &mut BitWriter, tokens: &[Token], last: bool) {
    let mut lit_freq = [0u32; 286];
    let mut dist_freq = [0u32; 30];
    for t in tokens {
        if t.dist == 0 {
            lit_freq[usize::from(t.value)] += 1;
        } else {
            lit_freq[257 + length_code(usize::from(t.value))] += 1;
            dist_freq[dist_code(usize::from(t.dist))] += 1;
        }
    }
    lit_freq[256] += 1;
    let lit_len = huffman_lengths(&lit_freq, 15);
    let dist_len = huffman_lengths(&dist_freq, 15);
    let n_lit = (257..286).rev().find(|&s| lit_len[s] > 0).map_or(257, |s| s + 1);
    let n_dist = (1..30).rev().find(|&s| dist_len[s] > 0).map_or(1, |s| s + 1);

    let all: Vec<u8> = lit_len[..n_lit].iter().chain(&dist_len[..n_dist]).copied().collect();
    let runs = run_length_code(&all);
    let mut cl_freq = [0u32; 19];
    for &(symbol, _) in &runs {
        cl_freq[usize::from(symbol)] += 1;
    }
    let cl_len = huffman_lengths(&cl_freq, 7);
    let n_cl = (4..19).rev().find(|&k| cl_len[CODE_LENGTH_ORDER[k]] > 0).map_or(4, |k| k + 1);

    let lit_codes = huffman_codes(&lit_len);
    let dist_codes = huffman_codes(&dist_len);
    let cl_codes = huffman_codes(&cl_len);

    w.put(u64::from(last), 1);
    w.put(2, 2);
    w.put((n_lit - 257) as u64, 5);
    w.put((n_dist - 1) as u64, 5);
    w.put((n_cl - 4) as u64, 4);
    for &symbol in &CODE_LENGTH_ORDER[..n_cl] {
        w.put(u64::from(cl_len[symbol]), 3);
    }
    for &(symbol, extra) in &runs {
        let s = usize::from(symbol);
        w.put(u64::from(cl_codes[s]), u32::from(cl_len[s]));
        match symbol {
            16 => w.put(u64::from(extra), 2),
            17 => w.put(u64::from(extra), 3),
            18 => w.put(u64::from(extra), 7),
            _ => {}
        }
    }
    for t in tokens {
        if t.dist == 0 {
            let s = usize::from(t.value);
            w.put(u64::from(lit_codes[s]), u32::from(lit_len[s]));
            continue;
        }
        let lc = length_code(usize::from(t.value));
        w.put(u64::from(lit_codes[257 + lc]), u32::from(lit_len[257 + lc]));
        w.put(u64::from(t.value - LENGTH_BASE[lc]), u32::from(LENGTH_EXTRA[lc]));
        let dc = dist_code(usize::from(t.dist));
        w.put(u64::from(dist_codes[dc]), u32::from(dist_len[dc]));
        w.put(u64::from(t.dist - DIST_BASE[dc]), u32::from(DIST_EXTRA[dc]));
    }
    w.put(u64::from(lit_codes[256]), u32::from(lit_len[256]));
}

/// `data` as one raw DEFLATE stream (no zlib header), byte-identical to the
/// Odin engine's `deflate_compress`.
pub fn deflate(data: &[u8]) -> Vec<u8> {
    let tokens = tokens(data);
    let mut w = BitWriter {
        out: Vec::with_capacity(data.len() / 2 + 64),
        ..BitWriter::default()
    };
    if tokens.is_empty() {
        block(&mut w, &[], true);
    }
    let chunks = tokens.chunks(BLOCK_TOKENS);
    let count = chunks.len();
    for (index, chunk) in chunks.enumerate() {
        block(&mut w, chunk, index + 1 == count);
    }
    if w.n > 0 {
        w.out.push(w.acc as u8);
    }
    w.out
}

/// A raw DEFLATE stream's bytes (any conforming writer's).
pub fn inflate(data: &[u8]) -> Option<Vec<u8>> {
    miniz_oxide::inflate::decompress_to_vec(data).ok()
}
