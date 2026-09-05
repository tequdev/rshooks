//! Host-side base58check decoding of a classic XRPL/Xahau r-address into
//! a validated 20-byte AccountID, entirely at proc-macro compile time (used
//! by [`crate::account_id`]).
//!
//! Not a `bs58` dependency — avoids adding mandatory build-time weight to
//! every Hook crate, same as [`crate::sha256`].

use crate::sha256::sha256;

/// XRPL's base58 alphabet — NOT Bitcoin's. Same 58 symbols, different
/// character order; index 0 (`'r'`) is this alphabet's "zero" symbol
/// (XRPL's analogue of Bitcoin base58's leading `'1'`).
const ALPHABET: &[u8; 58] = b"rpshnaf39wBUDNEGHJKLM4PQRST7VWXYZ2bcdeCg65jkm8oFqi1tuvAxyz";

/// Length of a base58check-decoded r-address: 1 version byte + 20-byte
/// AccountID payload + 4-byte checksum.
const DECODED_LEN: usize = 25;

/// Why a classic r-address failed to decode into a valid 20-byte AccountID.
/// Each variant's [`DecodeError::message`] becomes the `compile_error!` text
/// at the `account_id!` call site, so each one names exactly what was wrong.
#[cfg_attr(test, derive(Debug))]
pub(crate) enum DecodeError {
    /// A character outside the XRPL base58 alphabet.
    InvalidChar(char),
    /// The decoded byte length wasn't exactly 25 (version + payload +
    /// checksum).
    WrongLength(usize),
    /// `decoded[0]` wasn't `0x00` (the classic AccountID version byte).
    WrongVersion(u8),
    /// The double-SHA256 checksum didn't match the trailing 4 bytes.
    ChecksumMismatch,
}

impl DecodeError {
    /// A human-readable, actionable message describing this failure.
    pub(crate) fn message(&self) -> String {
        match self {
            DecodeError::InvalidChar(c) => {
                format!("account_id!: '{c}' is not a valid XRPL base58 character")
            }
            DecodeError::WrongLength(len) => format!(
                "account_id!: decoded address is {len} bytes, expected 25 \
                 (1 version byte + 20-byte AccountID + 4-byte checksum)"
            ),
            DecodeError::WrongVersion(v) => format!(
                "account_id!: decoded address has version byte 0x{v:02X}, \
                 expected 0x00 (classic AccountID)"
            ),
            DecodeError::ChecksumMismatch => {
                "account_id!: base58check checksum does not match — the address \
                 has a typo somewhere"
                    .to_string()
            }
        }
    }
}

/// Decodes a classic XRPL/Xahau r-address string into its 20-byte
/// AccountID, verifying length, version byte, and checksum along the way.
///
/// XRPL-alphabet base58 decode, then the standard base58check length /
/// version / double-SHA256-checksum checks.
// `decoded[0]`/`decoded[0..21]`/`decoded[21..25]`/`decoded[1..21]` are all
// only reached after the `decoded.len() != DECODED_LEN` (25) check above
// returns early, so every index/slice here is in bounds by construction.
#[allow(clippy::indexing_slicing)]
pub(crate) fn decode(address: &str) -> Result<[u8; 20], DecodeError> {
    let decoded = base58_decode(address)?;

    if decoded.len() != DECODED_LEN {
        return Err(DecodeError::WrongLength(decoded.len()));
    }

    if decoded[0] != 0x00 {
        return Err(DecodeError::WrongVersion(decoded[0]));
    }

    let checksum = sha256(&sha256(&decoded[0..21]));
    if checksum[0..4] != decoded[21..25] {
        return Err(DecodeError::ChecksumMismatch);
    }

    let mut account_id = [0u8; 20];
    account_id.copy_from_slice(&decoded[1..21]);
    Ok(account_id)
}

