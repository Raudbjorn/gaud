//! Basic usage example for antigravity-gate.
//!
//! This example demonstrates sending a simple message to Claude via the
//! Cloud Code API and receiving a response.
//!
//! # Running
//!
//! ```bash
//! cargo run --example basic_usage
//! ```
//!
//! The example will check for existing authentication and prompt for
//! OAuth if needed.

use std::sync::Arc;

use antigravity_gate::{CloudCodeClient, FileTokenStorage, Result};

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize tracing for debug output
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive(tracing::Level::INFO.into()),
        )
        .init();

    println!("=== Antigravity Gate: Basic Usage Example ===\n");

    // Create file-based token storage at the default location
    // ~/.config/antigravity-gate/token.json
    let storage = FileTokenStorage::default_path()?;
    println!("Using token storage at: {}", storage.path().display());

    // Build the client with the storage backend
    let client = Arc::new(CloudCodeClient::builder().with_storage(storage).build());

    // Check if we're already authenticated
    if !client.is_authenticated().await? {
        println!("\nNot authenticated. Starting OAuth flow...\n");

        // Start the OAuth authorization flow
        let (auth_url, state) = client.start_oauth_flow().await?;

        println!("Please open the following URL in your browser:");
        println!("\n  {}\n", auth_url);
        println!("After authorizing, you'll receive an authorization code.");
        println!("State parameter for verification: {}", state.state);

        // In a real application, you would:
        // 1. Start a local HTTP server to receive the callback
        // 2. Or prompt the user to paste the code from the callback URL
        //
        // For this example, we'll prompt for manual input:
        print!("Enter the authorization code: ");
        std::io::Write::flush(&mut std::io::stdout())?;

        let mut code = String::new();
        std::io::stdin().read_line(&mut code)?;
        let code = code.trim();

        // Exchange the authorization code for tokens
        let _token = client.complete_oauth_flow(code, Some(&state.state)).await?;

        println!("\nAuthentication successful!\n");
    } else {
        println!("Already authenticated.\n");
    }

    // Now we can make API requests
    println!("Sending request to Claude...\n");

    // Use the fluent builder API to construct and send a request
    let response = client
        .messages()
        .model("claude-sonnet-4-5")
        .max_tokens(1024)
        .system("You are a helpful assistant. Keep responses concise.")
        .user_message("What is the Rust programming language? Explain in 2-3 sentences.")
        .send()
        .await?;

    // Print the response
    println!("=== Response ===\n");
    println!("{}\n", response.text());

    // Print usage information
    println!("=== Usage ===");
    println!("Input tokens:  {}", response.usage.input_tokens);
    println!("Output tokens: {}", response.usage.output_tokens);
    if let Some(cached) = response.usage.cache_read_input_tokens {
        println!("Cache hit:     {} tokens", cached);
    }
    println!("Stop reason:   {:?}", response.stop_reason);

    Ok(())
}
