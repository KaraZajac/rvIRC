//! Encrypted DM sessions: X25519 key exchange + ChaCha20-Poly1305 AEAD.
//! Persistent identity keys, TOFU key tracking, and SAS verification.

use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use chacha20poly1305::aead::{Aead, KeyInit};
use chacha20poly1305::{ChaCha20Poly1305, Nonce};
use hkdf::Hkdf;
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use std::path::Path;
use x25519_dalek::{PublicKey, StaticSecret};

// ---------------------------------------------------------------------------
// Keypair (identity + ephemeral)
// ---------------------------------------------------------------------------

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

    /// Load identity keypair from a TOML file, or generate and save a new one.
    pub fn load_or_generate(path: &Path) -> Result<Self, String> {
        if path.exists() {
            let s = std::fs::read_to_string(path).map_err(|e| format!("read identity: {}", e))?;
            let stored: StoredKeypair =
                toml::from_str(&s).map_err(|e| format!("parse identity: {}", e))?;
            let secret_bytes: [u8; 32] = B64
                .decode(&stored.secret_key)
                .map_err(|e| format!("bad secret b64: {}", e))?
                .try_into()
                .map_err(|_| "secret key must be 32 bytes".to_string())?;
            let secret = StaticSecret::from(secret_bytes);
            let public = PublicKey::from(&secret);
            Ok(Self { secret, public })
        } else {
            let kp = Self::generate();
            kp.save(path)?;
            Ok(kp)
        }
    }

    /// Persist keypair to a TOML file with restrictive permissions.
    pub fn save(&self, path: &Path) -> Result<(), String> {
        let stored = StoredKeypair {
            secret_key: B64.encode(self.secret.as_bytes()),
            public_key: self.public_key_b64(),
        };
        let toml_str =
            toml::to_string_pretty(&stored).map_err(|e| format!("serialize identity: {}", e))?;
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        std::fs::write(path, &toml_str).map_err(|e| format!("write identity: {}", e))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let perms = std::fs::Permissions::from_mode(0o600);
            let _ = std::fs::set_permissions(path, perms);
        }
        Ok(())
    }
}

#[derive(Serialize, Deserialize)]
struct StoredKeypair {
    secret_key: String,
    public_key: String,
}

// ---------------------------------------------------------------------------
// SecureSession (directional keys)
// ---------------------------------------------------------------------------

pub struct SecureSession {
    send_cipher: ChaCha20Poly1305,
    recv_cipher: ChaCha20Poly1305,
    send_nonce_counter: u64,
    /// Raw DH shared secret, kept for SAS derivation.
    shared_secret: [u8; 32],
    /// Our identity public key (for SAS binding).
    pub our_identity_pub: [u8; 32],
    /// Their identity public key (for SAS binding).
    pub their_identity_pub: [u8; 32],
}

impl SecureSession {
    /// Derive a session from ephemeral DH. Uses lexicographic ordering of the
    /// ephemeral public keys to assign directional roles, preventing nonce reuse.
    pub fn from_exchange(
        our_ephemeral_secret: &StaticSecret,
        our_ephemeral_public: &PublicKey,
        their_ephemeral_pub_b64: &str,
        our_identity_pub: &PublicKey,
        their_identity_pub_bytes: [u8; 32],
    ) -> Result<Self, String> {
        let their_bytes: [u8; 32] = B64
            .decode(their_ephemeral_pub_b64)
            .map_err(|e| format!("bad base64: {}", e))?
            .try_into()
            .map_err(|_| "public key must be 32 bytes".to_string())?;
        let their_public = PublicKey::from(their_bytes);
        let shared = our_ephemeral_secret.diffie_hellman(&their_public);
        let shared_bytes: [u8; 32] = *shared.as_bytes();

        let hk = Hkdf::<Sha256>::new(None, &shared_bytes);

        let our_bytes = *our_ephemeral_public.as_bytes();
        let we_are_lower = our_bytes < their_bytes;

        let (send_info, recv_info) = if we_are_lower {
            (&b"rvIRC-dm-init"[..], &b"rvIRC-dm-resp"[..])
        } else {
            (&b"rvIRC-dm-resp"[..], &b"rvIRC-dm-init"[..])
        };

        let mut send_key = [0u8; 32];
        hk.expand(send_info, &mut send_key)
            .map_err(|_| "HKDF expand failed".to_string())?;

        let mut recv_key = [0u8; 32];
        hk.expand(recv_info, &mut recv_key)
            .map_err(|_| "HKDF expand failed".to_string())?;

        let send_cipher = ChaCha20Poly1305::new_from_slice(&send_key)
            .map_err(|e| format!("cipher init: {}", e))?;
        let recv_cipher = ChaCha20Poly1305::new_from_slice(&recv_key)
            .map_err(|e| format!("cipher init: {}", e))?;

        Ok(Self {
            send_cipher,
            recv_cipher,
            send_nonce_counter: 0,
            shared_secret: shared_bytes,
            our_identity_pub: *our_identity_pub.as_bytes(),
            their_identity_pub: their_identity_pub_bytes,
        })
    }

