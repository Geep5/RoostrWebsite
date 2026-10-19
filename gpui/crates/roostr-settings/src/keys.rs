//! A fresh Nostr secret key as `nsec1…` (KeyGate's "Create a new account":
//! `nip19.nsecEncode(generateSecretKey())`).

use anyhow::{Result, anyhow};

const CHARSET: &[u8; 32] = b"qpzry9x8gf2tvdw0s3jn54khce6mua7l";
/// secp256k1 group order: a secret key must be in `1..N`.
const ORDER: [u8; 32] = [
    0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xfe,
    0xba, 0xae, 0xdc, 0xe6, 0xaf, 0x48, 0xa0, 0x3b, 0xbf, 0xd2, 0x5e, 0x8c, 0xd0, 0x36, 0x41, 0x41,
];

fn polymod(values: &[u8]) -> u32 {
    const GEN: [u32; 5] = [
        0x3b6a_57b2,
        0x2650_8e6d,
        0x1ea1_19fa,
        0x3d42_33dd,
        0x2a14_62b3,
    ];
    let mut chk: u32 = 1;
    for &v in values {
        let top = chk >> 25;
        chk = ((chk & 0x01ff_ffff) << 5) ^ v as u32;
        for (i, g) in GEN.iter().enumerate() {
            if (top >> i) & 1 == 1 {
                chk ^= g;
            }
        }
    }
    chk
}

/// Bech32 (BIP-173) encoding of `data` under `hrp`.
fn bech32(hrp: &str, data: &[u8]) -> String {
    // 8-bit -> 5-bit groups, padded.
    let mut words = Vec::with_capacity(data.len() * 8 / 5 + 1);
    let (mut acc, mut bits) = (0u32, 0u32);
    for &b in data {
        acc = (acc << 8) | b as u32;
        bits += 8;
        while bits >= 5 {
            bits -= 5;
            words.push(((acc >> bits) & 31) as u8);
        }
    }
    if bits > 0 {
        words.push(((acc << (5 - bits)) & 31) as u8);
    }
    let mut values: Vec<u8> = hrp.bytes().map(|c| c >> 5).collect();
    values.push(0);
    values.extend(hrp.bytes().map(|c| c & 31));
    values.extend(&words);
    values.extend([0u8; 6]);
    let checksum = polymod(&values) ^ 1;
    let mut out = String::with_capacity(hrp.len() + 1 + words.len() + 6);
    out.push_str(hrp);
    out.push('1');
    for w in words {
        out.push(CHARSET[w as usize] as char);
    }
    for i in 0..6 {
        out.push(CHARSET[((checksum >> (5 * (5 - i))) & 31) as usize] as char);
    }
    out
}

/// A new random secret key, `nsec1…`.
pub fn generate_nsec() -> Result<String> {
    loop {
        let mut secret = [0u8; 32];
        getrandom::fill(&mut secret).map_err(|e| anyhow!("no randomness: {e}"))?;
        if secret.iter().any(|&b| b != 0) && secret < ORDER {
            return Ok(bech32("nsec", &secret));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_nip19_vector() {
        // NIP-19 test vector.
        let hex = "67dea2ed018072d675f5415ecfaed7d2597555e202d85b3d65ea4e58d2d92ffa";
        let bytes: Vec<u8> = (0..32)
            .map(|i| u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).unwrap())
            .collect();
        assert_eq!(
            bech32("nsec", &bytes),
            "nsec1vl029mgpspedva04g90vltkh6fvh240zqtv9k0t9af8935ke9laqsnlfe5"
        );
    }

    #[test]
    fn generates_valid_keys() {
        let key = generate_nsec().unwrap();
        assert!(key.starts_with("nsec1"));
        assert_eq!(key.len(), 63);
    }
}
