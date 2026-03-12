//! Encrypted DM sessions: X25519 key exchange + ChaCha20-Poly1305 AEAD.

use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use chacha20poly1305::aead::{Aead, KeyInit};
use chacha20poly1305::{ChaCha20Poly1305, Nonce};
use hkdf::Hkdf;
use sha2::Sha256;
use x25519_dalek::{PublicKey, StaticSecret};

pub struct Keypair {
    pub secret: StaticSecret,
    pub public: PublicKey,
}

impl Keypair {
    pub fn generate() -> Self {
        let secret = StaticSecret::random_from_rng(rand::thread_rng());
        let public = PublicKey::from(&secret);
        Self { secret, public }
    }

    pub fn public_key_b64(&self) -> String {
        B64.encode(self.public.as_bytes())
    }
}

pub struct SecureSession {
    cipher: ChaCha20Poly1305,
    nonce_counter: u64,
}

impl SecureSession {
    /// Derive a session from our secret and their public key via X25519 + HKDF-SHA256.
    pub fn from_exchange(our_secret: &StaticSecret, their_public_b64: &str) -> Result<Self, String> {
        let their_bytes: [u8; 32] = B64
            .decode(their_public_b64)
            .map_err(|e| format!("bad base64: {}", e))?
            .try_into()
            .map_err(|_| "public key must be 32 bytes".to_string())?;
        let their_public = PublicKey::from(their_bytes);
        let shared = our_secret.diffie_hellman(&their_public);

        let hk = Hkdf::<Sha256>::new(None, shared.as_bytes());
        let mut key_bytes = [0u8; 32];
        hk.expand(b"rvIRC-secure-dm", &mut key_bytes)
            .map_err(|_| "HKDF expand failed".to_string())?;

        let cipher = ChaCha20Poly1305::new_from_slice(&key_bytes)
            .map_err(|e| format!("cipher init: {}", e))?;

        Ok(Self {
            cipher,
            nonce_counter: 0,
        })
    }

    pub fn encrypt(&mut self, plaintext: &str) -> Result<(String, String), String> {
        self.nonce_counter += 1;
        let mut nonce_bytes = [0u8; 12];
        nonce_bytes[4..].copy_from_slice(&self.nonce_counter.to_be_bytes());
        let nonce = Nonce::from(nonce_bytes);

        let ciphertext = self
            .cipher
            .encrypt(&nonce, plaintext.as_bytes())
            .map_err(|e| format!("encrypt: {}", e))?;

        Ok((B64.encode(nonce_bytes), B64.encode(ciphertext)))
    }

    pub fn decrypt(&self, nonce_b64: &str, ciphertext_b64: &str) -> Result<String, String> {
        let nonce_bytes: [u8; 12] = B64
            .decode(nonce_b64)
            .map_err(|e| format!("bad nonce b64: {}", e))?
            .try_into()
            .map_err(|_| "nonce must be 12 bytes".to_string())?;
        let nonce = Nonce::from(nonce_bytes);

        let ciphertext = B64
            .decode(ciphertext_b64)
            .map_err(|e| format!("bad ciphertext b64: {}", e))?;

        let plaintext = self
            .cipher
            .decrypt(&nonce, ciphertext.as_ref())
            .map_err(|_| "decryption failed (wrong key or tampered)".to_string())?;

        String::from_utf8(plaintext).map_err(|e| format!("invalid UTF-8: {}", e))
    }
}