/// Plain base58 decode (XRPL alphabet), with leading zero-symbol (`'r'`)
/// characters becoming leading zero bytes in the output — no length or
/// checksum validation, that's [`decode`]'s job.
// `ALPHABET[0]` is a constant index into a fixed 58-element array — always
// in bounds.
#[allow(clippy::indexing_slicing)]
fn base58_decode(s: &str) -> Result<Vec<u8>, DecodeError> {
    // Accumulator built by repeated multiply-by-58-and-add, stored
    // little-endian while accumulating, reversed to big-endian at the end.
    let mut num: Vec<u8> = Vec::new();

    for ch in s.chars() {
        let idx = ALPHABET
            .iter()
            .position(|&b| b as char == ch)
            .ok_or(DecodeError::InvalidChar(ch))?;

        let mut carry = idx as u32;
        for byte in num.iter_mut() {
            carry += (*byte as u32) * 58;
            *byte = (carry & 0xff) as u8;
            carry >>= 8;
        }
        while carry > 0 {
            num.push((carry & 0xff) as u8);
            carry >>= 8;
        }
    }

    num.reverse();

    let leading_zeros = s
        .chars()
        .take_while(|&ch| ch == ALPHABET[0] as char)
        .count();
    let mut decoded = vec![0u8; leading_zeros];
    decoded.extend_from_slice(&num);
    Ok(decoded)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used, clippy::indexing_slicing)] // tests are exempt from panic-freedom lints, docs/DESIGN.md §8

    use super::*;

    /// Test-only base58 (XRPL alphabet) encode, used to cross-check
    /// [`base58_decode`] via a round trip — independent of the fixed
    /// expected-hex assertions below.
    fn base58_encode(bytes: &[u8]) -> String {
        let leading_zeros = bytes.iter().take_while(|&&b| b == 0).count();

        // Big-integer-by-repeated-division-by-58, base256 input.
        let mut digits: Vec<u8> = Vec::new();
        for &byte in bytes {
            let mut carry = byte as u32;
            for digit in digits.iter_mut() {
                carry += (*digit as u32) << 8;
                *digit = (carry % 58) as u8;
                carry /= 58;
            }
            while carry > 0 {
                digits.push((carry % 58) as u8);
                carry /= 58;
            }
        }

        let mut out = String::new();
        for _ in 0..leading_zeros {
            out.push(ALPHABET[0] as char);
        }
        for &d in digits.iter().rev() {
            out.push(ALPHABET[d as usize] as char);
        }
        out
    }

    fn hex_to_20(hex: &str) -> [u8; 20] {
        assert_eq!(hex.len(), 40, "test vector AccountID must be 40 hex chars");
        let mut out = [0u8; 20];
        for i in 0..20 {
            out[i] =
                u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).expect("valid hex in test vector");
        }
        out
    }

    #[test]
    fn genesis_master_account() {
        // Genesis/master account (seed "masterpassphrase"), also hardcoded
        // as `GENESIS_ACCOUNT` in examples/80_governance.
        assert_eq!(
            decode("rHb9CJAWyB4rj91VRWn96DkukG4bwdtyTh").ok(),
            Some(hex_to_20("b5f762798a53d543a014caf8b297cff8f2f937e8"))
        );
    }

    #[test]
    fn account_zero() {
        assert_eq!(decode("rrrrrrrrrrrrrrrrrrrrrhoLvTp").ok(), Some([0u8; 20]));
    }

    #[test]
    fn account_one() {
        let mut expected = [0u8; 20];
        expected[19] = 1;
        assert_eq!(decode("rrrrrrrrrrrrrrrrrrrrBZbvji").ok(), Some(expected));
    }

    #[test]
    fn checksum_mismatch() {
        // Last char 'h' -> 'H': valid chars/length/version, bad checksum.
        assert!(matches!(
            decode("rHb9CJAWyB4rj91VRWn96DkukG4bwdtyTH"),
            Err(DecodeError::ChecksumMismatch)
        ));
    }

    #[test]
    fn wrong_version() {
        // Well-formed base58check string with version byte 5, not 0.
        assert!(matches!(
            decode("sJHw2iRxXngPFKZvYbjkfifqt8CJghksMM"),
            Err(DecodeError::WrongVersion(5))
        ));
    }

    #[test]
    fn wrong_length_too_short() {
        // Truncated by 2 chars: decodes to fewer than 25 bytes.
        assert!(matches!(
            decode("rHb9CJAWyB4rj91VRWn96DkukG4bwdty"),
            Err(DecodeError::WrongLength(_))
        ));
    }

    #[test]
    fn invalid_char() {
        // '0' is not in the XRPL base58 alphabet.
        assert!(matches!(
            decode("rHb9CJAWyB4rj91VRWn96DkukG4bwdtyT0"),
            Err(DecodeError::InvalidChar('0'))
        ));
    }

    #[test]
    fn encode_decode_round_trip() {
        let address = "rHb9CJAWyB4rj91VRWn96DkukG4bwdtyTh";
        let decoded = base58_decode(address).expect("valid base58");
        assert_eq!(decoded.len(), DECODED_LEN);
        assert_eq!(base58_encode(&decoded), address);
    }
}
