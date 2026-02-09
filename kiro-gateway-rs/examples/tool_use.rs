//! Tool use example: define tools and handle tool calls.

use kiro_gateway::{
    ContentBlock, KiroClientBuilder, ResponseContentBlock, Result, Role, StopReason, ToolChoice,
};
use serde_json::json;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter("kiro_gateway=info")
        .init();

    let client = KiroClientBuilder::new()
        .credentials_file("~/.kiro/credentials.json")
        .build()
        .await?;

    // Define a weather tool
    let response = client
        .messages()
        .model("claude-sonnet-4.5")
        .max_tokens(1024)
        .tool(
            "get_weather",
            "Get the current weather for a location.",
            json!({
                "type": "object",
                "properties": {
                    "location": {
                        "type": "string",
                        "description": "City name"
                    }
                },
                "required": ["location"]
            }),
        )
        .tool_choice(ToolChoice::Auto)
        .user_message("What's the weather in Reykjavik?")
        .send()
        .await?;

    println!("Stop reason: {:?}", response.stop_reason);

    if response.stop_reason == Some(StopReason::ToolUse) {
        for block in &response.content {
            if let ResponseContentBlock::ToolUse { id, name, input } = block {
                println!("Tool call: {} (id: {})", name, id);
                println!("Input: {}", serde_json::to_string_pretty(input).unwrap());

                // Simulate tool response
                let tool_result = json!({
                    "temperature": "8°C",
                    "condition": "Partly cloudy",
                    "wind": "15 km/h NW"
                });

                // Send tool result back
                let final_response = client
                    .messages()
                    .model("claude-sonnet-4.5")
                    .max_tokens(1024)
                    .user_message("What's the weather in Reykjavik?")
                    .assistant_message(&response.text())
                    .message(
                        Role::User,
                        vec![ContentBlock::ToolResult {
                            tool_use_id: id.clone(),
                            content: kiro_gateway::models::request::ToolResultContent::Text(
                                tool_result.to_string(),
                            ),
                            is_error: false,
                        }],
                    )
                    .send()
                    .await?;

                println!("\nFinal response: {}", final_response.text());
            }
        }
    }

    Ok(())
}
