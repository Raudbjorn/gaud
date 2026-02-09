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
    // Try /etc/hostname first (common on Linux)
    if let Ok(name) = std::fs::read_to_string("/etc/hostname") {
        let name = name.trim().to_string();
        if !name.is_empty() {
            return name;
        }
    }

    // Fall back to `hostname` command (works on Linux, macOS, BSDs, Windows)
    if let Ok(output) = std::process::Command::new("hostname").output() {
        if output.status.success() {
            let name = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if !name.is_empty() {
                return name;
            }
        }
    }

    "unknown".into()
}

fn get_username() -> String {
    std::env::var("USER")
        .or_else(|_| std::env::var("USERNAME"))
        .unwrap_or_else(|_| "unknown".into())
}

/// Kiro IDE version string used in User-Agent.
pub const KIRO_IDE_VERSION: &str = "KiroIDE-0.7.45";

/// AWS SDK version string used in User-Agent.
pub const AWS_SDK_VERSION: &str = "aws-sdk-js/1.0.27";

/// Build the User-Agent header value.
pub fn user_agent(fingerprint: &str) -> String {
    format!(
        "{} ua/2.1 os/win32#10.0.19044 lang/js md/nodejs#22.21.1 api/codewhispererstreaming#1.0.27 m/E {}-{}",
        AWS_SDK_VERSION, KIRO_IDE_VERSION, fingerprint
    )
}

/// Build the x-amz-user-agent header value.
pub fn amz_user_agent(fingerprint: &str) -> String {
    format!("{} {}-{}", AWS_SDK_VERSION, KIRO_IDE_VERSION, fingerprint)
}
