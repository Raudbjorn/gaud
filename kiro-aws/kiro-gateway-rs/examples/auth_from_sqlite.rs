//! Load credentials from kiro-cli's SQLite database.
//!
//! Requires the `sqlite` feature: `cargo run --features sqlite --example auth_from_sqlite`

use kiro_gateway::{KiroClientBuilder, Result};

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter("kiro_gateway=debug")
        .init();

    let client = KiroClientBuilder::new()
        .sqlite_db("~/.local/share/kiro-cli/data.sqlite3")
        .build()
        .await?;

    // List available models
    let models = client.list_models().await?;
    println!("Available models:");
    for model in &models {
        println!("  - {}", model);
    }

    // Send a test message
    let response = client
        .messages()
        .model("auto")
        .max_tokens(256)
        .user_message("Say 'hello' in 3 languages.")
        .send()
        .await?;

    println!("\nResponse: {}", response.text());

    Ok(())
}
