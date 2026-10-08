//! Confirm-on-the-Mac pairing (docs/remote-protocol.md, "Confirmed pairing"):
//! the six digits both screens show, and the commitment that keeps them honest.
//!
//! A code derived from the Noise transcript alone could be ground: a party in
//! the middle runs two handshakes and, with its own ephemeral keys free to
//! choose, tries a million of them offline until the two transcripts give the
//! same six digits. So each side adds a 32-byte nonce, in an order that leaves
//! nobody a choice after seeing the other's — the device commits to its nonce
//! first, the host then sends its own in the clear, and only then does the
//! device reveal. An attacker gets one guess in a million, not a million
//! guesses.

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use sha2::{Digest, Sha256};

use crate::Result;

pub const NONCE_LEN: usize = 32;

const COMMIT_CONTEXT: &[u8] = b"fletch-pair-commit-v1";
const CODE_CONTEXT: &[u8] = b"fletch-pair-code-v1";

/// A nonce or commitment off the wire: base64url, no padding, exactly 32 bytes.
/// `what` names it in the error.
pub fn decode(text: &str, what: &str) -> Result<[u8; NONCE_LEN]> {
    URL_SAFE_NO_PAD
        .decode(text)
        .ok()
        .and_then(|bytes| bytes.try_into().ok())
        .ok_or_else(|| format!("{what} is not 32 bytes of base64url"))
}

/// A fresh random nonce.
pub fn nonce() -> Result<[u8; NONCE_LEN]> {
    let mut nonce = [0u8; NONCE_LEN];
    getrandom::fill(&mut nonce).map_err(|e| format!("no randomness: {e}"))?;
    Ok(nonce)
}

/// What the device sends before it has seen the host's nonce.
pub fn commitment(device_nonce: &[u8; NONCE_LEN]) -> [u8; 32] {
    Sha256::new()
        .chain_update(COMMIT_CONTEXT)
        .chain_update(device_nonce)
        .finalize()
        .into()
}

/// The six digits both screens show, zero-padded: the transcript hash and both
/// nonces, each end computing it from what it saw.
pub fn code(
    handshake_hash: &[u8; 32],
    device_nonce: &[u8; NONCE_LEN],
    host_nonce: &[u8; NONCE_LEN],
) -> String {
    let digest = Sha256::new()
        .chain_update(CODE_CONTEXT)
        .chain_update(handshake_hash)
        .chain_update(device_nonce)
        .chain_update(host_nonce)
        .finalize();
    let n = u32::from_be_bytes([digest[0], digest[1], digest[2], digest[3]]);
    format!("{:06}", n % 1_000_000)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Known answers, computed independently (Python's hashlib), so the host
    /// and anything that ever computes these elsewhere cannot drift apart.
    #[test]
    fn known_answers() {
        let hash = [1u8; 32];
        let device = [2u8; 32];
        let host = [3u8; 32];
        let hex: String = commitment(&device)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        assert_eq!(
            hex,
            "f849c13ba98807191dea89d4db823c7cbf09ac959561500544def82ad60b885b"
        );
        assert_eq!(code(&hash, &device, &host), "277588");
        // Order matters: swapping the nonces is a different code.
        assert_ne!(code(&hash, &host, &device), "277588");
    }

    #[test]
    fn decodes_only_32_bytes() {
        let encoded = crate::encode_key(&[7u8; 32]);
        assert_eq!(decode(&encoded, "nonce").unwrap(), [7u8; 32]);
        assert!(decode(&crate::encode_key(&[7u8; 31]), "nonce").is_err());
        assert!(decode("not base64!", "nonce").is_err());
    }

    #[test]
    fn nonces_differ() {
        assert_ne!(nonce().unwrap(), nonce().unwrap());
    }
}
