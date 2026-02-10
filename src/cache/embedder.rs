use crate::cache::types::CacheError;

/// Call an OpenAI-compatible `/v1/embeddings` endpoint and return the vector.
pub async fn embed(
    url: &str,
    model: &str,
    text: &str,
    api_key: Option<&str>,
) -> Result<Vec<f32>, CacheError> {
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
