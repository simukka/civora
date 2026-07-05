//! Passphrase-encrypted storage for the player's secret key.
//!
//! File format (versioned by the magic):
//! `CIVKEY1\n` || Argon2id salt (16) || XChaCha20 nonce (24) || AEAD
//! ciphertext of the 32-byte seed (48, including the Poly1305 tag).

use std::fmt;
use std::io;
use std::path::Path;

use argon2::Argon2;
use chacha20poly1305::aead::{Aead, KeyInit};
use chacha20poly1305::{Key, XChaCha20Poly1305, XNonce};
use rand_core::{OsRng, RngCore};

use crate::identity::Identity;

const MAGIC: &[u8; 8] = b"CIVKEY1\n";
const SALT_LEN: usize = 16;
const NONCE_LEN: usize = 24;
const CIPHERTEXT_LEN: usize = 32 + 16; // seed + Poly1305 tag
const FILE_LEN: usize = MAGIC.len() + SALT_LEN + NONCE_LEN + CIPHERTEXT_LEN;

#[derive(Debug)]
pub enum KeyfileError {
    Io(io::Error),
    /// Wrong magic or length: not a Civora key file.
    Malformed,
    /// AEAD authentication failed: wrong passphrase or tampered file.
    WrongPassphrase,
    /// Passphrase key derivation failed.
    Kdf,
}

impl fmt::Display for KeyfileError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            KeyfileError::Io(err) => write!(f, "{err}"),
            KeyfileError::Malformed => write!(f, "not a Civora key file"),
            KeyfileError::WrongPassphrase => write!(f, "wrong passphrase (or corrupted key file)"),
            KeyfileError::Kdf => write!(f, "passphrase key derivation failed"),
        }
    }
}

impl std::error::Error for KeyfileError {}

impl From<io::Error> for KeyfileError {
    fn from(err: io::Error) -> Self {
        KeyfileError::Io(err)
    }
}

fn derive_key(passphrase: &str, salt: &[u8]) -> Result<[u8; 32], KeyfileError> {
    let mut key = [0u8; 32];
    Argon2::default()
        .hash_password_into(passphrase.as_bytes(), salt, &mut key)
        .map_err(|_| KeyfileError::Kdf)?;
    Ok(key)
}

/// Encrypt `identity`'s seed with `passphrase` and write it to `path`,
/// creating parent directories and using owner-only permissions on Unix.
pub fn save_encrypted(
    path: &Path,
    identity: &Identity,
    passphrase: &str,
) -> Result<(), KeyfileError> {
    let mut salt = [0u8; SALT_LEN];
    OsRng.fill_bytes(&mut salt);
    let mut nonce = [0u8; NONCE_LEN];
    OsRng.fill_bytes(&mut nonce);

    let key = derive_key(passphrase, &salt)?;
    let ciphertext = XChaCha20Poly1305::new(Key::from_slice(&key))
        .encrypt(XNonce::from_slice(&nonce), identity.seed().as_slice())
        .map_err(|_| KeyfileError::Kdf)?;

    let mut bytes = Vec::with_capacity(FILE_LEN);
    bytes.extend_from_slice(MAGIC);
    bytes.extend_from_slice(&salt);
    bytes.extend_from_slice(&nonce);
    bytes.extend_from_slice(&ciphertext);

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    write_private(path, &bytes)?;
    Ok(())
}

/// Read the key file at `path` and decrypt it with `passphrase`.
pub fn load_encrypted(path: &Path, passphrase: &str) -> Result<Identity, KeyfileError> {
    let bytes = std::fs::read(path)?;
    if bytes.len() != FILE_LEN || &bytes[..MAGIC.len()] != MAGIC {
        return Err(KeyfileError::Malformed);
    }
    let salt = &bytes[MAGIC.len()..MAGIC.len() + SALT_LEN];
    let nonce = &bytes[MAGIC.len() + SALT_LEN..MAGIC.len() + SALT_LEN + NONCE_LEN];
    let ciphertext = &bytes[MAGIC.len() + SALT_LEN + NONCE_LEN..];

    let key = derive_key(passphrase, salt)?;
    let seed = XChaCha20Poly1305::new(Key::from_slice(&key))
        .decrypt(XNonce::from_slice(nonce), ciphertext)
        .map_err(|_| KeyfileError::WrongPassphrase)?;
    let seed: [u8; 32] = seed.try_into().map_err(|_| KeyfileError::Malformed)?;
    Ok(Identity::from_seed(seed))
}

/// Write with owner-only permissions where the platform supports it.
fn write_private(path: &Path, bytes: &[u8]) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(path)?;
        file.write_all(bytes)
    }
    #[cfg(not(unix))]
    {
        std::fs::write(path, bytes)
    }
}
