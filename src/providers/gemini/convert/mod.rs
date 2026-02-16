// Allow dead code since these conversion functions will be used by the client module
// in Phase 6 when transport is implemented
#![allow(dead_code)]

//! Format conversion between Anthropic and Google Generative AI APIs.
//!
//! This module provides bidirectional conversion between the Anthropic Messages API
//! format and the Google Generative AI format used by Cloud Code.
//!
//! ## Module Structure
//!
//! - `content`: Convert content blocks to/from Google Parts
//! - `schema`: Sanitize JSON Schema for Google API compatibility
//! - `request`: Convert `MessagesRequest` to `GoogleRequest`
//! - `response`: Convert `GoogleResponse` to `MessagesResponse`
//! - `thinking`: Signature cache for thinking block continuity
//!
//! ## Example
//!
//! ```rust,ignore
//! use gaud::providers::gemini::convert::{convert_request, convert_response};
//!
//! // Convert Anthropic request to Google format
//! let google_request = convert_request(&anthropic_request)?;
//!
//! // ... send to Cloud Code API ...
//!
//! // Convert Google response back to Anthropic format
//! let anthropic_response = convert_response(&google_response, "claude-sonnet-4-5")?;
//! ```

mod content;
mod request;
mod response;
mod schema;
mod thinking;

// Re-export public utilities
pub use content::convert_role;
pub use schema::sanitize_schema;
pub use thinking::{GLOBAL_SIGNATURE_CACHE, SignatureCache};

// Re-export internal conversion functions for use within the crate
// These will be used by the client/transport modules in future phases
#[allow(unused_imports)]
pub(crate) use content::{convert_content_to_parts, convert_parts_to_content};
#[allow(unused_imports)]
pub(crate) use request::convert_request;
#[allow(unused_imports)]
pub(crate) use response::convert_response;
