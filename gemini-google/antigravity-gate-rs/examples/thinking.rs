//! Thinking (extended reasoning) example for antigravity-gate.
//!
//! This example demonstrates using thinking models that show their
//! reasoning process before providing a response. Thinking models
//! produce thinking blocks that contain the model's internal reasoning.
//!
//! # Running
//!
//! ```bash
//! cargo run --example thinking
//! ```
//!
//! # Prerequisites
//!
//! Run the `basic_usage` or `auth_flow` example first to authenticate.
//!
//! # Supported Models
//!
//! Thinking is available on models with `-thinking` suffix:
//! - `claude-sonnet-4-5-thinking`
//! - `claude-opus-4-5-thinking`

use std::io::Write;
use std::sync::Arc;

use futures::StreamExt;

use antigravity_gate::{
    CloudCodeClient, ContentBlock, ContentDelta, FileTokenStorage, Result, StreamEvent,
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

    println!("=== Antigravity Gate: Thinking Example ===\n");

    // Create client
    let storage = FileTokenStorage::default_path()?;
    let client = Arc::new(CloudCodeClient::builder().with_storage(storage).build());

    // Verify authentication
    if !client.is_authenticated().await? {
        eprintln!("Not authenticated. Please run the basic_usage example first.");
        std::process::exit(1);
    }

    // A problem that benefits from step-by-step reasoning
    let problem = r#"
A farmer has a wolf, a goat, and a cabbage. He needs to cross a river with all three,
but his boat can only carry him and one item at a time. If left alone:
- The wolf will eat the goat
- The goat will eat the cabbage

What sequence of crossings should the farmer make to get everything across safely?
    "#
    .trim();

    println!("Problem:\n{}\n", problem);
    println!("---");
    println!("Thinking (streaming):\n");

    // Use streaming to see thinking as it happens
    let mut stream = client
        .messages()
        .model("claude-sonnet-4-5-thinking") // Use a thinking model
        .max_tokens(16000) // Thinking models need more tokens
        .system("You are a logical problem solver. Think through problems step by step.")
        .user_message(problem)
        .thinking_budget(10000) // Budget for thinking tokens
        .send_stream()
        .await?;

    let mut thinking_text = String::new();
    let mut response_text = String::new();
    let mut in_thinking = false;

    while let Some(event) = stream.next().await {
        let event = event?;

        match event {
            StreamEvent::ContentBlockStart { content_block, .. } => {
                match &content_block {
                    ContentBlock::Thinking { .. } => {
                        in_thinking = true;
                        print!("\x1b[90m"); // Gray color for thinking
                    }
                    ContentBlock::Text { .. } => {
                        if in_thinking {
                            print!("\x1b[0m"); // Reset color
                            println!("\n---\nResponse:\n");
                            in_thinking = false;
                        }
                    }
                    _ => {}
                }
            }

            StreamEvent::ContentBlockDelta { delta, .. } => {
                match delta {
                    ContentDelta::ThinkingDelta { thinking } => {
                        print!("{}", thinking);
                        std::io::stdout().flush()?;
                        thinking_text.push_str(&thinking);
                    }
                    ContentDelta::TextDelta { text } => {
                        print!("{}", text);
                        std::io::stdout().flush()?;
                        response_text.push_str(&text);
                    }
                    ContentDelta::SignatureDelta { signature: _ } => {
                        // Signature is used for verification, not displayed
                    }
                    _ => {}
                }
            }

            StreamEvent::ContentBlockStop { .. } => {
                if in_thinking {
                    println!();
                }
            }

            StreamEvent::MessageDelta { usage, .. } => {
                print!("\x1b[0m"); // Reset color
                if let Some(usage) = usage {
                    println!("\n---");
                    println!("\n=== Usage ===");
                    println!("Input tokens:   {}", usage.input_tokens);
                    println!("Output tokens:  {}", usage.output_tokens);
                }
            }

            StreamEvent::Error { error } => {
                print!("\x1b[0m"); // Reset color
                eprintln!("\nStream error: {}", error.message);
                return Err(antigravity_gate::Error::api(500, error.message, None));
            }

            _ => {}
        }
    }

    print!("\x1b[0m"); // Ensure color is reset

    println!("\n=== Summary ===");
    println!("Thinking length: {} characters", thinking_text.len());
    println!("Response length: {} characters", response_text.len());

    Ok(())
}
