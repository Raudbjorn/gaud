//! Model listing via Kiro API.

use tracing::{debug, info, warn};

use crate::config;
use crate::error::Result;
use crate::transport::http::KiroHttpClient;

/// Fetch available models from Kiro's ListAvailableModels endpoint.
pub async fn list_models(
    http: &KiroHttpClient,
    region: &str,
    profile_arn: Option<&str>,
) -> Result<Vec<String>> {
    let url = config::list_models_url(region, profile_arn)?;
    info!("Fetching available models from {}", url);

    match http.get(&url).await {
        Ok(response) => {
            let body: serde_json::Value = response.json().await.map_err(|e| {
                crate::error::Error::Conversion(format!("Failed to parse model list: {}", e))
            })?;

            let mut models: Vec<String> = Vec::new();

            // Extract model IDs from response
            if let Some(model_list) = body.get("models").and_then(|v| v.as_array()) {
                for model in model_list {
                    if let Some(id) = model.get("modelId").and_then(|v| v.as_str()) {
                        models.push(id.to_string());
                    }
                }
            }

            // Add hidden models
            for (name, _id) in config::hidden_models() {
                if !models.contains(&name.to_string()) {
                    models.push(name.to_string());
                }
            }

            debug!(count = models.len(), "Models fetched");
            Ok(models)
        }
        Err(e) => {
            warn!("Failed to fetch models: {}. Using fallback list.", e);
            Ok(config::fallback_models()
                .into_iter()
                .map(|s| s.to_string())
                .collect())
        }
    }
}
