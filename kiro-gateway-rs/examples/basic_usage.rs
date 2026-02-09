//! Basic usage example: send a message and print the response.

use kiro_gateway::{KiroClientBuilder, Result};

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter("kiro_gateway=info")
        .init();

    // Build client - tries default credential sources
    let client = KiroClientBuilder::new()
        .credentials_file("~/.kiro/credentials.json")
        .build()
        .await?;

    // Send a message
    let response = client
        .messages()
        .model("claude-sonnet-4.5")
        .max_tokens(1024)
        .system("You are a helpful assistant.")
        .user_message("What is the capital of Iceland?")
        .send()
        .await?;

    println!("Response: {}", response.text());
    println!(
        "Usage: {} input, {} output tokens",
        response.usage.input_tokens, response.usage.output_tokens
    );

    Ok(())
}
