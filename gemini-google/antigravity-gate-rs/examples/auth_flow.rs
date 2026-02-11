//! OAuth authentication flow example for antigravity-gate.
//!
//! This example demonstrates the complete OAuth authentication flow,
//! including:
//!
//! - Checking existing authentication
//! - Generating the authorization URL with PKCE
//! - Exchanging authorization codes for tokens
//! - Refreshing expired tokens
//! - Project discovery
//! - Logging out
//!
//! # Running
//!
//! ```bash
//! cargo run --example auth_flow
//! ```
//!
//! # Security
//!
//! This example uses file-based token storage at
//! `~/.config/antigravity-gate/token.json` with restricted permissions (0600).
//!
//! For production applications, consider using:
//! - System keyring (enable `keyring` feature)
//! - Custom secure storage via `CallbackStorage`

use std::io::Write;
use std::sync::Arc;

use antigravity_gate::{
    discover_project, CloudCodeClient, FileTokenStorage, OAuthFlow, Result, TokenStorage,
};

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize tracing
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive(tracing::Level::INFO.into()),
        )
        .init();

    println!("=== Antigravity Gate: OAuth Flow Example ===\n");

    // Create file-based token storage
    let storage = FileTokenStorage::default_path()?;
    println!("Token storage: {}", storage.path().display());

    // Create the OAuth flow handler
    let oauth = OAuthFlow::new(storage.clone());

    // Check current authentication status
    println!("\n--- Authentication Status ---\n");

    if oauth.is_authenticated().await? {
        println!("Status: Authenticated");

        // Load and display token info (without revealing secrets)
        if let Some(token) = storage.load().await? {
            let remaining = token.time_until_expiry();
            if remaining.as_secs() > 0 {
                println!("Access token expires in: {} seconds", remaining.as_secs());
            } else {
                println!("Access token: EXPIRED (will refresh on next request)");
            }

            // Parse project info from composite token
            let (_base, project_id, managed_id) = token.parse_refresh_parts();
            if let Some(project) = project_id {
                println!("Cached project ID: {}", project);
            }
            if let Some(managed) = managed_id {
                println!("Cached managed project ID: {}", managed);
            }
        }
    } else {
        println!("Status: Not authenticated");
    }

    // Main menu
    loop {
        println!("\n--- Menu ---\n");
        println!("1. Start new OAuth flow");
        println!("2. Refresh access token");
        println!("3. Discover project info");
        println!("4. Test API request");
        println!("5. Logout");
        println!("6. Exit");
        print!("\nChoice: ");
        std::io::stdout().flush()?;

        let mut choice = String::new();
        std::io::stdin().read_line(&mut choice)?;

        match choice.trim() {
            "1" => start_oauth_flow(&oauth).await?,
            "2" => refresh_token(&oauth).await?,
            "3" => discover_project_info(&oauth).await?,
            "4" => test_api_request(&storage).await?,
            "5" => logout(&oauth).await?,
            "6" => break,
            _ => println!("Invalid choice"),
        }
    }

    println!("\nGoodbye!");
    Ok(())
}