    pub fn encrypt(&mut self, plaintext: &str) -> Result<(String, String), String> {
        self.send_nonce_counter += 1;
        let mut nonce_bytes = [0u8; 12];
        nonce_bytes[4..].copy_from_slice(&self.send_nonce_counter.to_be_bytes());
        let nonce = Nonce::from(nonce_bytes);

        let ciphertext = self
            .send_cipher
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
            .recv_cipher
            .decrypt(&nonce, ciphertext.as_ref())
            .map_err(|_| "decryption failed (wrong key or tampered)".to_string())?;

        String::from_utf8(plaintext).map_err(|e| format!("invalid UTF-8: {}", e))
    }

    /// Derive a 6-word SAS code bound to the shared secret and both identity keys.
    pub fn sas_words(&self) -> Vec<&'static str> {
        let mut material = Vec::with_capacity(96);
        material.extend_from_slice(&self.shared_secret);
        material.extend_from_slice(&self.our_identity_pub);
        material.extend_from_slice(&self.their_identity_pub);

        let hk = Hkdf::<Sha256>::new(None, &material);
        let mut sas_bytes = [0u8; 6];
        hk.expand(b"rvIRC-sas-verify", &mut sas_bytes)
            .expect("HKDF expand for SAS");

        sas_bytes.iter().map(|b| SAS_WORDLIST[*b as usize]).collect()
    }
}

// ---------------------------------------------------------------------------
// TOFU Known Keys
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PeerKey {
    pub nick: String,
    pub server: String,
    pub identity_key: String,
    pub verified: bool,
    pub first_seen: String,
    pub last_seen: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct KnownKeys {
    #[serde(default)]
    pub peers: Vec<PeerKey>,
}

/// Result of checking a peer's key against the known keys store.
pub enum TofuResult {
    /// First time seeing this peer.
    FirstContact,
    /// Key matches what we have on record.
    KeyMatch { verified: bool },
    /// Key has changed since last time.
    KeyChanged,
}

impl KnownKeys {
    pub fn load(path: &Path) -> Self {
        if !path.exists() {
            return Self::default();
        }
        std::fs::read_to_string(path)
            .ok()
            .and_then(|s| toml::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, path: &Path) -> Result<(), String> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        let s = toml::to_string_pretty(self).map_err(|e| e.to_string())?;
        std::fs::write(path, s).map_err(|e| e.to_string())?;
        Ok(())
    }

    pub fn lookup(&self, nick: &str, server: &str) -> Option<&PeerKey> {
        self.peers
            .iter()
            .find(|p| p.nick.eq_ignore_ascii_case(nick) && p.server.eq_ignore_ascii_case(server))
    }

    /// Check a peer's identity key against the store.
    pub fn check(&self, nick: &str, server: &str, identity_key_b64: &str) -> TofuResult {
        match self.lookup(nick, server) {
            None => TofuResult::FirstContact,
            Some(p) if p.identity_key == identity_key_b64 => TofuResult::KeyMatch {
                verified: p.verified,
            },
            Some(_) => TofuResult::KeyChanged,
        }
    }

    /// Insert or update a peer's identity key. Resets verified on key change.
    pub fn upsert(&mut self, nick: &str, server: &str, identity_key_b64: &str) {
        let now = chrono_now();
        if let Some(p) = self.peers.iter_mut().find(|p| {
            p.nick.eq_ignore_ascii_case(nick) && p.server.eq_ignore_ascii_case(server)
        }) {
            if p.identity_key != identity_key_b64 {
                p.identity_key = identity_key_b64.to_string();
                p.verified = false;
            }
            p.last_seen = now;
        } else {
            self.peers.push(PeerKey {
                nick: nick.to_string(),
                server: server.to_string(),
                identity_key: identity_key_b64.to_string(),
                verified: false,
                first_seen: now.clone(),
                last_seen: now,
            });
        }
    }

    /// Mark a peer as verified.
    pub fn set_verified(&mut self, nick: &str, server: &str) -> bool {
        if let Some(p) = self.peers.iter_mut().find(|p| {
            p.nick.eq_ignore_ascii_case(nick) && p.server.eq_ignore_ascii_case(server)
        }) {
            p.verified = true;
            true
        } else {
            false
        }
    }

    pub fn is_verified(&self, nick: &str, server: &str) -> bool {
        self.lookup(nick, server).map_or(false, |p| p.verified)
    }
}

fn chrono_now() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    format!("{}", secs)
}

