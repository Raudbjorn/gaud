//! Configuration constants and URL templates for the Kiro API.

use std::time::Duration;

/// Default AWS region.
pub const DEFAULT_REGION: &str = "us-east-1";

/// Token refresh threshold - refresh when token expires within this window.
pub const TOKEN_REFRESH_THRESHOLD: Duration = Duration::from_secs(600); // 10 minutes

/// Safety margin for token expiry checks.
pub const EXPIRY_SAFETY_MARGIN: Duration = Duration::from_secs(60);

/// Maximum number of retry attempts.
pub const MAX_RETRIES: u32 = 3;

/// Base delay between retry attempts (exponential backoff: delay * 2^attempt).
pub const BASE_RETRY_DELAY: Duration = Duration::from_secs(1);

/// Timeout for first token in streaming responses.
pub const FIRST_TOKEN_TIMEOUT: Duration = Duration::from_secs(15);

/// Read timeout for streaming responses (between chunks).
pub const STREAMING_READ_TIMEOUT: Duration = Duration::from_secs(300);

/// Connect timeout for HTTP requests.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);

/// Default timeout for non-streaming requests.
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(300);

/// Maximum tool name length (Kiro API limit).
pub const MAX_TOOL_NAME_LENGTH: usize = 64;

/// Maximum tool description length before overflow to system prompt.
pub const MAX_TOOL_DESCRIPTION_LENGTH: usize = 10_000;

/// Default max input tokens for context usage calculation.
pub const DEFAULT_MAX_INPUT_TOKENS: u32 = 200_000;

/// Model cache TTL.
pub const MODEL_CACHE_TTL: Duration = Duration::from_secs(3600);

/// Kiro Desktop Auth refresh URL template.
/// `{region}` is replaced at runtime.
pub const KIRO_REFRESH_URL_TEMPLATE: &str =
    "https://prod.{region}.auth.desktop.kiro.dev/refreshToken";

/// AWS SSO OIDC token URL template.
pub const AWS_SSO_OIDC_URL_TEMPLATE: &str = "https://oidc.{region}.amazonaws.com/token";

/// Kiro API host template (generateAssistantResponse, ListAvailableModels).
pub const KIRO_API_HOST_TEMPLATE: &str = "https://q.{region}.amazonaws.com";

/// Kiro API origin query param.
pub const API_ORIGIN: &str = "AI_EDITOR";

/// Validate that a region string matches the expected AWS region format.
///
/// Valid format: `xx-xxxx-N` (e.g., `us-east-1`, `eu-west-2`, `ap-southeast-1`).
fn validate_region(region: &str) -> Result<(), crate::error::Error> {
    use std::sync::LazyLock;
    static REGION_RE: LazyLock<regex_lite::Regex> =
        LazyLock::new(|| regex_lite::Regex::new(r"^[a-z]{2}-[a-z]+-\d+$").unwrap());
    if REGION_RE.is_match(region) {
        Ok(())
    } else {
        Err(crate::error::Error::Config(format!(
            "Invalid AWS region format: '{}' (expected pattern like 'us-east-1')",
            region
        )))
    }
}

/// Percent-encode a string for use in URL query parameters.
fn url_encode(s: &str) -> String {
    // Encode characters that are not unreserved per RFC 3986
    let mut encoded = String::with_capacity(s.len() * 3);
    for byte in s.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                encoded.push(byte as char);
            }
            _ => {
                encoded.push_str(&format!("%{:02X}", byte));
            }
        }
    }
    encoded
}

/// Returns the Kiro Desktop Auth refresh URL for the given region.
pub fn kiro_refresh_url(region: &str) -> Result<String, crate::error::Error> {
    validate_region(region)?;
    Ok(KIRO_REFRESH_URL_TEMPLATE.replace("{region}", region))
}

/// Returns the AWS SSO OIDC token URL for the given region.
pub fn aws_sso_oidc_url(region: &str) -> Result<String, crate::error::Error> {
    validate_region(region)?;
    Ok(AWS_SSO_OIDC_URL_TEMPLATE.replace("{region}", region))
}

/// Returns the Kiro API host for the given region.
pub fn kiro_api_host(region: &str) -> Result<String, crate::error::Error> {
    validate_region(region)?;
    Ok(KIRO_API_HOST_TEMPLATE.replace("{region}", region))
}

/// Returns the generateAssistantResponse URL for the given region.
pub fn generate_assistant_response_url(region: &str, profile_arn: Option<&str>) -> Result<String, crate::error::Error> {
    let host = kiro_api_host(region)?;
    match profile_arn {
        Some(arn) => Ok(format!(
            "{}/generateAssistantResponse?origin={}&profileArn={}",
            host, API_ORIGIN, url_encode(arn)
        )),
        None => Ok(format!("{}/generateAssistantResponse?origin={}", host, API_ORIGIN)),
    }
}

/// Returns the ListAvailableModels URL for the given region.
pub fn list_models_url(region: &str, profile_arn: Option<&str>) -> Result<String, crate::error::Error> {
    let host = kiro_api_host(region)?;
    match profile_arn {
        Some(arn) => Ok(format!(
            "{}/ListAvailableModels?origin={}&profileArn={}",
            host, API_ORIGIN, url_encode(arn)
        )),
        None => Ok(format!("{}/ListAvailableModels?origin={}", host, API_ORIGIN)),
    }
}

/// Hidden models - not returned by Kiro ListAvailableModels but still functional.
pub fn hidden_models() -> Vec<(&'static str, &'static str)> {
    vec![("claude-3.7-sonnet", "CLAUDE_3_7_SONNET_20250219_V1_0")]
}

/// Fallback models when ListAvailableModels is unreachable.
pub fn fallback_models() -> Vec<&'static str> {
    vec![
        "auto",
        "claude-sonnet-4",
        "claude-haiku-4.5",
        "claude-sonnet-4.5",
        "claude-opus-4.5",
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_region_valid() {
        assert!(validate_region("us-east-1").is_ok());
        assert!(validate_region("eu-west-2").is_ok());
        assert!(validate_region("ap-southeast-1").is_ok());
    }

    #[test]
    fn test_validate_region_invalid() {
        assert!(validate_region("invalid").is_err());
        assert!(validate_region("US-EAST-1").is_err());
        assert!(validate_region("us-east-").is_err());
        assert!(validate_region("../etc/passwd").is_err());
        assert!(validate_region("us-east-1; DROP TABLE").is_err());
    }

    #[test]
    fn test_url_encode_arn() {
        let arn = "arn:aws:q:us-east-1:123456789012:profile/abc-123";
        let encoded = url_encode(arn);
        assert!(encoded.contains("%3A")); // colons encoded
        assert!(encoded.contains("%2F")); // slashes encoded
        assert!(!encoded.contains(':'));
        assert!(!encoded.contains('/'));
    }

    #[test]
    fn test_generate_url_encodes_arn() {
        let url = generate_assistant_response_url("us-east-1", Some("arn:aws:q:us-east-1:123:profile/x")).unwrap();
        assert!(!url.contains("arn:aws")); // raw ARN should not appear
        assert!(url.contains("profileArn=arn%3Aaws"));
    }

    #[test]
    fn test_generate_url_no_arn() {
        let url = generate_assistant_response_url("us-east-1", None).unwrap();
        assert!(!url.contains("profileArn"));
    }

    #[test]
    fn test_invalid_region_rejected() {
        assert!(generate_assistant_response_url("evil-region; DROP", None).is_err());
        assert!(kiro_refresh_url("../hack").is_err());
    }
}
