use crate::cache::types::CacheError;
use url::Url;
use std::net::IpAddr;

/// Call an OpenAI-compatible `/v1/embeddings` endpoint and return the vector.
pub async fn embed(
    url: &str,
    model: &str,
    text: &str,
    api_key: Option<&str>,
) -> Result<Vec<f32>, CacheError> {
    // SSRF Protection: Validate URL before making request
    ensure_safe_url(url).await?;

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
async fn ensure_safe_url(url_str: &str) -> Result<(), CacheError> {
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
            // Check for private ranges
            // 10.0.0.0/8
            // 172.16.0.0/12
            // 192.168.0.0/16
            // 127.0.0.0/8 (Loopback)
            // 169.254.0.0/16 (Link-local)
            let octets = ipv4.octets();
            !ipv4.is_private()
            && !ipv4.is_loopback()
            && !ipv4.is_link_local()
            // Manual private check if is_private() is not stable or sufficient in MSRV
            && !(octets[0] == 10)
            && !(octets[0] == 172 && octets[1] >= 16 && octets[1] <= 31)
            && !(octets[0] == 192 && octets[1] == 168)
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
    async fn test_ensure_safe_url() {
        // Safe public URLs
        assert!(ensure_safe_url("https://api.openai.com/v1/embeddings").await.is_ok());
        assert!(ensure_safe_url("https://www.google.com").await.is_ok());

        // Unsafe local/private URLs
        // Note: These rely on local DNS resolving localhost/127.0.0.1 correctly
        assert!(ensure_safe_url("http://localhost:8080").await.is_err());
        assert!(ensure_safe_url("http://127.0.0.1:8080").await.is_err());
        assert!(ensure_safe_url("http://10.0.0.5:8080").await.is_err());
        assert!(ensure_safe_url("http://192.168.1.1:8080").await.is_err());
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
