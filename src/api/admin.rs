use axum::extract::{Path, Query, State};
use axum::Extension;
use axum::Json;
use serde::{Deserialize, Serialize};

use crate::auth::users;
use crate::auth::AuthUser;
use crate::error::AppError;
use crate::AppState;

// ---------------------------------------------------------------------------
// Request / response types
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct CreateUserRequest {
    pub name: String,
    pub role: String,
}

#[derive(Debug, Deserialize)]
pub struct CreateApiKeyRequest {
    #[serde(default)]
    pub label: String,
}

#[derive(Debug, Serialize)]
pub struct CreatedApiKeyResponse {
    pub id: String,
    pub user_id: String,
    pub key_prefix: String,
    pub label: String,
    pub created_at: String,
    /// The full plaintext key. Shown exactly once.
    pub plaintext: String,
}

#[derive(Debug, Deserialize)]
pub struct SetBudgetRequest {
    pub monthly_limit: Option<f64>,
    pub daily_limit: Option<f64>,
}

#[derive(Debug, Deserialize)]
pub struct UsageQuery {
    pub user_id: Option<String>,
    pub provider: Option<String>,
    pub from: Option<String>,
    pub to: Option<String>,
    #[serde(default = "default_page")]
    pub page: u32,
    #[serde(default = "default_per_page")]
    pub per_page: u32,
}

fn default_page() -> u32 {
    1
}
fn default_per_page() -> u32 {
    50
}

#[derive(Debug, Serialize)]
pub struct UsageEntry {
    pub id: String,
    pub user_id: String,
    pub request_id: String,
    pub provider: String,
    pub model: String,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cost: f64,
    pub latency_ms: i64,
    pub status: String,
    pub created_at: String,
}

#[derive(Debug, Serialize)]
pub struct UsageResponse {
    pub data: Vec<UsageEntry>,
    pub page: u32,
    pub per_page: u32,
    pub total: i64,
}

