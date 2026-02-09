use axum::extract::State;
use axum::http::Request;
use axum::middleware::Next;
use axum::response::Response;

use crate::auth::AuthUser;
use crate::budget::tracker::BudgetStatus;
use crate::error::AppError;
use crate::AppState;

/// Axum middleware that enforces per-user budget limits.
///
/// Must run **after** the auth middleware so that `AuthUser` is available in
/// request extensions.
///
/// Behavior:
/// - If budgeting is disabled in config, passes through immediately.
/// - If the user has no budget row, passes through (unlimited).
/// - If the budget is exceeded, returns 429 (BudgetExceeded).
/// - If the warning threshold is crossed, adds an `X-Budget-Warning` header
///   to the response but still allows the request.
pub async fn budget_middleware(
    State(state): State<AppState>,
    request: Request<axum::body::Body>,
    next: Next,
) -> Result<Response, AppError> {
    if !state.config.budget.enabled {
        return Ok(next.run(request).await);
    }

    let user = request
        .extensions()
        .get::<AuthUser>()
        .cloned();

    let user = match user {
        Some(u) => u,
        // No auth user present -- let downstream handlers deal with auth.
        None => return Ok(next.run(request).await),
    };

    let warning_threshold = state.config.budget.warning_threshold_percent;
    let status = state.budget.check_budget(&user.user_id, warning_threshold)?;

    match status {
        BudgetStatus::Exceeded => {
            tracing::warn!(
                user_id = %user.user_id,
                "Budget exceeded, rejecting request"
            );
            Err(AppError::BudgetExceeded(format!(
                "Budget exceeded for user '{}'",
                user.name
            )))
        }
        BudgetStatus::Warning(pct) => {
            tracing::info!(
                user_id = %user.user_id,
                usage_percent = pct,
                "Budget warning threshold crossed"
            );
            let mut response = next.run(request).await;
            response.headers_mut().insert(
                "X-Budget-Warning",
                format!("{pct:.1}% of budget used").parse().unwrap(),
            );
            Ok(response)
        }
        BudgetStatus::Ok => Ok(next.run(request).await),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_budget_status_variants() {
        let ok = BudgetStatus::Ok;
        let warning = BudgetStatus::Warning(85.0);
        let exceeded = BudgetStatus::Exceeded;

        assert_eq!(ok, BudgetStatus::Ok);
        assert!(matches!(warning, BudgetStatus::Warning(pct) if (pct - 85.0).abs() < f64::EPSILON));
        assert_eq!(exceeded, BudgetStatus::Exceeded);
    }
}
