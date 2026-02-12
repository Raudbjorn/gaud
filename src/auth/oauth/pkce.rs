//! PKCE (Proof Key for Code Exchange) implementation.
//!
//! Provides PKCE functionality for OAuth 2.0 authorization code flows:
//! - Code verifier generation (128-char random string from safe alphabet)
//! - S256 code challenge derivation using SHA-256
//! - Verification that a challenge matches a verifier

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use rand::Rng;
use sha2::{Digest, Sha256};

/// PKCE challenge method constant.
const PKCE_METHOD: &str = "S256";

/// Characters allowed in the PKCE verifier (RFC 7636 unreserved chars).
const VERIFIER_CHARS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-._~";

/// PKCE verifier length in characters (128 chars, the maximum per RFC 7636).
const PKCE_VERIFIER_LENGTH: usize = 128;

/// PKCE (Proof Key for Code Exchange) data.
///
/// Contains a code verifier and its corresponding challenge for use
/// in the OAuth 2.0 authorization code flow with PKCE.
#[derive(Debug, Clone)]
pub struct Pkce {
    /// The code verifier (secret, used during token exchange).
    pub verifier: String,

    /// The code challenge (sent in authorization URL).
    /// SHA-256 hash of the verifier, base64url encoded without padding.
    pub challenge: String,

    /// The challenge method (always "S256").
    pub method: &'static str,
}

impl Pkce {
    /// Generate a new PKCE verifier/challenge pair.
    ///
    /// Uses 128 cryptographically random characters from the unreserved
    /// character set (alphanumeric + -._~). The challenge is the SHA-256
    /// hash of the verifier, base64url encoded without padding.
    #[must_use]
    pub fn generate() -> Self {
        let mut rng = rand::rng();
        let verifier: String = (0..PKCE_VERIFIER_LENGTH)
            .map(|_| {
                let idx = rng.random_range(0..VERIFIER_CHARS.len());
                VERIFIER_CHARS[idx] as char
            })
            .collect();

        let challenge = Self::compute_challenge(&verifier);

        Self {
            verifier,
            challenge,
            method: PKCE_METHOD,
        }
    }

    /// Verify that a challenge matches a verifier.
    #[must_use]
    pub fn verify(verifier: &str, challenge: &str) -> bool {
        let expected = Self::compute_challenge(verifier);
        expected == challenge
    }

    /// Compute the S256 challenge from a verifier.
    fn compute_challenge(verifier: &str) -> String {
        let mut hasher = Sha256::new();
        hasher.update(verifier.as_bytes());
        let hash = hasher.finalize();
        URL_SAFE_NO_PAD.encode(hash)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pkce_generation() {
        let pkce = Pkce::generate();
        assert!(!pkce.verifier.is_empty());
        assert!(!pkce.challenge.is_empty());
        assert_eq!(pkce.method, "S256");
        assert!(Pkce::verify(&pkce.verifier, &pkce.challenge));
    }

    #[test]
    fn test_verifier_length() {
        let pkce = Pkce::generate();
        assert_eq!(pkce.verifier.len(), 128);
    }

    #[test]
    fn test_verifier_uses_safe_chars() {
        let pkce = Pkce::generate();
        assert!(
            pkce.verifier.chars().all(|c| c.is_ascii_alphanumeric()
                || c == '-'
                || c == '.'
                || c == '_'
                || c == '~'),
            "Verifier contains invalid characters: {}",
            pkce.verifier
        );
    }

    #[test]
    fn test_challenge_url_safe() {
        let pkce = Pkce::generate();
        assert!(
            pkce.challenge
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'),
            "Challenge contains non-URL-safe characters: {}",
            pkce.challenge
        );
    }

    #[test]
    fn test_challenge_deterministic() {
        let pkce = Pkce::generate();
        let mut hasher = Sha256::new();
        hasher.update(pkce.verifier.as_bytes());
        let hash = hasher.finalize();
        let expected = URL_SAFE_NO_PAD.encode(hash);
        assert_eq!(pkce.challenge, expected);
    }

    #[test]
    fn test_verification_failure_wrong_verifier() {
        let pkce = Pkce::generate();
        assert!(!Pkce::verify("wrong_verifier", &pkce.challenge));
    }

    #[test]
    fn test_verification_failure_wrong_challenge() {
        let pkce = Pkce::generate();
        assert!(!Pkce::verify(&pkce.verifier, "wrong_challenge"));
    }

    #[test]
    fn test_unique_generation() {
        let pkce1 = Pkce::generate();
        let pkce2 = Pkce::generate();
        assert_ne!(pkce1.verifier, pkce2.verifier);
        assert_ne!(pkce1.challenge, pkce2.challenge);
    }

    #[test]
    fn test_clone() {
        let pkce = Pkce::generate();
        let cloned = pkce.clone();
        assert_eq!(pkce.verifier, cloned.verifier);
        assert_eq!(pkce.challenge, cloned.challenge);
        assert_eq!(pkce.method, cloned.method);
    }
}
