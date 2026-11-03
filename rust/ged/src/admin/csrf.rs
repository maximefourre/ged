//! Request forgery protection, shaped like Rails' `protect_from_forgery`
//! (`ActionController::RequestForgeryProtection`): the session holds a 32-byte
//! secret; a form carries it one-time-padded (`pad ++ pad xor secret`,
//! url-safe base64), so the token in a page changes on every render (BREACH)
//! while checking it needs only the secret.

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use subtle_eq::constant_time_eq;

pub const SECRET_LEN: usize = 32;

pub type Secret = [u8; SECRET_LEN];

/// 32 random bytes (two v4 UUIDs: 244 bits from the OS generator).
pub fn new_secret() -> Secret {
    let mut out = [0u8; SECRET_LEN];
    out[..16].copy_from_slice(uuid::Uuid::new_v4().as_bytes());
    out[16..].copy_from_slice(uuid::Uuid::new_v4().as_bytes());
    out
}

pub fn encode_secret(secret: &Secret) -> String {
    URL_SAFE_NO_PAD.encode(secret)
}

pub fn decode_secret(text: &str) -> Option<Secret> {
    URL_SAFE_NO_PAD.decode(text).ok()?.try_into().ok()
}

/// A fresh masked token for `secret`.
pub fn mask(secret: &Secret) -> String {
    let pad = new_secret();
    let mut raw = Vec::with_capacity(SECRET_LEN * 2);
    raw.extend_from_slice(&pad);
    raw.extend(pad.iter().zip(secret).map(|(p, s)| p ^ s));
    URL_SAFE_NO_PAD.encode(raw)
}

/// Whether `submitted` is a masked token of `secret`. Anything malformed is
/// simply false.
pub fn verify(secret: &Secret, submitted: &str) -> bool {
    let Ok(raw) = URL_SAFE_NO_PAD.decode(submitted.trim_end_matches('=')) else {
        return false;
    };
    if raw.len() != SECRET_LEN * 2 {
        return false;
    }
    let (pad, masked) = raw.split_at(SECRET_LEN);
    let unmasked: Vec<u8> = pad.iter().zip(masked).map(|(p, m)| p ^ m).collect();
    constant_time_eq(&unmasked, secret)
}

mod subtle_eq {
    /// Equal-length byte comparison that does not stop at the first difference.
    pub fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
        if a.len() != b.len() {
            return false;
        }
        a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_masked_token_verifies_against_its_secret() {
        let secret = new_secret();
        assert!(verify(&secret, &mask(&secret)));
    }

    #[test]
    fn every_mask_differs_but_all_verify() {
        let secret = new_secret();
        let (a, b) = (mask(&secret), mask(&secret));
        assert_ne!(a, b);
        assert!(verify(&secret, &a) && verify(&secret, &b));
    }

    #[test]
    fn a_token_of_another_secret_is_refused() {
        let (mine, theirs) = (new_secret(), new_secret());
        assert!(!verify(&mine, &mask(&theirs)));
    }

    #[test]
    fn malformed_tokens_are_refused() {
        let secret = new_secret();
        for bad in ["", "not-a-token", "AAAA", &"A".repeat(86), &"A".repeat(200)] {
            assert!(!verify(&secret, bad), "{bad:?}");
        }
        // The right length with the right padding bytes, but a flipped bit.
        let mut raw = URL_SAFE_NO_PAD.decode(mask(&secret)).unwrap();
        raw[SECRET_LEN] ^= 1;
        assert!(!verify(&secret, &URL_SAFE_NO_PAD.encode(raw)));
    }

    #[test]
    fn the_secret_round_trips_through_its_text_form() {
        let secret = new_secret();
        assert_eq!(decode_secret(&encode_secret(&secret)), Some(secret));
        assert_eq!(decode_secret("short"), None);
    }
}