#[derive(Debug, Serialize)]
pub struct DeletedResponse {
    pub deleted: bool,
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Ensure the calling user has the admin role.
fn require_admin(user: &AuthUser) -> Result<(), AppError> {
    if !user.is_admin() {
        return Err(AppError::Forbidden(
            "Admin role required".to_string(),
        ));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// User management
// ---------------------------------------------------------------------------

/// POST /admin/users
pub async fn create_user(
    State(state): State<AppState>,
    Extension(user): Extension<AuthUser>,
    Json(body): Json<CreateUserRequest>,
) -> Result<Json<users::User>, AppError> {
    require_admin(&user)?;
    let created = users::create_user(&state.db, &body.name, &body.role)?;
    Ok(Json(created))
}

/// GET /admin/users
pub async fn list_users(
    State(state): State<AppState>,
    Extension(user): Extension<AuthUser>,
) -> Result<Json<Vec<users::User>>, AppError> {
    require_admin(&user)?;
    let all = users::list_users(&state.db)?;
    Ok(Json(all))
}

/// DELETE /admin/users/:id
pub async fn delete_user(
    State(state): State<AppState>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<String>,
) -> Result<Json<DeletedResponse>, AppError> {
    require_admin(&user)?;
    users::delete_user(&state.db, &id)?;
    Ok(Json(DeletedResponse { deleted: true }))
}

// ---------------------------------------------------------------------------
// API key management
// ---------------------------------------------------------------------------

/// POST /admin/users/:id/keys
pub async fn create_api_key(
    State(state): State<AppState>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<String>,
    Json(body): Json<CreateApiKeyRequest>,
) -> Result<Json<CreatedApiKeyResponse>, AppError> {
    require_admin(&user)?;
    let created = users::create_api_key(&state.db, &id, &body.label)?;

    Ok(Json(CreatedApiKeyResponse {
        id: created.info.id,
        user_id: created.info.user_id,
        key_prefix: created.info.key_prefix,
        label: created.info.label,
        created_at: created.info.created_at,
        plaintext: created.plaintext,
    }))
}

/// GET /admin/users/:id/keys
pub async fn list_api_keys(
    State(state): State<AppState>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<String>,
) -> Result<Json<Vec<users::ApiKeyInfo>>, AppError> {
    require_admin(&user)?;
    let keys = users::list_api_keys(&state.db, &id)?;
    Ok(Json(keys))
}

/// DELETE /admin/keys/:id
pub async fn revoke_api_key(
    State(state): State<AppState>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<String>,
) -> Result<Json<DeletedResponse>, AppError> {
    require_admin(&user)?;
    users::revoke_api_key(&state.db, &id)?;
    Ok(Json(DeletedResponse { deleted: true }))
}

// ---------------------------------------------------------------------------
// Budget management
// ---------------------------------------------------------------------------

/// PUT /admin/budgets/:user_id
pub async fn set_budget(
    State(state): State<AppState>,
    Extension(user): Extension<AuthUser>,
    Path(user_id): Path<String>,
    Json(body): Json<SetBudgetRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    require_admin(&user)?;

    // Verify target user exists.
    users::get_user(&state.db, &user_id)?;

    state
        .budget
        .set_budget(&user_id, body.monthly_limit, body.daily_limit)?;

    let budget = state.budget.get_budget(&user_id)?;
    Ok(Json(serde_json::to_value(budget).unwrap()))
}

/// GET /admin/budgets/:user_id
pub async fn get_budget(
    State(state): State<AppState>,
    Extension(user): Extension<AuthUser>,
    Path(user_id): Path<String>,
) -> Result<Json<serde_json::Value>, AppError> {
    require_admin(&user)?;

    let budget = state.budget.get_budget(&user_id)?;
    match budget {
        Some(b) => Ok(Json(serde_json::to_value(b).unwrap())),
        None => Err(AppError::NotFound(format!(
            "No budget found for user '{user_id}'"
        ))),
    }
}

// ---------------------------------------------------------------------------
// Usage queries
// ---------------------------------------------------------------------------

/// GET /admin/usage
pub async fn query_usage(
    State(state): State<AppState>,
    Extension(user): Extension<AuthUser>,
    Query(params): Query<UsageQuery>,
) -> Result<Json<UsageResponse>, AppError> {
    require_admin(&user)?;

    let page = params.page.max(1);
    let per_page = params.per_page.clamp(1, 500);
    let offset = (page - 1) * per_page;

    // Build query dynamically based on provided filters.
    let mut where_clauses: Vec<String> = Vec::new();
    let mut bind_values: Vec<String> = Vec::new();

    if let Some(ref uid) = params.user_id {
        bind_values.push(uid.clone());
        where_clauses.push(format!("user_id = ?{}", bind_values.len()));
    }
    if let Some(ref provider) = params.provider {
        bind_values.push(provider.clone());
        where_clauses.push(format!("provider = ?{}", bind_values.len()));
    }
    if let Some(ref from) = params.from {
        bind_values.push(from.clone());
        where_clauses.push(format!("created_at >= ?{}", bind_values.len()));
    }
    if let Some(ref to) = params.to {
        bind_values.push(to.clone());
        where_clauses.push(format!("created_at <= ?{}", bind_values.len()));
    }

    let where_sql = if where_clauses.is_empty() {
        String::new()
    } else {
        format!("WHERE {}", where_clauses.join(" AND "))
    };

    let count_sql = format!("SELECT COUNT(*) FROM usage_log {where_sql}");
    let data_sql = format!(
        "SELECT id, user_id, request_id, provider, model, input_tokens, output_tokens, \
         cost, latency_ms, status, created_at \
         FROM usage_log {where_sql} ORDER BY created_at DESC LIMIT ?{} OFFSET ?{}",
        bind_values.len() + 1,
        bind_values.len() + 2,
    );

    let result = state.db.with_conn(|conn| {
        // Count total matching rows.
        let total: i64 = {
            let mut stmt = conn.prepare(&count_sql)?;
            let p: Vec<&dyn rusqlite::ToSql> =
                bind_values.iter().map(|v| v as &dyn rusqlite::ToSql).collect();
            stmt.query_row(p.as_slice(), |row| row.get(0))?
        };

        // Fetch the page of data.
        let mut data_params: Vec<Box<dyn rusqlite::ToSql>> =
            bind_values.iter().map(|v| Box::new(v.clone()) as Box<dyn rusqlite::ToSql>).collect();
        data_params.push(Box::new(per_page as i64));
        data_params.push(Box::new(offset as i64));

        let mut stmt = conn.prepare(&data_sql)?;
        let p: Vec<&dyn rusqlite::ToSql> = data_params.iter().map(|v| v.as_ref()).collect();
        let rows = stmt.query_map(p.as_slice(), |row| {
            Ok(UsageEntry {
                id: row.get(0)?,
                user_id: row.get(1)?,
                request_id: row.get(2)?,
                provider: row.get(3)?,
                model: row.get(4)?,
                input_tokens: row.get(5)?,
                output_tokens: row.get(6)?,
                cost: row.get(7)?,
                latency_ms: row.get(8)?,
                status: row.get(9)?,
                created_at: row.get(10)?,
            })
        })?;

        let data: Vec<UsageEntry> = rows.collect::<Result<Vec<_>, _>>()?;
        Ok((data, total))
    })?;

    let (data, total) = result;

    Ok(Json(UsageResponse {
        data,
        page,
        per_page,
        total,
    }))
}

// ---------------------------------------------------------------------------
// Settings management
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct UpdateSettingRequest {
    pub key: String,
    pub value: serde_json::Value,
}

/// GET /admin/settings
pub async fn get_settings(
    State(state): State<AppState>,
    Extension(user): Extension<AuthUser>,
) -> Result<Json<Vec<crate::config::SettingEntry>>, AppError> {
    require_admin(&user)?;
    let report = state.config.settings_report();
    Ok(Json(report))
}

/// PUT /admin/settings
pub async fn update_settings(
    State(state): State<AppState>,
    Extension(user): Extension<AuthUser>,
    Json(body): Json<UpdateSettingRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    require_admin(&user)?;

    // Reject changes to env-overridden settings.
    if state.config.env_overrides.is_overridden(&body.key) {
        return Err(AppError::BadRequest(format!(
            "Setting '{}' is overridden by environment variable '{}'. Unset the variable and restart to edit.",
            body.key,
            state.config.env_overrides.env_var_for(&body.key).unwrap_or("unknown")
        )));
    }

    // Create a mutable copy, update, and save.
    let mut config_copy = (*state.config).clone();
    config_copy
        .update_setting(&body.key, &body.value)
        .map_err(|e| AppError::BadRequest(e))?;
    config_copy
        .save(&state.config_path)
        .map_err(|e| AppError::Internal(e.to_string()))?;

    Ok(Json(serde_json::json!({
        "message": "Setting saved. Restart the server to apply changes.",
        "key": body.key
    })))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_require_admin_allows_admin() {
        let user = AuthUser {
            user_id: "u1".to_string(),
            name: "admin".to_string(),
            role: "admin".to_string(),
        };
        assert!(require_admin(&user).is_ok());
    }

    #[test]
    fn test_require_admin_rejects_member() {
        let user = AuthUser {
            user_id: "u1".to_string(),
            name: "member".to_string(),
            role: "member".to_string(),
        };
        assert!(require_admin(&user).is_err());
    }

    #[test]
    fn test_create_user_request_deserialization() {
        let json = r#"{"name": "alice", "role": "member"}"#;
        let req: CreateUserRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.name, "alice");
        assert_eq!(req.role, "member");
    }

    #[test]
    fn test_create_api_key_request_default_label() {
        let json = r#"{}"#;
        let req: CreateApiKeyRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.label, "");
    }

    #[test]
    fn test_set_budget_request_deserialization() {
        let json = r#"{"monthly_limit": 100.0, "daily_limit": 10.0}"#;
        let req: SetBudgetRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.monthly_limit, Some(100.0));
        assert_eq!(req.daily_limit, Some(10.0));
    }

