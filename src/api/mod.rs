pub mod admin;
pub mod chat;
pub mod embeddings;
pub mod health;
pub mod models;

use axum::Router;
use axum::routing::{delete, get, post, put};

use crate::AppState;

/// Build the full API router with all endpoint groups.
///
/// Route layout:
/// ```text
/// /health                        GET    (no auth)
/// /v1/chat/completions           POST   (auth required)
/// /v1/models                     GET    (auth required)
/// /v1/embeddings                 POST   (auth required)
/// /admin/users                   POST   (admin)
/// /admin/users                   GET    (admin)
/// /admin/users/:id               DELETE (admin)
/// /admin/users/:id/keys          POST   (admin)
/// /admin/users/:id/keys          GET    (admin)
/// /admin/keys/:id                DELETE (admin)
/// /admin/budgets/:user_id        PUT    (admin)
/// /admin/budgets/:user_id        GET    (admin)
/// /admin/usage                   GET    (admin)
/// /admin/settings                GET    (admin)
/// /admin/settings                PUT    (admin)
/// /admin/cache/stats             GET    (admin)
/// /admin/cache                   DELETE (admin)
/// /admin/cache/:model            DELETE (admin)
/// ```
pub fn build_api_router() -> Router<AppState> {
    let admin_routes = Router::new()
        .route("/users", post(admin::create_user))
        .route("/users", get(admin::list_users))
        .route("/users/{id}", delete(admin::delete_user))
        .route("/users/{id}/keys", post(admin::create_api_key))
        .route("/users/{id}/keys", get(admin::list_api_keys))
        .route("/keys/{id}", delete(admin::revoke_api_key))
        .route("/budgets/{user_id}", put(admin::set_budget))
        .route("/budgets/{user_id}", get(admin::get_budget))
        .route("/usage", get(admin::query_usage))
        .route("/settings", get(admin::get_settings))
        .route("/settings", put(admin::update_settings))
        .route("/cache/stats", get(admin::cache_stats))
        .route("/cache", delete(admin::flush_cache))
        .route("/cache/{model}", delete(admin::flush_cache_model));

    Router::new()
        .route("/health", get(health::health_check))
        .route("/v1/chat/completions", post(chat::chat_completions))
        .route("/v1/models", get(models::list_models))
        .route("/v1/embeddings", post(embeddings::create_embedding))
        .nest("/admin", admin_routes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_api_router_creates_router() {
        // Smoke test: ensure the router builds without panicking.
        let _router: Router<AppState> = build_api_router();
    }
}
