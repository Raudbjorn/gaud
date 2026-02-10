use crate::cache::types::CacheError;
use url::Url;
use std::net::IpAddr;

/// Call an OpenAI-compatible `/v1/embeddings` endpoint and return the vector.
///
/// When `allow_local` is `true`, the SSRF protection is bypassed so that
/// local embedding servers (e.g. Ollama on `localhost:11434`, a vLLM
/// instance on `192.168.x.x`) can be used.
pub async fn embed(
    url: &str,
    model: &str,
    text: &str,
    api_key: Option<&str>,
    allow_local: bool,
) -> Result<Vec<f32>, CacheError> {
    // SSRF Protection: Validate URL before making request
    ensure_safe_url(url, allow_local).await?;

    let client = reqwest::Client::new();

    let body = serde_json::json!({
        "model": model,
        "input": text,
    });

    let mut req = client.post(url).json(&body);
    if let Some(key) = api_key {
        req = req.bearer_auth(key);
    }

    let resp = req
        .send()
        .await
        .map_err(|e| CacheError::Embedding(format!("HTTP request failed: {e}")))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(CacheError::Embedding(format!(
            "Embedding API returned {status}: {body}"
        )));
    }

    let resp_body: EmbeddingApiResponse = resp
        .json()
        .await
        .map_err(|e| CacheError::Embedding(format!("Failed to parse response: {e}")))?;

    resp_body
        .data
        .into_iter()
        .next()
        .map(|d| d.embedding)
        .ok_or_else(|| CacheError::Embedding("Empty embedding response".into()))
}

/// Validate that the URL does not point to a local or private network address (SSRF protection).
///
/// When `allow_local` is `true` the check is skipped entirely, allowing
/// users to point at self-hosted embedding servers on localhost or private
/// networks (e.g. Ollama, vLLM, TEI).
async fn ensure_safe_url(url_str: &str, allow_local: bool) -> Result<(), CacheError> {
    if allow_local {
        return Ok(());
    }

    let url = Url::parse(url_str)
        .map_err(|e| CacheError::InvalidConfig(format!("Invalid embedding URL: {e}")))?;

    let host_str = url.host_str()
        .ok_or_else(|| CacheError::InvalidConfig("URL missing host".into()))?;

    // Resolve hostname to IPs
    let addrs = tokio::net::lookup_host((host_str, 80)) // Port doesn't matter for resolution
        .await
        .map_err(|e| CacheError::Embedding(format!("DNS resolution failed for {host_str}: {e}")))?;

    for addr in addrs {
        if !is_public_ip(&addr.ip()) {
             return Err(CacheError::InvalidConfig(format!(
                "SSRF Protection: URL resolves to private/local address {}",
                addr.ip()
            )));
        }
    }

    Ok(())
}

fn is_public_ip(addr: &IpAddr) -> bool {
    match addr {
        IpAddr::V4(ipv4) => {
            !ipv4.is_private()
            && !ipv4.is_loopback()
            && !ipv4.is_link_local()
        }
        IpAddr::V6(ipv6) => {
            !ipv6.is_loopback() && !ipv6.is_unique_local() && !ipv6.is_unicast_link_local()
        }
    }
}

// ---------------------------------------------------------------------------
// Response types (minimal, just what we need)
// ---------------------------------------------------------------------------

#[derive(serde::Deserialize)]
struct EmbeddingApiResponse {
    data: Vec<EmbeddingDataItem>,
}

#[derive(serde::Deserialize)]
struct EmbeddingDataItem {
    embedding: Vec<f32>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_ensure_safe_url_blocks_private() {
        // Safe public URLs (allow_local = false)
        assert!(ensure_safe_url("https://api.openai.com/v1/embeddings", false).await.is_ok());
        assert!(ensure_safe_url("https://www.google.com", false).await.is_ok());

        // Unsafe local/private URLs (allow_local = false)
        assert!(ensure_safe_url("http://localhost:8080", false).await.is_err());
        assert!(ensure_safe_url("http://127.0.0.1:8080", false).await.is_err());
        assert!(ensure_safe_url("http://10.0.0.5:8080", false).await.is_err());
        assert!(ensure_safe_url("http://192.168.1.1:8080", false).await.is_err());
    }

    #[tokio::test]
    async fn test_ensure_safe_url_allow_local() {
        // When allow_local is true, local addresses should be accepted
        assert!(ensure_safe_url("http://localhost:11434", true).await.is_ok());
        assert!(ensure_safe_url("http://127.0.0.1:8080", true).await.is_ok());
        assert!(ensure_safe_url("http://192.168.1.50:8080", true).await.is_ok());
    }

    #[test]
    fn test_embedding_response_parse() {
        let json = r#"{
            "object": "list",
            "data": [{"object": "embedding", "embedding": [0.1, 0.2, 0.3], "index": 0}],
            "model": "text-embedding-3-small",
            "usage": {"prompt_tokens": 5, "total_tokens": 5}
        }"#;
        let resp: super::EmbeddingApiResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.data.len(), 1);
        assert_eq!(resp.data[0].embedding.len(), 3);
    }

    #[test]
    fn test_empty_embedding_response() {
        let json = r#"{"object": "list", "data": [], "model": "test", "usage": {"prompt_tokens": 0, "total_tokens": 0}}"#;
        let resp: super::EmbeddingApiResponse = serde_json::from_str(json).unwrap();
        assert!(resp.data.is_empty());
    }
}
