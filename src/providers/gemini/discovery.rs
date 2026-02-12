//! Project discovery and management for Cloud Code API.
//!
//! This module handles discovering and managing Cloud Code project IDs:
//!
//! - Calling the `loadCodeAssist` API to discover project information
//! - Detecting subscription tier (Free, Pro, Ultra)
//! - Onboarding new users when no project exists
//!
//! # Project Discovery Flow
//!
//! 1. Call `loadCodeAssist` API with fallback endpoints
//! 2. Parse response for `cloudaicompanionProject` and tier info
//! 3. If no project exists, call `onboardUser` to create one
//! 4. Return `ProjectInfo` with project ID and subscription tier

use serde::{Deserialize, Serialize};
use tracing::{debug, info, instrument, warn};

use crate::providers::gemini::constants::{
    API_PATH_LOAD_CODE_ASSIST, API_PATH_ONBOARD_USER, DEFAULT_PROJECT_ID,
    LOAD_CODE_ASSIST_ENDPOINTS,
};
use crate::providers::gemini::error::{AuthError, Error, Result};

/// Subscription tier for Cloud Code.
///
/// Determines available models and quotas.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SubscriptionTier {
    /// Free tier with limited quotas.
    Free,
    /// Pro tier with higher quotas.
    Pro,
    /// Ultra tier with highest quotas.
    Ultra,
    /// Unknown or undetected tier.
    Unknown,
}

impl std::fmt::Display for SubscriptionTier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SubscriptionTier::Free => write!(f, "free"),
            SubscriptionTier::Pro => write!(f, "pro"),
            SubscriptionTier::Ultra => write!(f, "ultra"),
            SubscriptionTier::Unknown => write!(f, "unknown"),
        }
    }
}

impl std::str::FromStr for SubscriptionTier {
    type Err = std::convert::Infallible;

    /// Parse a tier string into a SubscriptionTier.
    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        let lower = s.to_lowercase();
        let tier = if lower.contains("free") || lower.contains("basic") {
            SubscriptionTier::Free
        } else if lower.contains("pro") {
            SubscriptionTier::Pro
        } else if lower.contains("ultra") || lower.contains("max") {
            SubscriptionTier::Ultra
        } else {
            SubscriptionTier::Unknown
        };
        Ok(tier)
    }
}

impl SubscriptionTier {
    /// Parse a tier string into a SubscriptionTier (convenience method).
    pub fn parse(s: &str) -> Self {
        s.parse().unwrap_or(SubscriptionTier::Unknown)
    }
}

/// Information about a Cloud Code project.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectInfo {
    /// The Cloud Code project ID.
    pub project_id: String,

    /// The managed project ID (cloudaicompanionProject).
    pub managed_project_id: Option<String>,

    /// The detected subscription tier.
    pub subscription_tier: SubscriptionTier,
}

impl ProjectInfo {
    /// Create a new ProjectInfo.
    pub fn new(
        project_id: String,
        managed_project_id: Option<String>,
        subscription_tier: SubscriptionTier,
    ) -> Self {
        Self {
            project_id,
            managed_project_id,
            subscription_tier,
        }
    }

    /// Create a default ProjectInfo for fallback scenarios.
    pub fn default_fallback() -> Self {
        Self {
            project_id: DEFAULT_PROJECT_ID.to_string(),
            managed_project_id: None,
            subscription_tier: SubscriptionTier::Unknown,
        }
    }
}

/// Response from the loadCodeAssist API.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LoadCodeAssistResponse {
    /// The managed project ID.
    #[serde(default)]
    cloudaicompanion_project: Option<String>,

    /// Project ID for the user.
    #[serde(default)]
    project: Option<String>,

    /// Subscription tier (Pro/Ultra).
    #[serde(default)]
    paid_tier: Option<String>,

    /// Current tier (alternative field).
    #[serde(default)]
    current_tier: Option<String>,
}

/// Response from the onboardUser API.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct OnboardUserResponse {
    /// The created project ID.
    project: Option<String>,
}

