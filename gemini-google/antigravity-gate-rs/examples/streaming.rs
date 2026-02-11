//! Streaming example for antigravity-gate.
//!
//! This example demonstrates streaming responses from Claude, receiving
//! content incrementally as it's generated rather than waiting for the
//! full response.
//!
//! # Running
//!
//! ```bash
//! cargo run --example streaming
//! ```
//!
//! # Prerequisites
//!
//! Run the `basic_usage` or `auth_flow` example first to authenticate.

use std::io::Write;
use std::sync::Arc;

use futures::StreamExt;

use antigravity_gate::{CloudCodeClient, ContentDelta, FileTokenStorage, Result, StreamEvent};

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize tracing
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive(tracing::Level::INFO.into()),
        )
        .init();

    println!("=== Antigravity Gate: Streaming Example ===\n");

    // Create client with file-based storage
    let storage = FileTokenStorage::default_path()?;
    let client = Arc::new(CloudCodeClient::builder().with_storage(storage).build());

    // Verify authentication
    if !client.is_authenticated().await? {
        eprintln!("Not authenticated. Please run the basic_usage example first.");
        std::process::exit(1);
    }

    println!("Streaming response from Claude...\n");
    println!("---");

    // Build and send a streaming request
    let mut stream = client
        .messages()
        .model("claude-sonnet-4-5")
        .max_tokens(1024)
        .system("You are a storyteller. Write engaging, creative content.")
        .user_message("Write a very short (3-4 sentence) story about a robot learning to paint.")
        .send_stream()
        .await?;

    // Process stream events as they arrive
    let mut total_text = String::new();

    while let Some(event) = stream.next().await {
        let event = event?;

        match event {
            StreamEvent::MessageStart { message } => {
                // Message started, we have the message ID and model
                tracing::debug!(
                    message_id = %message.id,
                    model = %message.model,
                    "Stream started"
                );
            }

            StreamEvent::ContentBlockStart {
                index,
                content_block,
            } => {
                // A new content block is starting
                let block_type = if content_block.is_text() {
                    "text"
                } else if content_block.is_thinking() {
                    "thinking"
                } else if content_block.is_tool_use() {
                    "tool_use"
                } else {
                    "other"
                };
                tracing::debug!(index, content_type = block_type, "Block started");
            }

            StreamEvent::ContentBlockDelta { delta, .. } => {
                // Incremental content update
                match delta {
                    ContentDelta::TextDelta { text } => {
                        // Print text immediately as it arrives
                        print!("{}", text);
                        std::io::stdout().flush()?;
                        total_text.push_str(&text);
                    }
                    ContentDelta::ThinkingDelta { thinking } => {
                        // For thinking models, you'd see thinking deltas
                        tracing::debug!(thinking = %thinking, "Thinking");
                    }
                    ContentDelta::SignatureDelta { signature } => {
                        tracing::debug!(signature = %signature, "Signature received");
                    }
                    ContentDelta::InputJsonDelta { partial_json } => {
                        tracing::debug!(json = %partial_json, "Tool input");
                    }
                }
            }

            StreamEvent::ContentBlockStop { index } => {
                tracing::debug!(index, "Block finished");
            }

            StreamEvent::MessageDelta { delta, usage } => {
                // Final message metadata
                if let Some(stop_reason) = delta.stop_reason {
                    tracing::debug!(stop_reason = ?stop_reason, "Message complete");
                }
                if let Some(usage) = usage {
                    println!("\n---");
                    println!("\n=== Usage ===");
                    println!("Input tokens:  {}", usage.input_tokens);
                    println!("Output tokens: {}", usage.output_tokens);
                }
            }

            StreamEvent::MessageStop => {
                tracing::debug!("Stream ended");
            }

            StreamEvent::Ping => {
                // Keep-alive, ignore
            }

            StreamEvent::Error { error } => {
                eprintln!("\nStream error: {} - {}", error.error_type, error.message);
                return Err(antigravity_gate::Error::api(500, error.message, None));
            }
        }
    }

    println!("\n\n=== Full Response ===");
    println!("{}", total_text);
    println!("\nTotal characters: {}", total_text.len());

    Ok(())
}
