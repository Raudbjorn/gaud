//! Raw Kiro API example: send payloads in Kiro's native format.

use kiro_gateway::{KiroClientBuilder, Result};
use serde_json::json;
use uuid::Uuid;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter("kiro_gateway=info")
        .init();

    let client = KiroClientBuilder::new()
        .credentials_file("~/.kiro/credentials.json")
        .build()
        .await?;

    // Build a raw Kiro payload
    let payload = json!({
        "conversationState": {
            "chatTriggerType": "MANUAL",
            "conversationId": Uuid::new_v4().to_string(),
            "currentMessage": {
                "userInputMessage": {
                    "content": "Hello from the raw Kiro API!",
                    "modelId": "claude-sonnet-4.5",
                    "origin": "AI_EDITOR"
                }
            }
        }
    });

    let response = client.raw_request(&payload).await?;
    println!("Raw response:\n{}", &response[..response.len().min(500)]);

    Ok(())
}
