use axum::Json;
use axum::http::StatusCode;
use serde::Serialize;

use crate::providers::types::EmbeddingRequest;

#[derive(Debug, Serialize)]
pub struct NotImplementedResponse {
    error: NotImplementedError,
}

#[derive(Debug, Serialize)]
struct NotImplementedError {
    message: String,
    r#type: String,
    code: String,
}

/// POST /v1/embeddings
///
/// Placeholder endpoint that returns 501 Not Implemented.
/// Embedding support will be added in a future release.
pub async fn create_embedding(
    Json(_request): Json<EmbeddingRequest>,
) -> (StatusCode, Json<NotImplementedResponse>) {
    (
        StatusCode::NOT_IMPLEMENTED,
        Json(NotImplementedResponse {
            error: NotImplementedError {
                message: "Embeddings are not yet supported. This feature will be available in a future release.".to_string(),
                r#type: "not_implemented_error".to_string(),
                code: "not_implemented".to_string(),
            },
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_not_implemented_response_serialization() {
        let response = NotImplementedResponse {
            error: NotImplementedError {
                message: "not yet".to_string(),
                r#type: "not_implemented_error".to_string(),
                code: "not_implemented".to_string(),
            },
        };

        let json = serde_json::to_value(&response).unwrap();
        assert_eq!(json["error"]["type"], "not_implemented_error");
        assert_eq!(json["error"]["code"], "not_implemented");
    }
}
