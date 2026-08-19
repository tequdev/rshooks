//! `util_sha512h`/`util_accid`/`util_raddr`/`util_verify` semantics (P2-C —
//! `.claude/design/TESTENV_PHASE2_DESIGN.md` §4 "util_* and ledger_keylet",
//! stage plan §7). `util_keylet` itself lives in [`super::keylet`], not here
//! (§0's family table lists it under "util (5)", but the implementation is
//! large enough on its own, 26 keylet types plus
//! [`rshooks_core::backend::KeyletArg`] resolution, to warrant its own
//! module). `ledger_keylet` needs [`crate::world::World`] access (a seeded
//! ledger-object search) this module's other functions don't, so it lives
//! directly in `crate::backend`'s `impl HostBackend for Backend` block —
//! see that file's own module doc comment.
//!
//! Every function here is a direct port of xahaud's own implementation
//! (`Xahau/xahaud`, branch `dev`), not a reinterpretation of the `hook-api`
//! skill's prose summary — sources consulted:
//!
//! - `src/xrpld/app/hook/detail/HookAPI.cpp` (`HookAPI::util_raddr`/
//!   `util_accid`/`util_verify`/`util_sha512h`) — every function's own
//!   validation order and error codes.
//! - `src/libxrpl/protocol/PublicKey.cpp` (`ripple::verify`, `publicKeyType`,
//!   `ed25519Canonical`) — [`util_verify`]'s exact digest/key-type dispatch,
//!   confirmed from source rather than assumed: `HookAPI::util_verify` calls
//!   `ripple::verify(pubkey, data, sig, /* mustBeFullyCanonical */ false)`,
//!   which for a `0x02`/`0x03`-prefixed (secp256k1) key verifies the
//!   **SHA-512-Half of `data`** (`sha512Half(m)`, i.e. exactly
//!   [`util_sha512h`]'s own primitive applied to the payload — not the raw
//!   payload, and not a hash of the signature), and for a `0xED`-prefixed
//!   (ed25519) key verifies the **raw, unhashed `data`** directly (Ed25519
//!   already hashes internally as part of the algorithm). `false` for
//!   `mustBeFullyCanonical` means both "canonical" and "fully canonical"
//!   secp256k1 signature forms (see `ecdsaCanonicality` in `PublicKey.cpp`)
//!   are accepted — ECDSA's `(R, S)`/`(R, G-S)` malleable-pair symmetry means
//!   a plain, unmodified verify against the parsed signature already accepts
//!   both forms with no extra normalization step needed (this port relies on
//!   that standard ECDSA property rather than reimplementing
//!   `secp256k1_ecdsa_signature_normalize`).
//! - `rshooks_macros::base58` (`crates/rshooks-macros/src/base58.rs`) — the
//!   XRPL base58 alphabet and base58check decode algorithm, copied here
//!   (not depended on: that crate is proc-macro-only, compile-time-only, and
//!   cannot be a runtime dependency of this crate — see that module's own
//!   doc comment for the same reasoning applied to `account_id!`). The
//!   encode direction (needed here for [`util_raddr`], which that
//!   proc-macro crate has no need for) is new, but uses the identical
//!   alphabet/checksum convention.

use std::string::String;
use std::vec::Vec;

use ed25519_dalek::{Signature as Ed25519Signature, Verifier, VerifyingKey as Ed25519VerifyingKey};
use k256::ecdsa::signature::hazmat::PrehashVerifier;
use k256::ecdsa::{Signature as Secp256k1Signature, VerifyingKey as Secp256k1VerifyingKey};
use sha2::{Digest, Sha256, Sha512};

use rshooks_core::{INVALID_ARGUMENT, INVALID_KEY, TOO_BIG, TOO_SMALL};

// ---------------------------------------------------------------------
// `util_sha512h` (`HookAPI::util_sha512h` -> `ripple::sha512Half`).
// ---------------------------------------------------------------------

