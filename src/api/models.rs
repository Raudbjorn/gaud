use axum::Json;
use axum::extract::State;
use chrono::Utc;

use crate::AppState;
use crate::error::AppError;
use crate::providers::types::{ModelInfo, ModelsResponse};

/// GET /v1/models
///
/// Returns the list of all available models across all configured providers.
/// Compatible with the OpenAI `GET /v1/models` response format.
pub async fn list_models(State(state): State<AppState>) -> Result<Json<ModelsResponse>, AppError> {
    let router = state.router.read().await;
    let available = router.available_models();

    let now = Utc::now().timestamp();
    let models: Vec<ModelInfo> = available
        .into_iter()
        .map(|(model_id, provider_id)| ModelInfo {
            id: model_id,
            object: "model".to_string(),
            created: now,
            owned_by: provider_id,
        })
        .collect();

    Ok(Json(ModelsResponse {
        object: "list".to_string(),
        data: models,
    }))
}

#[cfg(test)]
mod tests {
    use crate::providers::types::{ModelInfo, ModelsResponse};

    #[test]
    fn test_models_response_format() {
        let response = ModelsResponse {
            object: "list".to_string(),
            data: vec![ModelInfo {
                id: "claude-3-sonnet".to_string(),
                object: "model".to_string(),
                created: 1700000000,
                owned_by: "anthropic".to_string(),
            }],
        };

        let json = serde_json::to_value(&response).unwrap();
        assert_eq!(json["object"], "list");
        assert_eq!(json["data"][0]["id"], "claude-3-sonnet");
        assert_eq!(json["data"][0]["object"], "model");
        assert_eq!(json["data"][0]["owned_by"], "anthropic");
    }

    #[test]
    fn test_models_response_empty() {
        let response = ModelsResponse {
            object: "list".to_string(),
            data: vec![],
        };

        let json = serde_json::to_value(&response).unwrap();
        assert!(json["data"].as_array().unwrap().is_empty());
    }
}
