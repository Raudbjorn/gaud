use axum::Json;
use axum::extract::State;
use serde::Serialize;

use crate::AppState;
use crate::providers::types::ProviderStatus;

#[derive(Debug, Serialize)]
pub struct HealthResponse {
    pub status: String,
    pub providers: Vec<ProviderStatus>,
}

/// GET /health
///
/// Returns overall system health and per-provider status.
/// No authentication required.
pub async fn health_check(State(state): State<AppState>) -> Json<HealthResponse> {
    let router = state.router.read().await;

    // Build provider status from the router's available data.
    let provider_ids = router.provider_ids().to_vec();
    let all_models = router.available_models();

    let providers: Vec<ProviderStatus> = provider_ids
        .iter()
        .map(|id| {
            let models: Vec<String> = all_models
                .iter()
                .filter(|(_, provider_id)| provider_id == id)
                .map(|(model, _)| model.clone())
                .collect();

            let healthy = router
                .circuit_state(id)
                .map(|s| s != crate::providers::health::CircuitState::Open)
                .unwrap_or(false);

            let latency_ms = router.stats(id).map(|s| s.avg_latency_ms());

            ProviderStatus {
                provider: id.clone(),
                healthy,
                models,
                latency_ms,
            }
        })
        .collect();

    Json(HealthResponse {
        status: "ok".to_string(),
        providers,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_health_response_serialization() {
        let response = HealthResponse {
            status: "ok".to_string(),
            providers: vec![ProviderStatus {
                provider: "test".to_string(),
                healthy: true,
                models: vec!["model-1".to_string()],
                latency_ms: Some(42),
            }],
        };

        let json = serde_json::to_value(&response).unwrap();
        assert_eq!(json["status"], "ok");
        assert_eq!(json["providers"][0]["provider"], "test");
        assert_eq!(json["providers"][0]["healthy"], true);
    }

    #[test]
    fn test_health_response_empty_providers() {
        let response = HealthResponse {
            status: "ok".to_string(),
            providers: vec![],
        };

        let json = serde_json::to_value(&response).unwrap();
        assert_eq!(json["status"], "ok");
        assert!(json["providers"].as_array().unwrap().is_empty());
    }
}
