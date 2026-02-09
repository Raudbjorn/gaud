use argon2::password_hash::SaltString;
use argon2::{Argon2, PasswordHash, PasswordHasher, PasswordVerifier};
use rand::Rng;
use sha2::{Digest, Sha256};

const KEY_PREFIX: &str = "sk-prx-";
const KEY_RANDOM_LEN: usize = 32;
/// Salt length in bytes (16 bytes = 22 base64 chars, well within argon2 limits).
const SALT_LEN: usize = 16;

/// A newly generated API key containing the raw plaintext and its argon2 hash.
#[derive(Debug)]
pub struct GeneratedKey {
    /// The full plaintext key to display to the user exactly once.
    pub plaintext: String,
    /// The argon2 hash to store in the database.
    pub hash: String,
    /// The short prefix (e.g. "sk-prx-a1b2c3d4") for display in listings.
    pub prefix: String,
}

/// Generate a new API key with the format `sk-prx-{32 alphanumeric}`.
///
/// Returns the plaintext key, its argon2 hash, and a short prefix for display.
pub fn generate_api_key() -> Result<GeneratedKey, argon2::password_hash::Error> {
    let random_part = generate_random_alphanumeric(KEY_RANDOM_LEN);
    let plaintext = format!("{KEY_PREFIX}{random_part}");
    let prefix = format!("{KEY_PREFIX}{}...", &random_part[..8]);
    let hash = hash_key(&plaintext)?;

    Ok(GeneratedKey {
        plaintext,
        hash,
        prefix,
    })
}

/// Hash a plaintext API key using argon2id.
///
/// We first SHA-256 the key to produce a fixed-length input for argon2,
/// which avoids issues with variable-length passwords and keeps the argon2
/// input consistent.
pub fn hash_key(plaintext: &str) -> Result<String, argon2::password_hash::Error> {
    let sha_digest = sha256_key(plaintext);
    let salt = generate_salt()?;
    let argon2 = Argon2::default();
    let hash = argon2.hash_password(sha_digest.as_bytes(), &salt)?;
    Ok(hash.to_string())
}

/// Generate a random salt string for argon2 using `rand`.
fn generate_salt() -> Result<SaltString, argon2::password_hash::Error> {
    let mut rng = rand::rng();
    let mut salt_bytes = [0u8; SALT_LEN];
    rng.fill(&mut salt_bytes);
    // SaltString::encode_b64 produces a base64 (no padding) string from raw bytes.
    SaltString::encode_b64(&salt_bytes)
}

/// Verify a plaintext API key against an argon2 hash.
pub fn verify_key(plaintext: &str, hash: &str) -> Result<bool, argon2::password_hash::Error> {
    let sha_digest = sha256_key(plaintext);
    let parsed_hash = PasswordHash::new(hash)?;
    Ok(Argon2::default()
        .verify_password(sha_digest.as_bytes(), &parsed_hash)
        .is_ok())
}

/// SHA-256 digest of a key, returned as a hex string.
fn sha256_key(key: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(key.as_bytes());
    format!("{:x}", hasher.finalize())
}

/// Generate a random alphanumeric string of the given length.
fn generate_random_alphanumeric(len: usize) -> String {
    const CHARSET: &[u8] = b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";
    let mut rng = rand::rng();
    (0..len)
        .map(|_| {
            let idx = rng.random_range(0..CHARSET.len());
            CHARSET[idx] as char
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_api_key_format() {
        let key = generate_api_key().unwrap();
        assert!(key.plaintext.starts_with("sk-prx-"));
        assert_eq!(key.plaintext.len(), KEY_PREFIX.len() + KEY_RANDOM_LEN);
        assert!(key.prefix.ends_with("..."));
        assert!(!key.hash.is_empty());
    }

    #[test]
    fn test_generate_api_key_unique() {
        let key1 = generate_api_key().unwrap();
        let key2 = generate_api_key().unwrap();
        assert_ne!(key1.plaintext, key2.plaintext);
        assert_ne!(key1.hash, key2.hash);
    }

    #[test]
    fn test_hash_and_verify() {
        let plaintext = "sk-prx-testkey12345678901234567890ab";
        let hash = hash_key(plaintext).unwrap();
        assert!(verify_key(plaintext, &hash).unwrap());
    }

    #[test]
    fn test_verify_wrong_key() {
        let plaintext = "sk-prx-testkey12345678901234567890ab";
        let hash = hash_key(plaintext).unwrap();
        assert!(!verify_key("sk-prx-wrongkey1234567890123456789", &hash).unwrap());
    }

    #[test]
    fn test_alphanumeric_only() {
        let key = generate_api_key().unwrap();
        let random_part = &key.plaintext[KEY_PREFIX.len()..];
        assert!(random_part.chars().all(|c| c.is_ascii_alphanumeric()));
    }
}
