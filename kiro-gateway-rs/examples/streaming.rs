//! Streaming example: stream response tokens as they arrive.

use futures::StreamExt;
use kiro_gateway::{ContentDelta, KiroClientBuilder, Result, StreamEvent};

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter("kiro_gateway=info")
        .init();

    let client = KiroClientBuilder::new()
        .credentials_file("~/.kiro/credentials.json")
        .build()
        .await?;

    let mut stream = client
        .messages()
        .model("claude-sonnet-4.5")
        .max_tokens(2048)
        .user_message("Write a short poem about Rust programming.")
        .send_stream()
        .await?;

    while let Some(event) = stream.next().await {
        match event? {
            StreamEvent::ContentBlockDelta {
                delta: ContentDelta::TextDelta { text },
                ..
            } => {
                print!("{}", text);
            }
            StreamEvent::MessageDelta { delta, usage, .. } => {
                if let Some(reason) = delta.stop_reason {
                    println!("\n\nStop reason: {:?}", reason);
                }
                if let Some(usage) = usage {
                    println!(
                        "Usage: {} input, {} output tokens",
                        usage.input_tokens, usage.output_tokens
                    );
                }
            }
            _ => {}
        }
    }

    Ok(())
}