/// SHA-512-Half: the first 32 bytes of a SHA-512 digest. The one hashing
/// primitive every other function in this module (and `super::keylet`)
/// builds on — `ripple::sha512Half` throughout xahaud.
#[allow(clippy::indexing_slicing)] // `full` is a SHA-512 digest (`GenericArray<u8, U64>`), always exactly 64 bytes, so `full[..32]` is always in bounds
pub(crate) fn sha512_half(data: &[u8]) -> [u8; 32] {
    let mut hasher = Sha512::new();
    hasher.update(data);
    let full = hasher.finalize();
    let mut out = [0u8; 32];
    out.copy_from_slice(&full[..32]);
    out
}

pub(crate) fn util_sha512h(data: &[u8]) -> [u8; 32] {
    sha512_half(data)
}

// ---------------------------------------------------------------------
// `util_accid`/`util_raddr` (`HookAPI::util_accid`/`util_raddr` ->
// `decodeBase58Token`/`encodeBase58Token(TokenType::AccountID)`).
// ---------------------------------------------------------------------

/// XRPL's base58 alphabet — NOT Bitcoin's (same 58 symbols, different
/// order). Copied verbatim from `rshooks_macros::base58::ALPHABET` — see
/// this module's doc comment for why that crate can't be a runtime
/// dependency here.
const ALPHABET: &[u8; 58] = b"rpshnaf39wBUDNEGHJKLM4PQRST7VWXYZ2bcdeCg65jkm8oFqi1tuvAxyz";

/// 1 version byte + 20-byte `AccountID` payload + 4-byte checksum.
const DECODED_LEN: usize = 25;

