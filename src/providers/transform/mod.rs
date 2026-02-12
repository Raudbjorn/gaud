//! Shared transformation utilities and provider-specific transformers.
//!
//! This module contains the common infrastructure for converting between
//! OpenAI-compatible format and provider-specific API formats.

pub mod claude;
pub mod copilot;
pub mod gemini;
pub mod kiro;
pub mod sse;
pub mod util;

// Re-export commonly used items.
pub use claude::ClaudeTransformer;
pub use copilot::CopilotTransformer;
pub use crate::providers::transform::gemini::GeminiTransformer;
pub use kiro::KiroTransformer;
pub use sse::{SseEvent, SseParser};
pub use util::*;
