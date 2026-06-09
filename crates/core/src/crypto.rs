//! AES-256-GCM token sealing.
//!
//! Direct port of `core/crypto.ts`. **Format must stay byte-compatible** with
//! `node:crypto`'s `aes-256-gcm` so tokens sealed by the TypeScript server
//! decrypt unchanged:
//!   - 12-byte random IV (nonce),
//!   - 16-byte auth tag stored in its own field (the `ciphertext` field holds
//!     only the ciphertext — hence the *detached* AEAD API),
//!   - empty AAD,
//!   - every field base64 (standard alphabet, padded).

use aes_gcm::aead::{AeadInPlace, KeyInit};
use aes_gcm::{Aes256Gcm, Nonce};
use base64::Engine;
use thiserror::Error;

const IV_BYTES: usize = 12;

#[derive(Debug, Error)]
pub enum CryptoError {
    #[error("failed to seal token: {0}")]
    Seal(String),
    #[error("failed to open token: {0}")]
    Open(String),
}

/// A sealed token: ciphertext, IV, and auth tag, each base64-encoded. Mirrors
/// the `SealedT` shape and the three DB columns (`*_ciphertext`, `*_iv`,
/// `*_auth_tag`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sealed {
    pub ciphertext: String,
    pub iv: String,
    pub auth_tag: String,
}

/// Seals and opens tokens with a fixed 32-byte key. The same cipher is used for
/// both GitHub PATs and Jira API tokens (Jira reuses the GitHub key).
#[derive(Clone)]
pub struct TokenCipher {
    cipher: Aes256Gcm,
}

impl TokenCipher {
    pub fn new(key: &[u8; 32]) -> Self {
        Self {
            cipher: Aes256Gcm::new(key.into()),
        }
    }

    /// Encrypts `plaintext`, returning a fresh random IV and the detached tag.
    pub fn seal(&self, plaintext: &str) -> Result<Sealed, CryptoError> {
        let mut iv = [0u8; IV_BYTES];
        getrandom::fill(&mut iv).map_err(|e| CryptoError::Seal(e.to_string()))?;
        let nonce = Nonce::from_slice(&iv);

        let mut buf = plaintext.as_bytes().to_vec();
        let tag = self
            .cipher
            .encrypt_in_place_detached(nonce, b"", &mut buf)
            .map_err(|e| CryptoError::Seal(e.to_string()))?;

        let b64 = base64::engine::general_purpose::STANDARD;
        Ok(Sealed {
            ciphertext: b64.encode(&buf),
            iv: b64.encode(iv),
            auth_tag: b64.encode(tag),
        })
    }

    /// Decrypts a sealed token, verifying the auth tag.
    pub fn open(&self, sealed: &Sealed) -> Result<String, CryptoError> {
        let b64 = base64::engine::general_purpose::STANDARD;
        let decode = |field: &str, val: &str| {
            b64.decode(val.as_bytes())
                .map_err(|e| CryptoError::Open(format!("{field} base64: {e}")))
        };
        let iv = decode("iv", &sealed.iv)?;
        let tag = decode("authTag", &sealed.auth_tag)?;
        let mut buf = decode("ciphertext", &sealed.ciphertext)?;

        if iv.len() != IV_BYTES {
            return Err(CryptoError::Open(format!("iv must be {IV_BYTES} bytes")));
        }
        let nonce = Nonce::from_slice(&iv);
        let tag = aes_gcm::Tag::from_slice(&tag);

        self.cipher
            .decrypt_in_place_detached(nonce, b"", &mut buf, tag)
            .map_err(|e| CryptoError::Open(e.to_string()))?;

        String::from_utf8(buf).map_err(|e| CryptoError::Open(e.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cipher() -> TokenCipher {
        TokenCipher::new(&[7u8; 32])
    }

    #[test]
    fn round_trips() {
        let c = cipher();
        let sealed = c.seal("ghp_secret_token").unwrap();
        assert_eq!(c.open(&sealed).unwrap(), "ghp_secret_token");
    }

    #[test]
    fn fresh_iv_per_seal() {
        let c = cipher();
        let a = c.seal("x").unwrap();
        let b = c.seal("x").unwrap();
        assert_ne!(a.iv, b.iv);
        assert_ne!(a.ciphertext, b.ciphertext);
    }

    #[test]
    fn tampered_tag_fails() {
        let c = cipher();
        let mut sealed = c.seal("x").unwrap();
        sealed.auth_tag = base64::engine::general_purpose::STANDARD.encode([0u8; 16]);
        assert!(c.open(&sealed).is_err());
    }

    /// Known-answer vector produced by `node:crypto` to lock byte-compatibility:
    ///   key  = 32 bytes of 0x07
    ///   iv   = 12 bytes of 0x00
    ///   text = "hello-rust"
    /// Generated with:
    ///   const cl = crypto.createCipheriv("aes-256-gcm", Buffer.alloc(32, 7), Buffer.alloc(12, 0));
    ///   const ct = Buffer.concat([cl.update("hello-rust", "utf8"), cl.final()]);
    ///   ct.toString("base64") / cl.getAuthTag().toString("base64")
    /// This guards against any divergence from the TS sealing format.
    #[test]
    fn decrypts_node_sealed_vector() {
        let c = cipher();
        let b64 = base64::engine::general_purpose::STANDARD;
        let sealed = Sealed {
            ciphertext: NODE_VECTOR_CIPHERTEXT.to_string(),
            iv: b64.encode([0u8; 12]),
            auth_tag: NODE_VECTOR_AUTH_TAG.to_string(),
        };
        assert_eq!(c.open(&sealed).unwrap(), "hello-rust");
    }

    const NODE_VECTOR_CIPHERTEXT: &str = "Cb3I3pS/eZ2Scw==";
    const NODE_VECTOR_AUTH_TAG: &str = "OE4MNGivYSVM8gE542/IlQ==";
}
