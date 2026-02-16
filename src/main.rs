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
use std::time::Duration;

use axum::Router;
use axum::middleware;
use tokio::net::TcpListener;
use tokio::signal;
use tokio::sync::RwLock;
use tower_http::cors::{Any, CorsLayer};
use tower_http::request_id::{MakeRequestUuid, PropagateRequestIdLayer, SetRequestIdLayer};
use tower_http::trace::TraceLayer;
use tracing_subscriber::EnvFilter;

use gaud::AppState;
use gaud::api;
use gaud::auth::middleware::require_auth;
use gaud::auth::users::bootstrap_admin;
use gaud::budget::{BudgetTracker, spawn_audit_logger};
use gaud::cache::SemanticCacheService;
use gaud::config::{Config, KiroProviderConfig, LitellmProviderConfig};
use gaud::db::Database;
use gaud::oauth::OAuthManager;
use gaud::providers::LlmProvider;
use gaud::providers::kiro::KiroProvider;
use gaud::providers::litellm::{LitellmConfig, LitellmProvider};
use gaud::providers::router::ProviderRouter;
use gaud::web;

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

fn main() -> anyhow::Result<()> {
    tokio::runtime::Builder::new_multi_thread()
        .thread_stack_size(10 * 1024 * 1024) // 10 MiB per worker thread
        .enable_all()
        .build()
        .expect("Failed to build Tokio runtime")
        .block_on(async_main())
}

async fn async_main() -> anyhow::Result<()> {
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

    // 5b. Create OAuth manager (needed for provider registration)
    let config_arc = Arc::new(config.clone());
    let oauth_manager = Arc::new(OAuthManager::from_config(config_arc.clone(), db.clone()));
    tracing::debug!("OAuth manager initialized");

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

    // Register LiteLLM provider if configured.
    if let Some(ref litellm_config) = config.providers.litellm {
        match build_litellm_provider(litellm_config).await {
            Ok(provider) => {
                let model_count = provider.models().len();
                provider_router.register(Arc::new(provider));
                tracing::info!(models = model_count, "LiteLLM provider registered");
            }
            Err(e) => {
                tracing::warn!(error = %e, "Failed to initialize LiteLLM provider, skipping");
            }
        }
    }

    // Register Gemini provider if configured
    if config.providers.gemini.is_some() {
        let gemini = gaud::providers::gemini::provider::GeminiProvider::new(oauth_manager.clone());
        provider_router.register(Arc::new(gemini));
        tracing::info!("Gemini provider registered");
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

    // 9. Initialize semantic cache (if enabled)
    let cache = if config.cache.enabled {
        match SemanticCacheService::new(&config.cache).await {
            Ok(c) => {
                let c = Arc::new(c);
                // Spawn TTL eviction every 5 minutes.
                let c2 = Arc::clone(&c);
                let ttl = config.cache.ttl_secs;
                tokio::spawn(async move {
                    let mut interval = tokio::time::interval(Duration::from_secs(300));
                    loop {
                        interval.tick().await;
                        match c2.evict_expired(ttl).await {
                            Ok(n) if n > 0 => {
                                tracing::debug!(evicted = n, "Cache TTL eviction");
                            }
                            Err(e) => {
                                tracing::warn!(error = %e, "Cache eviction failed");
                            }
                            _ => {}
                        }
                    }
                });
                tracing::info!(
                    mode = %config.cache.mode,
                    ttl_secs = config.cache.ttl_secs,
                    max_entries = config.cache.max_entries,
                    "Semantic cache initialized"
                );
                Some(c)
            }
            Err(e) => {
                tracing::warn!(error = %e, "Cache init failed, running without cache");
                None
            }
        }
    } else {
        None
    };

    // 10. Auth-disabled warning
    if !config.auth.enabled {
        tracing::warn!("Authentication is DISABLED -- all requests treated as admin");
    }

    // (OAuth manager already initialized at step 5b)

    // 11. Build shared application state
    let cost_calculator = Arc::new(gaud::providers::cost::CostCalculator::new());
    let state = AppState {
        config: config_arc,
        config_path: config_path.clone(),
        db: db.clone(),
        router: provider_router,
        budget,
        audit_tx,
        cost_calculator,
        cache,
        oauth_manager,
    };

    // 12. Build the combined router
    let app = build_app(state.clone());

    // 13. Bind and serve
    let listen_addr = config.listen_addr();
    let listener = TcpListener::bind(&listen_addr).await?;
    tracing::info!(addr = %listen_addr, "Listening");

    println!();
    println!("  gaud v{} is running", env!("CARGO_PKG_VERSION"));
    println!("  API:       http://{listen_addr}/v1/");
    println!("  Dashboard: http://{listen_addr}/ui/dashboard");
    println!("  Health:    http://{listen_addr}/health");
    println!();

    // 14. Serve with graceful shutdown
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;

    // 15. Cleanup
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
/// Constructs a `KiroClient` using the refresh-token auth flow that matches
/// the kiro-aws reference implementation.
async fn build_kiro_provider(kiro_config: &KiroProviderConfig) -> anyhow::Result<KiroProvider> {
    use gaud::providers::kiro::{
        AutoDetectProvider, KiroAuthManager, KiroClient, KiroProvider, machine_fingerprint,
    };

    let region = kiro_config.effective_region();
    let fingerprint = machine_fingerprint();

    let manager = Arc::new(KiroAuthManager::new(fingerprint.clone(), region.clone()));

    let auth = Arc::new(
        AutoDetectProvider::new(
            manager,
            kiro_config.credentials_file.as_ref().map(PathBuf::from),
            kiro_config.kiro_db_path.as_ref().map(PathBuf::from),
            kiro_config.sso_cache_dir.as_ref().map(PathBuf::from),
        )
        .await,
    );

    let client = KiroClient::new(
        auth,
        region,
        kiro_config.effective_profile_arn(),
        fingerprint,
    );
    Ok(KiroProvider::new(client))
}

// ---------------------------------------------------------------------------
// LiteLLM provider builder
// ---------------------------------------------------------------------------

/// Build a [`LitellmProvider`] from the configuration.
///
/// The provider connects to the LiteLLM proxy at the configured URL and
/// optionally discovers available models from its `/v1/models` endpoint.
async fn build_litellm_provider(config: &LitellmProviderConfig) -> anyhow::Result<LitellmProvider> {
    let litellm_config = LitellmConfig {
        url: config.url.clone(),
        api_key: config.api_key.clone(),
        discover_models: config.discover_models,
        models: config.models.clone(),
        timeout_secs: config.timeout_secs,
    };

    let provider = LitellmProvider::new(litellm_config)
        .await
        .map_err(|e| anyhow::anyhow!("LiteLLM provider init failed: {e}"))?;

    Ok(provider)
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
    let api_routes =
        api::build_api_router().layer(middleware::from_fn_with_state(state.clone(), require_auth));

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