async fn start_oauth_flow<S: TokenStorage + 'static>(oauth: &OAuthFlow<S>) -> Result<()> {
    println!("\n--- Starting OAuth Flow ---\n");

    // Generate authorization URL with PKCE
    let (auth_url, state) = oauth.start_authorization_async().await?;

    println!("Authorization URL:\n");
    println!("  {}\n", auth_url);

    println!("This URL includes:");
    println!("  - Client ID: (Google Cloud Code)");
    println!(
        "  - PKCE code challenge: {}...",
        &state.code_challenge[..20]
    );
    println!("  - State parameter: {}", state.state);
    println!("  - Scopes: openid, email, https://www.googleapis.com/auth/cloud-platform");

    println!("\nOpen the URL in your browser to authenticate with Google.");
    println!("After authorizing, you'll be redirected to a localhost URL.");
    println!("Copy the 'code' parameter from the redirect URL.\n");

    print!("Enter the authorization code (or 'cancel'): ");
    std::io::stdout().flush()?;

    let mut code = String::new();
    std::io::stdin().read_line(&mut code)?;
    let code = code.trim();

    if code == "cancel" || code.is_empty() {
        println!("OAuth flow cancelled.");
        return Ok(());
    }

    // Exchange code for tokens
    println!("\nExchanging code for tokens...");
    let token = oauth.exchange_code(code, Some(&state.state)).await?;

    println!("\nAuthentication successful!");
    println!("Access token received (expires in {} seconds)", {
        let now = chrono::Utc::now().timestamp();
        (token.expires_at - now).max(0)
    });

    // Discover project info
    println!("\nDiscovering project info...");
    let access_token = oauth.get_access_token().await?;
    let project = discover_project(&access_token, None).await?;

    println!("Project ID: {}", project.project_id);
    println!("Subscription tier: {:?}", project.subscription_tier);

    Ok(())
}

async fn refresh_token<S: TokenStorage + 'static>(oauth: &OAuthFlow<S>) -> Result<()> {
    println!("\n--- Refreshing Token ---\n");

    if !oauth.is_authenticated().await? {
        println!("Not authenticated. Please complete OAuth flow first.");
        return Ok(());
    }

    // Force refresh by getting token (auto-refreshes if expired)
    println!("Requesting fresh access token...");
    let token = oauth.get_access_token().await?;

    println!("Access token refreshed!");
    println!("Token prefix: {}...", &token[..20.min(token.len())]);

    Ok(())
}

async fn discover_project_info<S: TokenStorage + 'static>(oauth: &OAuthFlow<S>) -> Result<()> {
    println!("\n--- Discovering Project Info ---\n");

    if !oauth.is_authenticated().await? {
        println!("Not authenticated. Please complete OAuth flow first.");
        return Ok(());
    }

    let access_token = oauth.get_access_token().await?;

    println!("Calling loadCodeAssist API...");
    let project = discover_project(&access_token, None).await?;

    println!("\nProject Information:");
    println!("  Project ID:          {}", project.project_id);
    println!("  Subscription Tier:   {:?}", project.subscription_tier);
    if let Some(managed) = &project.managed_project_id {
        println!("  Managed Project ID:  {}", managed);
    }

    Ok(())
}

async fn test_api_request<S: TokenStorage + Clone + 'static>(storage: &S) -> Result<()> {
    println!("\n--- Testing API Request ---\n");

    let client = Arc::new(
        CloudCodeClient::builder()
            .with_storage(storage.clone())
            .build(),
    );

    if !client.is_authenticated().await? {
        println!("Not authenticated. Please complete OAuth flow first.");
        return Ok(());
    }

    println!("Sending test message to Claude...\n");

    let response = client
        .messages()
        .model("claude-sonnet-4-5")
        .max_tokens(100)
        .user_message("Say 'Hello from Antigravity Gate!' and nothing else.")
        .send()
        .await?;

    println!("Response: {}\n", response.text());
    println!("Usage:");
    println!("  Input tokens:  {}", response.usage.input_tokens);
    println!("  Output tokens: {}", response.usage.output_tokens);

    Ok(())
}

async fn logout<S: TokenStorage + 'static>(oauth: &OAuthFlow<S>) -> Result<()> {
    println!("\n--- Logging Out ---\n");

    if !oauth.is_authenticated().await? {
        println!("Not currently authenticated.");
        return Ok(());
    }

    print!("Are you sure you want to logout? (y/N): ");
    std::io::stdout().flush()?;

    let mut confirm = String::new();
    std::io::stdin().read_line(&mut confirm)?;

    if confirm.trim().to_lowercase() == "y" {
        oauth.logout().await?;
        println!("Logged out successfully. Token removed from storage.");
    } else {
        println!("Logout cancelled.");
    }

    Ok(())
}
