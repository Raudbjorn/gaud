//! Gaud -- Multi-user LLM proxy with OpenAI-compatible API.
//!
//! This is the application entry point. It wires together all modules:
//!   - Configuration loading
//!   - Database initialization
//!   - Admin user bootstrapping
//!   - Provider router creation
//!   - Budget tracker + audit logger
//!   - Combined HTTP server (API + Web UI)
//!   - Graceful shutdown on SIGTERM / SIGINT

use std::path::PathBuf;
use std::sync::Arc;

use axum::middleware;
use axum::Router;
use tokio::net::TcpListener;
use tokio::signal;
use tokio::sync::RwLock;
use tower_http::cors::{Any, CorsLayer};
use tower_http::request_id::{MakeRequestUuid, PropagateRequestIdLayer, SetRequestIdLayer};
use tower_http::trace::TraceLayer;
use tracing_subscriber::EnvFilter;

use gaud::api;
use gaud::auth::middleware::require_auth;
use gaud::auth::users::bootstrap_admin;
use gaud::budget::{spawn_audit_logger, BudgetTracker};
use gaud::config::{Config, KiroProviderConfig};
use gaud::db::Database;
use gaud::providers::kiro::KiroProvider;
use gaud::providers::router::ProviderRouter;
use gaud::web;
use gaud::AppState;

// ---------------------------------------------------------------------------
// CLI argument parsing (minimal, no clap dependency)
// ---------------------------------------------------------------------------

struct CliArgs {
    config_path: PathBuf,
}

fn parse_args() -> CliArgs {
    let mut args = std::env::args().skip(1);
    let mut config_path = PathBuf::from("llm-proxy.toml");

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--config" | "-c" => {
                if let Some(path) = args.next() {
                    config_path = PathBuf::from(path);
                } else {
                    eprintln!("Error: --config requires a path argument");
                    std::process::exit(1);
                }
            }
            "--help" | "-h" => {
                print_usage();
                std::process::exit(0);
            }
            "--version" | "-V" => {
                println!("gaud {}", env!("CARGO_PKG_VERSION"));
                std::process::exit(0);
            }
            other => {
                eprintln!("Unknown argument: {other}");
                eprintln!("Run with --help for usage information.");
                std::process::exit(1);
            }
        }
    }

    CliArgs { config_path }
}

fn print_usage() {
    println!(
        "\
gaud {version} -- Multi-user LLM proxy

USAGE:
    gaud [OPTIONS]

OPTIONS:
    -c, --config <PATH>    Path to configuration file [default: llm-proxy.toml]
    -h, --help             Print this help message
    -V, --version          Print version information

ENVIRONMENT:
    RUST_LOG               Override log level (e.g. RUST_LOG=debug)
    GAUD_CONFIG            Alternative to --config flag
",
        version = env!("CARGO_PKG_VERSION")
    );
}

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // 1. Parse CLI arguments
    let cli = parse_args();

    // Allow GAUD_CONFIG env var as alternative to --config flag
    let config_path = std::env::var("GAUD_CONFIG")
        .map(PathBuf::from)
        .unwrap_or(cli.config_path);

    // 2. Load configuration
    let config = Config::load(&config_path)?;

    // 3. Initialize tracing/logging
    init_tracing(&config);

    tracing::info!(
        version = env!("CARGO_PKG_VERSION"),
        config = %config_path.display(),
        "Starting gaud"
    );

    // 4. Open database
    let db = Database::open(&config.database.path)?;
    tracing::info!(path = %config.database.path.display(), "Database opened");

    // 5. Bootstrap admin user (creates admin + prints API key on first run)
    match bootstrap_admin(&db, &config.auth.default_admin_name) {
        Ok(Some(result)) => {
            tracing::info!(
                admin = %result.user.name,
                "Admin user bootstrapped (first run)"
            );
        }
        Ok(None) => {
            tracing::debug!("Admin bootstrap skipped (users already exist)");
        }
        Err(err) => {
            tracing::error!(error = %err, "Failed to bootstrap admin user");
            return Err(err.into());
        }
    }

    // 6. Create provider router
    //
    //    The ProviderRouter needs concrete LlmProvider instances. Since the
    //    provider implementations (claude, gemini, copilot) require token
    //    storage and are constructed asynchronously, we create an empty router
    //    here and let it be populated once the OAuth/token infrastructure is
    //    ready. The router is behind an Arc<RwLock<>> so it can be updated.
    let mut provider_router = ProviderRouter::new();

    // Register Kiro provider if configured.
    if let Some(ref kiro_config) = config.providers.kiro {
        match build_kiro_provider(kiro_config).await {
            Ok(provider) => {
                provider_router.register(Arc::new(provider));
                tracing::info!("Kiro provider registered");
            }
            Err(e) => {
                tracing::warn!(error = %e, "Failed to initialize Kiro provider, skipping");
            }
        }
    }

    let provider_router = Arc::new(RwLock::new(provider_router));

    // 7. Create budget tracker
    let budget = Arc::new(BudgetTracker::new(db.clone()));

    // 8. Create audit channel + spawn background logger
    let (audit_tx, audit_rx) = tokio::sync::mpsc::unbounded_channel();
    let audit_db = db.clone();
    let audit_budget = budget.clone();
    let _audit_handle = spawn_audit_logger(audit_db, audit_budget, audit_rx);
    tracing::debug!("Audit logger spawned");

    // 9. Auth-disabled warning
    if !config.auth.enabled {
        tracing::warn!("Authentication is DISABLED -- all requests treated as admin");
    }

    // 10. Build shared application state
    let state = AppState {
        config: Arc::new(config.clone()),
        config_path: config_path.clone(),
        db: db.clone(),
        router: provider_router,
        budget,
        audit_tx,
    };

    // 11. Build the combined router
    let app = build_app(state.clone());

    // 12. Bind and serve
    let listen_addr = config.listen_addr();
    let listener = TcpListener::bind(&listen_addr).await?;
    tracing::info!(addr = %listen_addr, "Listening");

    println!();
    println!("  gaud v{} is running", env!("CARGO_PKG_VERSION"));
    println!("  API:       http://{listen_addr}/v1/");
    println!("  Dashboard: http://{listen_addr}/ui/dashboard");
    println!("  Health:    http://{listen_addr}/health");
    println!();

    // 13. Serve with graceful shutdown
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;

    // 14. Cleanup
    tracing::info!("Shutting down gracefully");
    // Database is dropped automatically via Arc/Mutex.
    // The audit_tx sender is dropped here, which will cause the audit logger
    // to drain remaining entries and exit.

    Ok(())
}

