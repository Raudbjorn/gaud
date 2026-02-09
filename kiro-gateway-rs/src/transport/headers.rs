//! Kiro API header construction.

use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use uuid::Uuid;

use crate::auth::constants;

/// Build the standard headers for Kiro API requests.
pub fn kiro_api_headers(access_token: &str, fingerprint: &str) -> HeaderMap {
    let mut headers = HeaderMap::new();

    headers.insert(
        reqwest::header::AUTHORIZATION,
        HeaderValue::from_str(&format!("Bearer {}", access_token)).unwrap_or_else(|_| {
            HeaderValue::from_static("Bearer invalid")
        }),
    );

    headers.insert(
        reqwest::header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );

    headers.insert(
        reqwest::header::USER_AGENT,
        HeaderValue::from_str(&constants::user_agent(fingerprint)).unwrap_or_else(|_| {
            HeaderValue::from_static("kiro-gateway-rs")
        }),
    );

    headers.insert(
        HeaderName::from_static("x-amz-user-agent"),
        HeaderValue::from_str(&constants::amz_user_agent(fingerprint)).unwrap_or_else(|_| {
            HeaderValue::from_static("kiro-gateway-rs")
        }),
    );

    headers.insert(
        HeaderName::from_static("x-amzn-codewhisperer-optout"),
        HeaderValue::from_static("true"),
    );

    headers.insert(
        HeaderName::from_static("x-amzn-kiro-agent-mode"),
        HeaderValue::from_static("vibe"),
    );

    // Unique invocation ID for request tracing
    headers.insert(
        HeaderName::from_static("amz-sdk-invocation-id"),
        HeaderValue::from_str(&Uuid::new_v4().to_string()).unwrap_or_else(|_| {
            HeaderValue::from_static("00000000-0000-0000-0000-000000000000")
        }),
    );

    headers.insert(
        HeaderName::from_static("amz-sdk-request"),
        HeaderValue::from_static("attempt=1; max=3"),
    );

    headers
}

/// Build headers for streaming requests (adds Connection: close to prevent CLOSE_WAIT).
pub fn kiro_streaming_headers(access_token: &str, fingerprint: &str) -> HeaderMap {
    let mut headers = kiro_api_headers(access_token, fingerprint);

    headers.insert(
        reqwest::header::CONNECTION,
        HeaderValue::from_static("close"),
    );

    headers
}