    #[test]
    fn test_set_budget_request_optional_fields() {
        let json = r#"{"monthly_limit": 50.0}"#;
        let req: SetBudgetRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.monthly_limit, Some(50.0));
        assert_eq!(req.daily_limit, None);
    }

    #[test]
    fn test_usage_query_defaults() {
        let json = r#"{}"#;
        let query: UsageQuery = serde_json::from_str(json).unwrap();
        assert_eq!(query.page, 1);
        assert_eq!(query.per_page, 50);
        assert!(query.user_id.is_none());
        assert!(query.provider.is_none());
    }

    #[test]
    fn test_usage_entry_serialization() {
        let entry = UsageEntry {
            id: "id1".to_string(),
            user_id: "user1".to_string(),
            request_id: "req1".to_string(),
            provider: "claude".to_string(),
            model: "claude-3-sonnet".to_string(),
            input_tokens: 100,
            output_tokens: 50,
            cost: 0.001,
            latency_ms: 250,
            status: "success".to_string(),
            created_at: "2025-01-01 00:00:00".to_string(),
        };

        let json = serde_json::to_value(&entry).unwrap();
        assert_eq!(json["provider"], "claude");
        assert_eq!(json["input_tokens"], 100);
    }

    #[test]
    fn test_usage_response_serialization() {
        let response = UsageResponse {
            data: vec![],
            page: 1,
            per_page: 50,
            total: 0,
        };

        let json = serde_json::to_value(&response).unwrap();
        assert_eq!(json["page"], 1);
        assert_eq!(json["per_page"], 50);
        assert_eq!(json["total"], 0);
        assert!(json["data"].as_array().unwrap().is_empty());
    }

    #[test]
    fn test_deleted_response_serialization() {
        let response = DeletedResponse { deleted: true };
        let json = serde_json::to_value(&response).unwrap();
        assert_eq!(json["deleted"], true);
    }

    #[test]
    fn test_created_api_key_response_serialization() {
        let response = CreatedApiKeyResponse {
            id: "key1".to_string(),
            user_id: "user1".to_string(),
            key_prefix: "sk-prx-abcd1234...".to_string(),
            label: "test".to_string(),
            created_at: "2025-01-01 00:00:00".to_string(),
            plaintext: "sk-prx-full-key-here".to_string(),
        };

        let json = serde_json::to_value(&response).unwrap();
        assert_eq!(json["plaintext"], "sk-prx-full-key-here");
        assert_eq!(json["key_prefix"], "sk-prx-abcd1234...");
    }

    #[test]
    fn test_update_setting_request_deserialization() {
        let json = r#"{"key": "server.host", "value": "0.0.0.0"}"#;
        let req: UpdateSettingRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.key, "server.host");
        assert_eq!(req.value, serde_json::json!("0.0.0.0"));
    }

    #[test]
    fn test_update_setting_request_bool_value() {
        let json = r#"{"key": "auth.enabled", "value": false}"#;
        let req: UpdateSettingRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.key, "auth.enabled");
        assert_eq!(req.value, serde_json::json!(false));
    }

    #[test]
    fn test_update_setting_request_number_value() {
        let json = r#"{"key": "server.port", "value": 9090}"#;
        let req: UpdateSettingRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.key, "server.port");
        assert_eq!(req.value, serde_json::json!(9090));
    }
}