/// Discover project information by calling the Cloud Code API.
#[instrument(skip(token))]
pub async fn discover_project(token: &str, hint_project_id: Option<&str>) -> Result<ProjectInfo> {
    // Try each endpoint in order
    let mut last_error: Option<Error> = None;

    for endpoint in LOAD_CODE_ASSIST_ENDPOINTS {
        let url = format!("{}{}", endpoint, API_PATH_LOAD_CODE_ASSIST);
        debug!(endpoint = %endpoint, "Trying loadCodeAssist endpoint");

        match try_load_code_assist(token, &url, hint_project_id).await {
            Ok(response) => {
                // Parse the response into ProjectInfo
                let project_id = response
                    .project
                    .or(response.cloudaicompanion_project.clone())
                    .unwrap_or_else(|| DEFAULT_PROJECT_ID.to_string());

                let managed_project_id = response.cloudaicompanion_project;

                // Detect subscription tier from paidTier or currentTier
                let tier = response
                    .paid_tier
                    .as_deref()
                    .or(response.current_tier.as_deref())
                    .map(SubscriptionTier::parse)
                    .unwrap_or(SubscriptionTier::Unknown);

                info!(
                    project_id = %project_id,
                    tier = %tier,
                    "Discovered project successfully"
                );

                return Ok(ProjectInfo::new(project_id, managed_project_id, tier));
            }
            Err(e) => {
                warn!(endpoint = %endpoint, error = %e, "Failed to load code assist");

                // If we get a 403/404, try the next endpoint
                if matches!(&e, Error::Api { status, .. } if *status == 403 || *status == 404) {
                    last_error = Some(e);
                    continue;
                }

                // For other errors, try onboarding
                if matches!(&e, Error::Api { status, .. } if *status >= 400 && *status < 500) {
                    debug!("Attempting user onboarding");
                    match try_onboard_user(token, endpoint).await {
                        Ok(project_id) => {
                            info!(project_id = %project_id, "User onboarded successfully");
                            return Ok(ProjectInfo::new(project_id, None, SubscriptionTier::Free));
                        }
                        Err(onboard_err) => {
                            warn!(error = %onboard_err, "User onboarding failed");
                            last_error = Some(onboard_err);
                            continue;
                        }
                    }
                }

                last_error = Some(e);
            }
        }
    }

    // All endpoints failed, return fallback with last error logged
    if let Some(e) = last_error {
        warn!(error = %e, "All endpoints failed, using fallback project ID");
    }

    Ok(ProjectInfo::default_fallback())
}

/// Try to call loadCodeAssist API at a specific endpoint.
async fn try_load_code_assist(
    token: &str,
    url: &str,
    hint_project_id: Option<&str>,
) -> Result<LoadCodeAssistResponse> {
    let client = reqwest::Client::new();

    let mut request = client
        .post(url)
        .header("Authorization", format!("Bearer {}", token))
        .header("Content-Type", "application/json");

    // Add project hint if provided
    let body = if let Some(project_id) = hint_project_id {
        serde_json::json!({
            "project": project_id
        })
    } else {
        serde_json::json!({})
    };

    request = request.json(&body);

    let response = request.send().await?;

    let status = response.status();
    if !status.is_success() {
        let body = response.text().await?;
        return Err(Error::api(status.as_u16(), body, None));
    }

    let response: LoadCodeAssistResponse = response.json().await?;
    Ok(response)
}

/// Onboard a new user to Cloud Code.
#[instrument(skip(token))]
pub async fn onboard_user(token: &str, tier: &str) -> Result<String> {
    // Try each endpoint
    for endpoint in LOAD_CODE_ASSIST_ENDPOINTS {
        match try_onboard_user(token, endpoint).await {
            Ok(project_id) => return Ok(project_id),
            Err(e) => {
                warn!(endpoint = %endpoint, error = %e, "Onboarding failed at endpoint");
                continue;
            }
        }
    }

    Err(Error::Auth(AuthError::ProjectDiscovery(
        "Failed to onboard user at all endpoints".to_string(),
    )))
}

/// Try to onboard user at a specific endpoint.
async fn try_onboard_user(token: &str, endpoint: &str) -> Result<String> {
    let url = format!("{}{}", endpoint, API_PATH_ONBOARD_USER);

    let client = reqwest::Client::new();
    let response = client
        .post(&url)
        .header("Authorization", format!("Bearer {}", token))
        .header("Content-Type", "application/json")
        .json(&serde_json::json!({}))
        .send()
        .await?;

    let status = response.status();
    if !status.is_success() {
        let body = response.text().await?;
        return Err(Error::api(status.as_u16(), body, None));
    }

    let response: OnboardUserResponse = response.json().await?;

    response.project.ok_or_else(|| {
        Error::Auth(AuthError::ProjectDiscovery(
            "No project ID in onboard response".to_string(),
        ))
    })
}
