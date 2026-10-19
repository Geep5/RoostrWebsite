//! BIP-173 bech32, as NIP-19 (`npub1…`, `nsec1…`) and nostr-tools' `nip19`
//! use it: the original bech32 checksum (not bech32m), lowercase output,
//! mixed case rejected, strings up to 5000 characters.

use std::fmt;

const CHARSET: &[u8; 32] = b"qpzry9x8gf2tvdw0s3jn54khce6mua7l";
/// nostr-tools' `Bech32MaxSize`.
const MAX_LENGTH: usize = 5000;
const CHECKSUM_LENGTH: usize = 6;

/// Why a string is not bech32.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Bech32Error {
    /// Longer than 5000 characters, or too short to hold a checksum.
    Length,
    /// Both upper- and lowercase letters.
    MixedCase,
    /// No `1` separator, or an empty human-readable part.
    Separator,
    /// A character outside the human-readable or data alphabet.
    Character,
    /// The checksum does not match.
    Checksum,
    /// The 5-bit groups do not regroup into whole bytes.
    Padding,
}

impl fmt::Display for Bech32Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Length => "invalid bech32 length",
            Self::MixedCase => "bech32 mixes upper and lower case",
            Self::Separator => "bech32 has no separator",
            Self::Character => "invalid bech32 character",
            Self::Checksum => "invalid bech32 checksum",
            Self::Padding => "invalid bech32 padding",
        })
    }
}

impl std::error::Error for Bech32Error {}

fn polymod(values: impl IntoIterator<Item = u8>) -> u32 {
    const GENERATOR: [u32; 5] = [
        0x3b6a_57b2,
        0x2650_8e6d,
        0x1ea1_19fa,
        0x3d42_33dd,
        0x2a14_62b3,
    ];
    let mut chk: u32 = 1;
    for value in values {
        let top = chk >> 25;
        chk = ((chk & 0x01ff_ffff) << 5) ^ u32::from(value);
        for (i, g) in GENERATOR.iter().enumerate() {
            if (top >> i) & 1 == 1 {
                chk ^= g;
            }
        }
    }
    chk
}

fn hrp_expand(hrp: &[u8]) -> impl Iterator<Item = u8> + '_ {
    hrp.iter()
        .map(|c| c >> 5)
        .chain(std::iter::once(0))
        .chain(hrp.iter().map(|c| c & 31))
}

/// Regroup `data` from `from`-bit to `to`-bit groups. Encoding pads the
/// last group; decoding refuses leftover bits that are not zero padding.
fn convert_bits(data: &[u8], from: u32, to: u32, pad: bool) -> Result<Vec<u8>, Bech32Error> {
    let mut acc: u32 = 0;
    let mut bits: u32 = 0;
    let max = (1u32 << to) - 1;
    let mut out = Vec::with_capacity(data.len() * from as usize / to as usize + 1);
    for &value in data {
        acc = (acc << from) | u32::from(value);
        bits += from;
        while bits >= to {
            bits -= to;
            out.push(((acc >> bits) & max) as u8);
        }
    }
    if pad {
        if bits > 0 {
            out.push(((acc << (to - bits)) & max) as u8);
        }
    } else if bits >= from || (acc << (to - bits)) & max != 0 {
        return Err(Bech32Error::Padding);
    }
    Ok(out)
}

/// `hrp` + `1` + `data` in 5-bit groups + checksum. `hrp` must be lowercase
/// ASCII (NIP-19 prefixes are).
pub fn encode(hrp: &str, data: &[u8]) -> String {
    let hrp_bytes = hrp.as_bytes();
    let words = convert_bits(data, 8, 5, true).expect("padding never fails");
    let checksum = {
        let pm = polymod(
            hrp_expand(hrp_bytes)
                .chain(words.iter().copied())
                .chain([0u8; CHECKSUM_LENGTH]),
        ) ^ 1;
        (0..CHECKSUM_LENGTH).map(move |i| ((pm >> (5 * (5 - i))) & 31) as u8)
    };
    let mut out = String::with_capacity(hrp.len() + 1 + words.len() + CHECKSUM_LENGTH);
    out.push_str(hrp);
    out.push('1');
    for w in words.iter().copied().chain(checksum) {
        out.push(CHARSET[w as usize] as char);
    }
    out
}

/// The lowercase human-readable part and the data bytes of a bech32 string.
pub fn decode(text: &str) -> Result<(String, Vec<u8>), Bech32Error> {
    if text.len() > MAX_LENGTH {
        return Err(Bech32Error::Length);
    }
    let bytes = text.as_bytes();
    if bytes.iter().any(|c| !(33..=126).contains(c)) {
        return Err(Bech32Error::Character);
    }
    let has_lower = bytes.iter().any(u8::is_ascii_lowercase);
    let has_upper = bytes.iter().any(u8::is_ascii_uppercase);
    if has_lower && has_upper {
        return Err(Bech32Error::MixedCase);
    }
    let lower = text.to_ascii_lowercase();
    let split = lower.rfind('1').ok_or(Bech32Error::Separator)?;
    if split == 0 {
        return Err(Bech32Error::Separator);
    }
    let (hrp, rest) = (&lower[..split], &lower.as_bytes()[split + 1..]);
    if rest.len() < CHECKSUM_LENGTH {
        return Err(Bech32Error::Length);
    }
    let words = rest
        .iter()
        .map(|c| CHARSET.iter().position(|x| x == c).map(|p| p as u8))
        .collect::<Option<Vec<u8>>>()
        .ok_or(Bech32Error::Character)?;
    if polymod(hrp_expand(hrp.as_bytes()).chain(words.iter().copied())) != 1 {
        return Err(Bech32Error::Checksum);
    }
    let data = convert_bits(&words[..words.len() - CHECKSUM_LENGTH], 5, 8, false)?;
    Ok((hrp.to_string(), data))
}

/// The 32 bytes behind `hrp1…`, as hex; `None` for any other prefix or size.
pub fn decode_hex32(text: &str, hrp: &str) -> Option<String> {
    match decode(text) {
        Ok((found, data)) if found == hrp && data.len() == 32 => Some(hex::encode(data)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bip173_valid_strings_round_trip() {
        let long = format!("11{}c8247j", "q".repeat(82));
        for text in [
            "A12UEL5L",
            "a12uel5l",
            "an83characterlonghumanreadablepartthatcontainsthenumber1andtheexcludedcharactersbio1tt5tgs",
            "abcdef1qpzry9x8gf2tvdw0s3jn54khce6mua7lmqqqxw",
            &long,
            "split1checkupstagehandshakeupstreamerranterredcaperred2y9e3w",
        ] {
            let (hrp, data) = decode(text).unwrap_or_else(|e| panic!("{text}: {e}"));
            assert_eq!(encode(&hrp, &data), text.to_ascii_lowercase());
        }
    }

    #[test]
    fn bip173_invalid_strings() {
        // BIP-173's 90-character cap is not applied: nostr-tools allows 5000.
        for text in [
            "\u{20}1nwldj5",
            "\u{7f}1axkwrx",
            "pzry9x0s0muk",
            "1pzry9x0s0muk",
            "x1b4n0q5v",
            "li1dgmt3",
            "de1lg7wt\u{ff}",
            "A1G7SGD8",
            "10a06t8",
            "1qzzfhee",
            "a12UEL5L",
        ] {
            assert!(decode(text).is_err(), "{text} must not decode");
        }
    }
}
