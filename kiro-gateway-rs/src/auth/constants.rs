//! Auth constants: User-Agent strings, fingerprint generation.

use sha2::{Digest, Sha256};

/// Generate a machine fingerprint from hostname + username.
pub fn machine_fingerprint() -> String {
    let hostname = get_hostname();
    let username = get_username();
    let input = format!("{}-{}-kiro-gateway", hostname, username);
    let hash = Sha256::digest(input.as_bytes());
    hash.iter().map(|b| format!("{:02x}", b)).collect()
}

fn get_hostname() -> String {
    #[cfg(unix)]
    {
        // Try /etc/hostname first (most common on Linux)
        if let Ok(name) = std::fs::read_to_string("/etc/hostname") {
            let trimmed = name.trim();
            if !trimmed.is_empty() {
                return trimmed.to_string();
            }
        }
        // Fallback: run hostname command (works on macOS, BSDs, etc.)
        if let Ok(output) = std::process::Command::new("hostname").output() {
            if output.status.success() {
                let name = String::from_utf8_lossy(&output.stdout);
                let trimmed = name.trim();
                if !trimmed.is_empty() {
                    return trimmed.to_string();
                }
            }
        }
        "unknown".into()
    }
    #[cfg(not(unix))]
    {
        "unknown".into()
    }
}

fn get_username() -> String {
    std::env::var("USER")
        .or_else(|_| std::env::var("USERNAME"))
        .unwrap_or_else(|_| "unknown".into())
}

/// Library version string.
pub const GATEWAY_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Build the User-Agent header value.
///
/// Uses an honest User-Agent that identifies as gaud-kiro-gateway with actual
/// platform information. The Kiro API requires specific User-Agent components
/// (aws-sdk prefix, os, lang metadata) for request routing and compatibility,
/// so we include the required format while using truthful values.
pub fn user_agent(fingerprint: &str) -> String {
    let os = actual_os();
    format!(
        "gaud-kiro-gateway/{} ua/2.1 os/{} lang/rust m/E gaud-{}",
        GATEWAY_VERSION, os, fingerprint
    )
}

/// Build the x-amz-user-agent header value.
pub fn amz_user_agent(fingerprint: &str) -> String {
    format!("gaud-kiro-gateway/{} gaud-{}", GATEWAY_VERSION, fingerprint)
}

/// Get the actual OS identifier.
fn actual_os() -> &'static str {
    #[cfg(target_os = "linux")]
    { "linux" }
    #[cfg(target_os = "macos")]
    { "macos" }
    #[cfg(target_os = "windows")]
    { "windows" }
    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    { "unknown" }
}