/// Compute a short hex fingerprint of a public key (first 8 bytes of SHA-256).
pub fn key_fingerprint(pub_key_b64: &str) -> String {
    use sha2::Digest;
    if let Ok(bytes) = B64.decode(pub_key_b64) {
        let hash = Sha256::digest(&bytes);
        hash.iter()
            .take(8)
            .map(|b| format!("{:02X}", b))
            .collect::<Vec<_>>()
            .join(":")
    } else {
        "??".to_string()
    }
}

// ---------------------------------------------------------------------------
// SAS wordlist (256 words, index = byte value)
// ---------------------------------------------------------------------------

const SAS_WORDLIST: [&str; 256] = [
    "anchor", "apple", "arrow", "atlas", "autumn", "badge", "ballet", "bamboo",
    "banner", "barrel", "beacon", "blade", "blanket", "blaze", "bloom", "bolt",
    "branch", "breeze", "bridge", "bronze", "brush", "cabin", "cactus", "candle",
    "canyon", "carbon", "castle", "cedar", "chain", "chalk", "cherry", "cipher",
    "circuit", "cliff", "cloud", "cobalt", "comet", "copper", "coral", "crane",
    "crater", "creek", "crown", "crystal", "current", "dagger", "dawn", "delta",
    "desert", "diamond", "dolphin", "dome", "dragon", "drift", "eagle", "eclipse",
    "ember", "engine", "falcon", "feather", "fern", "flame", "flint", "flower",
    "forest", "fossil", "frost", "galaxy", "garden", "garnet", "glacier", "globe",
    "granite", "harbor", "harvest", "hawk", "hazel", "helix", "hermit", "hollow",
    "honey", "horizon", "iceberg", "igloo", "impact", "island", "ivory", "jacket",
    "jade", "jaguar", "jasmine", "jewel", "jungle", "karma", "kayak", "kingdom",
    "knight", "lagoon", "lantern", "lark", "latch", "laurel", "lemon", "leopard",
    "light", "linden", "lotus", "lunar", "magnet", "maple", "marble", "meadow",
    "meteor", "mirror", "mocha", "monarch", "mosaic", "mountain", "mural", "nebula",
    "nectar", "nimbus", "noble", "nova", "nutmeg", "oasis", "obsidian", "ocean",
    "olive", "onyx", "orbit", "orchid", "osprey", "oyster", "paddle", "palace",
    "palm", "panther", "paper", "parcel", "pasture", "pebble", "pepper", "phoenix",
    "pillar", "pine", "planet", "plaza", "plume", "polar", "portal", "prism",
    "pulse", "puma", "puzzle", "quartz", "quest", "quill", "rabbit", "radiant",
    "rain", "raven", "reef", "ridge", "river", "robin", "rocket", "ruby",
    "saddle", "sage", "salmon", "sapphire", "saturn", "scarlet", "scroll", "shadow",
    "shell", "shield", "signal", "silver", "sketch", "slate", "snow", "solar",
    "spark", "sphere", "spiral", "spruce", "stamp", "star", "steam", "stone",
    "storm", "stream", "summit", "sunset", "swan", "sword", "tablet", "talon",
    "temple", "terra", "thistle", "thunder", "tiger", "timber", "torch", "tower",
    "trail", "trident", "trophy", "tulip", "tunnel", "turtle", "valley", "vapor",
    "velvet", "venture", "violet", "viper", "vivid", "volcano", "voyage", "walnut",
    "wander", "wave", "willow", "window", "winter", "wolf", "wreath", "xenon",
    "yacht", "yarn", "yellow", "zenith", "zephyr", "zinc", "zodiac", "alpine",
    "amber", "arctic", "azure", "basalt", "birch", "bison", "bliss", "brave",
    "brook", "calm", "cedar", "charm", "cider", "clover", "dawn", "dream",
];