// ---------------------------------------------------------------------------
// Kiro provider builder
// ---------------------------------------------------------------------------

/// Build a [`KiroProvider`] from the configuration.
///
/// The kiro-gateway client is constructed via its builder, picking up
/// credentials from the config (refresh token, credentials file, or env vars).
async fn build_kiro_provider(kiro_config: &KiroProviderConfig) -> anyhow::Result<KiroProvider> {
    let mut builder = kiro_gateway::KiroClientBuilder::new();

    if let Some(ref token) = kiro_config.refresh_token {
        builder = builder.refresh_token(token.clone());
    }

    if let Some(ref path) = kiro_config.credentials_file {
        builder = builder.credentials_file(path);
    }

    if let Some(ref region) = kiro_config.region {
        builder = builder.region(region.clone());
    }

    let client = builder.build().await?;
    Ok(KiroProvider::new(client))
}

// ---------------------------------------------------------------------------
// Router assembly
// ---------------------------------------------------------------------------

/// Build the combined application router with all middleware layers.
fn build_app(state: AppState) -> Router {
    let config = &state.config;

    // -- CORS layer -----------------------------------------------------------
    let cors = build_cors_layer(config);

    // -- Request ID layer (X-Request-ID) --------------------------------------
    let request_id = SetRequestIdLayer::x_request_id(MakeRequestUuid);
    let propagate_id = PropagateRequestIdLayer::x_request_id();

    // -- Tracing layer --------------------------------------------------------
    let trace = TraceLayer::new_for_http();

    // -- API routes (require auth) --------------------------------------------
    let api_routes = api::build_api_router()
        .layer(middleware::from_fn_with_state(state.clone(), require_auth));

    // -- Web UI routes (no API auth middleware) --------------------------------
    let web_routes = web::build_web_router();

    // -- Combine all routes ---------------------------------------------------
    Router::new()
        .merge(web_routes)
        .merge(api_routes)
        // Global middleware stack (applied to all routes)
        .layer(propagate_id)
        .layer(request_id)
        .layer(trace)
        .layer(cors)
        .with_state(state)
}

/// Build the CORS layer from config.
fn build_cors_layer(config: &Config) -> CorsLayer {
    if config.server.cors_origins.is_empty() {
        // Default: allow all origins for development convenience
        CorsLayer::new()
            .allow_origin(Any)
            .allow_methods(Any)
            .allow_headers(Any)
    } else {
        let origins: Vec<_> = config
            .server
            .cors_origins
            .iter()
            .filter_map(|o| o.parse().ok())
            .collect();

        CorsLayer::new()
            .allow_origin(origins)
            .allow_methods(Any)
            .allow_headers(Any)
    }
}

// ---------------------------------------------------------------------------
// Tracing initialization
// ---------------------------------------------------------------------------

/// Set up the tracing subscriber based on configuration.
fn init_tracing(config: &Config) {
    // RUST_LOG env var takes precedence over config file
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| {
        let level = &config.logging.level;
        // Set gaud crate to the configured level, dependencies to warn
        EnvFilter::new(format!("gaud={level},tower_http={level},warn"))
    });

    let subscriber = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(true)
        .with_thread_ids(false)
        .with_file(false)
        .with_line_number(false);

    if config.logging.json {
        subscriber.json().init();
    } else {
        subscriber.init();
    }
}

// ---------------------------------------------------------------------------
// Graceful shutdown
// ---------------------------------------------------------------------------

/// Wait for a shutdown signal (SIGTERM or SIGINT / Ctrl+C).
async fn shutdown_signal() {
    let ctrl_c = async {
        signal::ctrl_c()
            .await
            .expect("Failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        signal::unix::signal(signal::unix::SignalKind::terminate())
            .expect("Failed to install SIGTERM handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        () = ctrl_c => {
            tracing::info!("Received SIGINT (Ctrl+C)");
        }
        () = terminate => {
            tracing::info!("Received SIGTERM");
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_print_usage_does_not_panic() {
        // Just verify it doesn't panic.
        print_usage();
    }

    #[test]
    fn test_build_cors_layer_empty_origins() {
        let config = Config::default();
        let _cors = build_cors_layer(&config);
        // No panic means success.
    }

    #[test]
    fn test_build_cors_layer_with_origins() {
        let mut config = Config::default();
        config.server.cors_origins = vec!["http://localhost:3000".to_string()];
        let _cors = build_cors_layer(&config);
    }
}
