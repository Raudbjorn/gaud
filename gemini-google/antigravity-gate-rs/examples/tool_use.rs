//! Tool use (function calling) example for antigravity-gate.
//!
//! This example demonstrates how to define tools and handle tool calls
//! from Claude. The model can request to call functions with structured
//! arguments, and you respond with tool results.
//!
//! # Running
//!
//! ```bash
//! cargo run --example tool_use
//! ```
//!
//! # Prerequisites
//!
//! Run the `basic_usage` or `auth_flow` example first to authenticate.

use std::sync::Arc;

use serde_json::json;

use antigravity_gate::{
    CloudCodeClient, ContentBlock, FileTokenStorage, Message, MessageContent, Result, Tool,
    ToolChoice,
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

    println!("=== Antigravity Gate: Tool Use Example ===\n");

    // Create client
    let storage = FileTokenStorage::default_path()?;
    let client = Arc::new(CloudCodeClient::builder().with_storage(storage).build());

    // Verify authentication
    if !client.is_authenticated().await? {
        eprintln!("Not authenticated. Please run the basic_usage example first.");
        std::process::exit(1);
    }

    // Define a weather tool with a JSON Schema for its input
    let weather_tool = Tool::new(
        "get_weather",
        "Get the current weather for a location. Returns temperature, conditions, and humidity.",
        json!({
            "type": "object",
            "properties": {
                "location": {
                    "type": "string",
                    "description": "The city and country, e.g., 'Tokyo, Japan' or 'London, UK'"
                },
                "units": {
                    "type": "string",
                    "enum": ["celsius", "fahrenheit"],
                    "description": "Temperature units to use",
                    "default": "celsius"
                }
            },
            "required": ["location"]
        }),
    );

    // Define a calculator tool
    let calculator_tool = Tool::new(
        "calculator",
        "Perform mathematical calculations. Supports basic arithmetic operations.",
        json!({
            "type": "object",
            "properties": {
                "expression": {
                    "type": "string",
                    "description": "Mathematical expression to evaluate, e.g., '2 + 2' or '15 * 3'"
                }
            },
            "required": ["expression"]
        }),
    );

    println!("Sending request with tools defined...\n");

    // First request: Ask a question that should trigger tool use
    let response = Arc::clone(&client)
        .messages()
        .model("claude-sonnet-4-5")
        .max_tokens(1024)
        .system("You are a helpful assistant with access to weather and calculator tools. Use them when appropriate.")
        .user_message("What's the weather like in Tokyo? Also, what's 42 * 7?")
        .tools(vec![weather_tool.clone(), calculator_tool.clone()])
        .tool_choice(ToolChoice::Auto)
        .send()
        .await?;

    println!("=== First Response ===");
    println!("Stop reason: {:?}\n", response.stop_reason);

    // Check if the model wants to use tools
    let tool_uses: Vec<_> = response
        .content
        .iter()
        .filter_map(|block| {
            if let ContentBlock::ToolUse { id, name, input } = block {
                Some((id.clone(), name.clone(), input.clone()))
            } else {
                None
            }
        })
        .collect();

    if tool_uses.is_empty() {
        println!("No tool calls requested. Response:");
        println!("{}", response.text());
        return Ok(());
    }

    println!("Tool calls requested:");
    for (id, name, input) in &tool_uses {
        println!("  - {} ({}): {}", name, id, input);
    }
    println!();

    // Simulate executing the tools and collecting results
    let mut tool_results = Vec::new();

    for (id, name, input) in &tool_uses {
        let result = match name.as_str() {
            "get_weather" => {
                // Simulate weather API response
                let location = input
                    .get("location")
                    .and_then(|v| v.as_str())
                    .unwrap_or("Unknown");
                json!({
                    "location": location,
                    "temperature": 22,
                    "units": "celsius",
                    "conditions": "Partly cloudy",
                    "humidity": 65
                })
                .to_string()
            }
            "calculator" => {
                // Simulate calculator
                let expression = input
                    .get("expression")
                    .and_then(|v| v.as_str())
                    .unwrap_or("0");
                // For demo, just return a hardcoded result for 42 * 7
                if expression.contains("42") && expression.contains("7") {
                    "294".to_string()
                } else {
                    "Result: (simulated)".to_string()
                }
            }
            _ => "Unknown tool".to_string(),
        };

        println!("Executed {}: {}", name, result);

        tool_results.push(ContentBlock::tool_result(id, result));
    }

    println!("\nSending tool results back to Claude...\n");

    // Build the conversation history with tool results
    // The conversation should include:
    // 1. Original user message
    // 2. Assistant's tool_use response
    // 3. User's tool_result response

    let response2 = Arc::clone(&client)
        .messages()
        .model("claude-sonnet-4-5")
        .max_tokens(1024)
        .system("You are a helpful assistant with access to weather and calculator tools.")
        // Original user message
        .user_message("What's the weather like in Tokyo? Also, what's 42 * 7?")
        // Assistant's response with tool calls
        .message(Message {
            role: antigravity_gate::Role::Assistant,
            content: MessageContent::Blocks(response.content.clone()),
        })
        // Tool results from user
        .message(Message {
            role: antigravity_gate::Role::User,
            content: MessageContent::Blocks(tool_results),
        })
        .tools(vec![weather_tool, calculator_tool])
        .send()
        .await?;

    println!("=== Final Response ===");
    println!("Stop reason: {:?}\n", response2.stop_reason);
    println!("{}", response2.text());

    println!("\n=== Usage (final turn) ===");
    println!("Input tokens:  {}", response2.usage.input_tokens);
    println!("Output tokens: {}", response2.usage.output_tokens);

    Ok(())
}