/// Plain base58 decode (XRPL alphabet) — no length/checksum validation,
/// that's [`util_accid`]'s job. Ported from
/// `rshooks_macros::base58::base58_decode`.
// `carry` accumulates a base-256 digit (`u8` promoted to `u32`) times the
// constant 58 plus a byte already known `< 256` — bounded well below
// `u32::MAX` for any realistic input length (this function is only ever
// called on a `util_accid` argument already capped at 49 bytes); matches
// `rshooks_macros::base58::base58_decode`'s own unannotated arithmetic
// (that proc-macro crate runs under a different, host-only lint profile).
#[allow(clippy::arithmetic_side_effects)]
fn base58_decode(s: &str) -> Option<Vec<u8>> {
    let mut num: Vec<u8> = Vec::new();

    for ch in s.chars() {
        let idx = ALPHABET.iter().position(|&b| b as char == ch)?;

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
    let mut decoded = std::vec![0u8; leading_zeros];
    decoded.extend_from_slice(&num);
    Some(decoded)
}

/// XRPL base58 encode (big-integer-by-repeated-division-by-58), the inverse
/// of [`base58_decode`]. Not present in `rshooks_macros::base58` (that
/// proc-macro crate only ever needs to decode a source-code literal) — new
/// here, but the same alphabet/convention.
// `carry` is bounded the same way as `base58_decode`'s (only ever called on
// a 20-byte `AccountID`); `ALPHABET[d as usize]` is safe because `d` is
// always `carry % 58`, so `< 58 == ALPHABET.len()`.
#[allow(clippy::arithmetic_side_effects, clippy::indexing_slicing)]
fn base58_encode(bytes: &[u8]) -> String {
    let leading_zeros = bytes.iter().take_while(|&&b| b == 0).count();

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

/// `HookAPI::util_accid` (`decodeBase58Token(raddress, TokenType::
/// AccountID)`; empty result -> `INVALID_ARGUMENT`) plus the wasm-facing
/// wrapper's own `read_len > 49 -> TOO_BIG` bound (`hook-api` skill's
/// `utility.md`; not shown in the `HookAPI::` C++ snippet itself, which
/// operates on an already-bounded `std::string`, but this backend receives
/// the raw resolved bytes before any such bound has been applied, so it is
/// enforced here).
// `decoded[0]`/`decoded[0..21]`/`decoded[21..25]`/`decoded[1..21]` are all
// only reached after the `decoded.len() != DECODED_LEN` (25) check
// immediately above returns early — every index/slice below is in bounds
// by construction (mirrors `rshooks_macros::base58::decode`'s identical
// reasoning for the same checks).
#[allow(clippy::indexing_slicing)]
pub(crate) fn util_accid(r_address: &[u8]) -> Result<Vec<u8>, i64> {
    if r_address.len() > 49 {
        return Err(TOO_BIG);
    }
    let text = std::str::from_utf8(r_address).map_err(|_| INVALID_ARGUMENT)?;
    let decoded = base58_decode(text).ok_or(INVALID_ARGUMENT)?;

    if decoded.len() != DECODED_LEN {
        return Err(INVALID_ARGUMENT);
    }
    if decoded[0] != 0x00 {
        return Err(INVALID_ARGUMENT);
    }
    let checksum = sha256(&sha256(&decoded[0..21]));
    if checksum[0..4] != decoded[21..25] {
        return Err(INVALID_ARGUMENT);
    }
    Ok(decoded[1..21].to_vec())
}

/// `HookAPI::util_raddr` (`accountID.size() != 20 -> INVALID_ARGUMENT`, then
/// `encodeBase58Token(TokenType::AccountID, ...)`).
pub(crate) fn util_raddr(accid: &[u8]) -> Result<Vec<u8>, i64> {
    if accid.len() != 20 {
        return Err(INVALID_ARGUMENT);
    }
    let mut payload = Vec::with_capacity(DECODED_LEN);
    payload.push(0x00u8);
    payload.extend_from_slice(accid);
    let checksum = sha256(&sha256(&payload));
    payload.extend_from_slice(&checksum[0..4]);
    Ok(base58_encode(&payload).into_bytes())
}

fn sha256(data: &[u8]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(data);
    hasher.finalize().into()
}

// ---------------------------------------------------------------------
// `util_verify` (`HookAPI::util_verify` -> `ripple::verify`) — see the
// module doc comment for the digest-choice citation.
// ---------------------------------------------------------------------

/// Big-endian Ed25519 subgroup order `L`, verbatim from `PublicKey.cpp`'s
/// `ed25519Canonical` — the malleability check xahaud applies to an Ed25519
/// signature's `S` component *before* calling into the actual Ed25519
/// verify routine (so a non-canonical `S` is rejected even if the
/// underlying primitive would otherwise accept it).
const ED25519_ORDER_BE: [u8; 32] = [
    0x10, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x14, 0xDE, 0xF9, 0xDE, 0xA2, 0xF7, 0x9C, 0xD6, 0x58, 0x12, 0x63, 0x1A, 0x5C, 0xF5, 0xD3, 0xED,
];

/// `PublicKey.cpp::ed25519Canonical`: `sig` must be exactly 64 bytes, and
/// its `S` half (`sig[32..64]`, stored little-endian, reversed to
/// big-endian here) must be strictly less than [`ED25519_ORDER_BE`].
// `sig[32..64]` is reached only after the `sig.len() != 64` check just
// above; `s_be[i]` is safe because `i` ranges over `sig[32..64]`'s own 32
// elements via `.enumerate()`, so `i` is always `< 32 == s_be.len()`.
#[allow(clippy::indexing_slicing)]
fn ed25519_canonical(sig: &[u8]) -> bool {
    if sig.len() != 64 {
        return false;
    }
    let mut s_be = [0u8; 32];
    for (i, &b) in sig[32..64].iter().rev().enumerate() {
        s_be[i] = b;
    }
    s_be < ED25519_ORDER_BE
}

/// `HookAPI::util_verify`: key-length/data/signature bound checks (order
/// matters — matches `HookAPI.cpp`'s own sequence), then dispatch by key
/// prefix (`ripple::publicKeyType`).
pub(crate) fn util_verify(data: &[u8], signature: &[u8], public_key: &[u8]) -> i64 {
    if public_key.len() != 33 {
        return INVALID_KEY;
    }
    if data.is_empty() {
        return TOO_SMALL;
    }
    if signature.len() < 30 {
        return TOO_SMALL;
    }

    match public_key.first() {
        Some(0xED) => i64::from(verify_ed25519(data, signature, public_key)),
        Some(0x02 | 0x03) => i64::from(verify_secp256k1(data, signature, public_key)),
        _ => INVALID_KEY,
    }
}

/// Ed25519 branch of `ripple::verify`: canonical-`S` check, then verify the
/// **raw, unhashed** `data` against the key with its `0xED` prefix
/// stripped.
fn verify_ed25519(data: &[u8], signature: &[u8], public_key: &[u8]) -> bool {
    if !ed25519_canonical(signature) {
        return false;
    }
    let Some(key_slice) = public_key.get(1..33) else {
        return false;
    };
    let Ok(key_bytes) = <[u8; 32]>::try_from(key_slice) else {
        return false;
    };
    let Ok(verifying_key) = Ed25519VerifyingKey::from_bytes(&key_bytes) else {
        return false;
    };
    let Ok(sig_bytes) = <[u8; 64]>::try_from(signature) else {
        return false;
    };
    let sig = Ed25519Signature::from_bytes(&sig_bytes);
    verifying_key.verify(data, &sig).is_ok()
}

/// secp256k1 branch of `ripple::verify`: DER-parse the signature (a parse
/// failure returns `false`, not an error — see the module doc comment), then
/// ECDSA-verify it against `sha512Half(data)` (not `data` itself).
/// `mustBeFullyCanonical = false` needs no explicit low-`S` normalization
/// here — see the module doc comment for why a plain verify already accepts
/// both malleable forms.
fn verify_secp256k1(data: &[u8], signature: &[u8], public_key: &[u8]) -> bool {
    let Ok(verifying_key) = Secp256k1VerifyingKey::from_sec1_bytes(public_key) else {
        return false;
    };
    let Ok(sig) = Secp256k1Signature::from_der(signature) else {
        return false;
    };
    let digest = sha512_half(data);
    verifying_key.verify_prehash(&digest, &sig).is_ok()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::indexing_slicing)] // tests are exempt, docs/DESIGN.md §8

    use super::*;

    // -- util_sha512h --

    #[test]
    fn sha512h_known_answer_empty_input() {
        // SHA-512("") first 32 bytes, RFC/NIST known-answer vector.
        let expected = [
            0xcf, 0x83, 0xe1, 0x35, 0x7e, 0xef, 0xb8, 0xbd, 0xf1, 0x54, 0x28, 0x50, 0xd6, 0x6d,
            0x80, 0x07, 0xd6, 0x20, 0xe4, 0x05, 0x0b, 0x57, 0x15, 0xdc, 0x83, 0xf4, 0xa9, 0x21,
            0xd3, 0x6c, 0xe9, 0xce,
        ];
        assert_eq!(util_sha512h(b""), expected);
    }

    #[test]
    fn sha512h_known_answer_abc() {
        // SHA-512("abc") first 32 bytes, RFC/NIST known-answer vector.
        let expected = [
            0xdd, 0xaf, 0x35, 0xa1, 0x93, 0x61, 0x7a, 0xba, 0xcc, 0x41, 0x73, 0x49, 0xae, 0x20,
            0x41, 0x31, 0x12, 0xe6, 0xfa, 0x4e, 0x89, 0xa9, 0x7e, 0xa2, 0x0a, 0x9e, 0xee, 0xe6,
            0x4b, 0x55, 0xd3, 0x9a,
        ];
        assert_eq!(util_sha512h(b"abc"), expected);
    }

    // -- util_accid/util_raddr round trip --

    const GENESIS_RADDR: &str = "rHb9CJAWyB4rj91VRWn96DkukG4bwdtyTh";
    const GENESIS_ACCID: [u8; 20] = [
        0xb5, 0xf7, 0x62, 0x79, 0x8a, 0x53, 0xd5, 0x43, 0xa0, 0x14, 0xca, 0xf8, 0xb2, 0x97, 0xcf,
        0xf8, 0xf2, 0xf9, 0x37, 0xe8,
    ];

    #[test]
    fn util_accid_genesis_account() {
        assert_eq!(
            util_accid(GENESIS_RADDR.as_bytes()),
            Ok(GENESIS_ACCID.to_vec())
        );
    }

    #[test]
    fn util_raddr_genesis_account() {
        assert_eq!(
            util_raddr(&GENESIS_ACCID),
            Ok(GENESIS_RADDR.as_bytes().to_vec())
        );
    }

    #[test]
    fn util_accid_util_raddr_round_trip_arbitrary_bytes() {
        let accid = [0x42u8; 20];
        let raddr = util_raddr(&accid).unwrap();
        assert_eq!(util_accid(&raddr), Ok(accid.to_vec()));
    }

    #[test]
    fn util_accid_bad_checksum() {
        // Last char 'h' -> 'H': valid alphabet/length/version, bad checksum.
        assert_eq!(
            util_accid(b"rHb9CJAWyB4rj91VRWn96DkukG4bwdtyTH"),
            Err(INVALID_ARGUMENT)
        );
    }

    #[test]
    fn util_accid_wrong_length() {
        assert_eq!(
            util_accid(b"rHb9CJAWyB4rj91VRWn96DkukG4bwdty"),
            Err(INVALID_ARGUMENT)
        );
    }

    #[test]
    fn util_accid_invalid_char() {
        assert_eq!(
            util_accid(b"rHb9CJAWyB4rj91VRWn96DkukG4bwdtyT0"),
            Err(INVALID_ARGUMENT)
        );
    }

    #[test]
    fn util_accid_too_big() {
        let long = std::vec![b'r'; 50];
        assert_eq!(util_accid(&long), Err(TOO_BIG));
    }

    #[test]
    fn util_raddr_wrong_length() {
        assert_eq!(util_raddr(&[0u8; 19]), Err(INVALID_ARGUMENT));
        assert_eq!(util_raddr(&[0u8; 21]), Err(INVALID_ARGUMENT));
    }

    // -- util_verify --

    #[test]
    fn util_verify_ed25519_positive_and_negative() {
        use ed25519_dalek::{Signer, SigningKey};

        let signing_key = SigningKey::from_bytes(&[7u8; 32]);
        let verifying_key = signing_key.verifying_key();
        let mut public_key = std::vec![0xEDu8];
        public_key.extend_from_slice(verifying_key.as_bytes());

        let data = b"rshooks testenv ed25519 vector";
        let sig = signing_key.sign(data);

        assert_eq!(util_verify(data, &sig.to_bytes(), &public_key), 1);
        // Tampered payload -> verification fails, not an error.
        assert_eq!(util_verify(b"tampered", &sig.to_bytes(), &public_key), 0);
        // Tampered signature -> verification fails.
        let mut bad_sig = sig.to_bytes();
        bad_sig[0] ^= 0xFF;
        assert_eq!(util_verify(data, &bad_sig, &public_key), 0);
    }

    #[test]
    fn util_verify_secp256k1_positive_and_negative() {
        use k256::ecdsa::signature::hazmat::PrehashSigner;
        use k256::ecdsa::{Signature, SigningKey};

        let signing_key = SigningKey::from_bytes(&[9u8; 32].into()).unwrap();
        let verifying_key = signing_key.verifying_key();
        let public_key = verifying_key.to_encoded_point(true); // compressed, 33 bytes
        assert_eq!(public_key.as_bytes().len(), 33);

        let data = b"rshooks testenv secp256k1 vector";
        let digest = sha512_half(data);
        let sig: Signature = signing_key.sign_prehash(&digest).unwrap();
        let der = sig.to_der();

        assert_eq!(util_verify(data, der.as_bytes(), public_key.as_bytes()), 1);
        assert_eq!(
            util_verify(b"tampered", der.as_bytes(), public_key.as_bytes()),
            0
        );
    }

    #[test]
    fn util_verify_bad_key_type_and_length() {
        assert_eq!(util_verify(b"data", &[0u8; 64], &[0x04u8; 33]), INVALID_KEY);
        assert_eq!(util_verify(b"data", &[0u8; 64], &[0xEDu8; 32]), INVALID_KEY);
    }

    #[test]
    fn util_verify_too_small() {
        assert_eq!(util_verify(b"", &[0u8; 64], &[0xEDu8; 33]), TOO_SMALL);
        assert_eq!(util_verify(b"data", &[0u8; 10], &[0xEDu8; 33]), TOO_SMALL);
    }
}
