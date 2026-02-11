# Design: antigravity-gate-rs

## Overview

This document describes the technical architecture for antigravity-gate-rs, a Rust library providing programmatic access to Google's Cloud Code API. The design follows patterns established by claude-gate-rs while adapting to the unique requirements of Cloud Code authentication and format conversion.

### Design Goals

1. **API Familiarity**: Use Anthropic Messages API format as the primary interface
2. **Zero-Copy Streaming**: Efficient SSE parsing without unnecessary allocations
3. **Pluggable Storage**: Support multiple token storage backends via traits
4. **Type Safety**: Leverage Rust's type system for compile-time correctness
5. **Minimal Dependencies**: Only include essential crates

### Key Design Decisions

1. **Anthropic-first API**: Users write Anthropic format; conversion is internal
2. **Composite Token Storage**: Encode project IDs in token format for single-file storage
3. **Lazy Project Discovery**: Only call loadCodeAssist when project ID is needed
4. **Signature Cache**: In-memory LRU cache for thinking signature recovery
5. **Feature Flags**: Optional keyring support, CLI tools behind features

---

## Architecture

### System Overview

```
┌──────────────────────────────────────────────────────────────────────┐
│                         User Application                              │
└──────────────────────────────────┬───────────────────────────────────┘
                                   │
                                   ▼
┌──────────────────────────────────────────────────────────────────────┐
│                      CloudCodeClient<S: TokenStorage>                 │
│  ┌────────────────┐  ┌───────────────┐  ┌────────────────────────┐   │
│  │  OAuthFlow<S>  │  │ RequestBuilder │  │   ResponseConverter    │   │
│  │  (Google Auth) │  │ (Anthropic→GCP)│  │   (GCP→Anthropic)      │   │
│  └───────┬────────┘  └───────┬───────┘  └───────────┬────────────┘   │
│          │                   │                      │                │
│          ▼                   ▼                      ▼                │
│  ┌────────────────┐  ┌───────────────┐  ┌────────────────────────┐   │
│  │ TokenStorage   │  │  HttpClient   │  │   SignatureCache       │   │
│  │ (File/Keyring/ │  │  (reqwest)    │  │   (LRU + TTL)          │   │
│  │  Callback)     │  │               │  │                        │   │
│  └────────────────┘  └───────────────┘  └────────────────────────┘   │
└──────────────────────────────────────────────────────────────────────┘
                                   │
                                   ▼
┌──────────────────────────────────────────────────────────────────────┐
│                     Google Cloud Code API                             │
│  ┌─────────────────────┐  ┌─────────────────────────────────────┐    │
│  │ cloudcode-pa.       │  │ daily-cloudcode-pa.googleapis.com   │    │
│  │ googleapis.com      │  │ (fallback)                          │    │
│  └─────────────────────┘  └─────────────────────────────────────┘    │
└──────────────────────────────────────────────────────────────────────┘
```

### Component Architecture

```
antigravity-gate/
├── src/
│   ├── lib.rs              # Public API exports
│   ├── client.rs           # CloudCodeClient + MessagesRequestBuilder
│   ├── auth/
│   │   ├── mod.rs          # Auth module exports
│   │   ├── oauth.rs        # Google OAuth PKCE flow
│   │   ├── project.rs      # Project discovery (loadCodeAssist)
│   │   └── token.rs        # TokenInfo, composite format parsing
│   ├── storage/
│   │   ├── mod.rs          # TokenStorage trait + re-exports
│   │   ├── file.rs         # FileTokenStorage
│   │   ├── memory.rs       # MemoryTokenStorage
│   │   ├── callback.rs     # CallbackStorage
│   │   └── keyring.rs      # KeyringTokenStorage (feature-gated)
│   ├── convert/
│   │   ├── mod.rs          # Conversion module exports
│   │   ├── request.rs      # Anthropic → Google request conversion
│   │   ├── response.rs     # Google → Anthropic response conversion
│   │   ├── content.rs      # Content block conversion
│   │   ├── schema.rs       # JSON Schema sanitization
│   │   └── thinking.rs     # Thinking block handling + signatures
│   ├── models/
│   │   ├── mod.rs          # Model types exports
│   │   ├── request.rs      # Anthropic request types
│   │   ├── response.rs     # Anthropic response types
│   │   ├── content.rs      # Content blocks (text, tool_use, thinking)
│   │   ├── tools.rs        # Tool definitions
│   │   └── stream.rs       # Streaming event types
│   ├── transport/
│   │   ├── mod.rs          # Transport module exports
│   │   ├── http.rs         # HTTP client wrapper
│   │   └── sse.rs          # SSE stream parser
│   ├── error.rs            # Error types
│   └── constants.rs        # API endpoints, OAuth config, model detection
└── examples/
    ├── basic_usage.rs
    ├── streaming.rs
    ├── tool_use.rs
    └── auth_flow.rs
```

### Technology Stack

| Layer | Technology | Rationale |
|-------|------------|-----------|
| HTTP Client | reqwest 0.12 | Mature, async, streaming support |
| Async Runtime | tokio | Industry standard, required by reqwest |
| Serialization | serde + serde_json | De facto standard for Rust |
| OAuth | oauth2 | Well-maintained, PKCE support |
| Crypto | sha2, base64, rand | Standard crates for PKCE |
| Streaming | async-stream | Clean async iterator pattern |
| Keyring | keyring (optional) | Cross-platform secret storage |

---

## Components and Interfaces

### CloudCodeClient

**Purpose**: Primary entry point for API access

**Responsibilities**:
- Manage OAuth flow lifecycle
- Build and send API requests
- Handle token refresh transparently
- Cache project ID

**Interface**:
```rust
impl<S: TokenStorage + 'static> CloudCodeClient<S> {
    // Construction
    pub fn builder() -> CloudCodeClientBuilder<S>;
    pub fn new(storage: S) -> Result<Self>;

    // Authentication
    pub async fn is_authenticated(&self) -> Result<bool>;
    pub async fn start_oauth_flow(&self) -> Result<String>;
    pub async fn complete_oauth_flow(&self, code: &str, state: Option<&str>) -> Result<TokenInfo>;
    pub async fn logout(&self) -> Result<()>;

    // API Access
    pub fn messages(&self) -> MessagesRequestBuilder<'_, S>;

    // Low-level
    pub async fn request(&self, method: Method, path: &str, body: Option<Value>) -> Result<Response>;
    pub async fn request_stream(&self, path: &str, body: Value) -> Result<SseStream>;
}
```

### OAuthFlow

**Purpose**: Handle Google OAuth 2.0 with PKCE

**Responsibilities**:
- Generate authorization URL with code challenge
- Exchange authorization code for tokens
- Refresh access tokens
- Store/retrieve tokens via TokenStorage

**Interface**:
```rust
impl<S: TokenStorage> OAuthFlow<S> {
    pub fn new(storage: S) -> Self;
    pub fn with_config(storage: S, config: OAuthConfig) -> Self;

    pub fn start_authorization(&mut self) -> Result<(String, OAuthFlowState)>;
    pub async fn exchange_code(&mut self, code: &str, state: Option<&str>) -> Result<TokenInfo>;
    pub async fn get_access_token(&self) -> Result<String>;
    pub async fn is_authenticated(&self) -> Result<bool>;
    pub async fn logout(&self) -> Result<()>;
}
```

### ProjectDiscovery

**Purpose**: Discover and manage Cloud Code project IDs

**Responsibilities**:
- Call loadCodeAssist API
- Parse project response
- Trigger onboardUser if needed
- Extract subscription tier

**Interface**:
```rust
pub async fn discover_project(token: &str, hint_project_id: Option<&str>) -> Result<ProjectInfo>;
pub async fn onboard_user(token: &str, tier: &str) -> Result<String>;

pub struct ProjectInfo {
    pub project_id: String,
    pub managed_project_id: Option<String>,
    pub subscription_tier: SubscriptionTier,
}

pub enum SubscriptionTier {
    Free,
    Pro,
    Ultra,
    Unknown,
}
```

### RequestConverter

**Purpose**: Convert Anthropic format to Google Generative AI format

**Responsibilities**:
- Convert messages array to contents array
- Convert tool definitions to functionDeclarations
- Convert tool results to functionResponse
- Handle thinking config per model family
- Sanitize JSON schemas

**Interface**:
```rust
pub fn convert_request(request: &MessagesRequest, model: &str) -> Result<GoogleRequest>;

// Internal helpers
fn convert_messages(messages: &[Message], model_family: ModelFamily) -> Vec<Content>;
fn convert_tools(tools: &[Tool]) -> Vec<FunctionDeclaration>;
fn convert_thinking_config(thinking: Option<&ThinkingConfig>, family: ModelFamily) -> Option<GoogleThinkingConfig>;
```

### ResponseConverter

**Purpose**: Convert Google format back to Anthropic format

**Responsibilities**:
- Convert candidates to content array
- Convert functionCall to tool_use blocks
- Convert thinking parts to thinking blocks
- Preserve signatures for thinking continuity

**Interface**:
```rust
pub fn convert_response(response: GoogleResponse, model: &str) -> Result<MessagesResponse>;
pub fn convert_stream_event(event: GoogleStreamEvent, model: &str) -> Result<StreamEvent>;
```

### SignatureCache

**Purpose**: Cache thinking signatures for recovery

**Responsibilities**:
- Store signature → model family mappings
- Retrieve signatures for unsigned thinking blocks
- Expire entries after TTL
- Provide sentinel value for unrecoverable cases

**Interface**:
```rust
pub struct SignatureCache {
    cache: RwLock<LruCache<String, CachedSignature>>,
    ttl: Duration,
}

impl SignatureCache {
    pub fn new(capacity: usize, ttl: Duration) -> Self;
    pub fn store(&self, signature: &str, family: ModelFamily);
    pub fn get(&self, signature: &str) -> Option<ModelFamily>;
    pub fn get_or_sentinel(&self, content_hash: &str) -> String;
}
```

### TokenStorage Trait

**Purpose**: Abstract token persistence

**Interface** (same as claude-gate-rs for compatibility):
```rust
#[async_trait]
pub trait TokenStorage: Send + Sync {
    async fn load(&self) -> Result<Option<TokenInfo>>;
    async fn save(&self, token: &TokenInfo) -> Result<()>;
    async fn remove(&self) -> Result<()>;
    async fn exists(&self) -> Result<bool> { ... }
    fn name(&self) -> &str { "unknown" }
}
```

---

## Data Models

### TokenInfo

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenInfo {
    pub token_type: String,           // "oauth"
    pub access_token: String,
    pub refresh_token: String,        // Composite: "refresh|projectId|managedProjectId"
    pub expires_at: i64,              // Unix timestamp
}

impl TokenInfo {
    pub fn new(access: String, refresh: String, expires_in: i64) -> Self;
    pub fn is_expired(&self) -> bool;
    pub fn time_until_expiry(&self) -> Duration;

    // Composite token helpers
    pub fn parse_refresh_parts(&self) -> (String, Option<String>, Option<String>);
    pub fn with_project_ids(self, project: &str, managed: Option<&str>) -> Self;
}
```

- **Validation**: access_token and refresh_token must be non-empty
- **Relationships**: Stored by TokenStorage implementations

### MessagesRequest

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessagesRequest {
    pub model: String,
    pub messages: Vec<Message>,
    pub max_tokens: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system: Option<SystemPrompt>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<Tool>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_choice: Option<ToolChoice>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking: Option<ThinkingConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stream: Option<bool>,
}
```

### Message

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: Role,
    pub content: MessageContent,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    User,
    Assistant,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum MessageContent {
    Text(String),
    Blocks(Vec<ContentBlock>),
}
```

### ContentBlock

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentBlock {
    Text {
        text: String,
    },
    ToolUse {
        id: String,
        name: String,
        input: Value,
    },
    ToolResult {
        tool_use_id: String,
        content: ToolResultContent,
        #[serde(skip_serializing_if = "Option::is_none")]
        is_error: Option<bool>,
    },
    Thinking {
        thinking: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        signature: Option<String>,
    },
    Image {
        source: ImageSource,
    },
    Document {
        source: DocumentSource,
    },
}
```

### MessagesResponse

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessagesResponse {
    pub id: String,
    pub model: String,
    pub role: Role,
    pub content: Vec<ContentBlock>,
    pub stop_reason: Option<StopReason>,
    pub usage: Usage,
}

impl MessagesResponse {
    pub fn text(&self) -> String;           // Extract all text content
    pub fn tool_calls(&self) -> Vec<&ContentBlock>;  // Extract tool_use blocks
    pub fn thinking(&self) -> Option<&str>; // Extract thinking content
}
```

### StreamEvent

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum StreamEvent {
    MessageStart { message: PartialMessage },
    ContentBlockStart { index: usize, content_block: ContentBlock },
    ContentBlockDelta { index: usize, delta: ContentDelta },
    ContentBlockStop { index: usize },
    MessageDelta { delta: MessageDelta, usage: Option<Usage> },
    MessageStop,
    Ping,
    Error { error: ApiError },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentDelta {
    TextDelta { text: String },
    InputJsonDelta { partial_json: String },
    ThinkingDelta { thinking: String },
}
```

---

## API Design

### Cloud Code Endpoints

| Endpoint | Method | Purpose |
|----------|--------|---------|
| `/v1internal:generateContent` | POST | Non-streaming messages |
| `/v1internal:streamGenerateContent?alt=sse` | POST | Streaming messages |
| `/v1internal:loadCodeAssist` | POST | Project discovery |
| `/v1internal:onboardUser` | POST | Create managed project |
| `/v1internal:fetchAvailableModels` | POST | List models + quotas |

### Request Wrapping

Cloud Code requires requests to be wrapped:

```json
{
  "project": "project-id",
  "model": "claude-sonnet-4-5-thinking",
  "request": { /* Google Generative AI format */ },
  "userAgent": "antigravity",
  "requestType": "agent",
  "requestId": "agent-uuid"
}
```

### Headers

```rust
const HEADERS: &[(&str, &str)] = &[
    ("User-Agent", "antigravity/1.11.5 linux/x64"),
    ("X-Goog-Api-Client", "google-cloud-sdk vscode_cloudshelleditor/0.1"),
    ("Client-Metadata", r#"{"ideType":"IDE_UNSPECIFIED","platform":"PLATFORM_UNSPECIFIED","pluginType":"GEMINI"}"#),
];

// For Claude thinking models, add:
("anthropic-beta", "interleaved-thinking-2025-05-14")
```

---

## Error Handling

| Category | Error Type | HTTP Status | User Action |
|----------|------------|-------------|-------------|
| Authentication | `AuthError::NotAuthenticated` | - | Start OAuth flow |
| Authentication | `AuthError::TokenExpired` | 401 | Re-authenticate |
| Authentication | `AuthError::InvalidGrant` | 400 | Re-authenticate (refresh invalid) |
| Rate Limit | `ApiError::RateLimit` | 429 | Wait and retry |
| Validation | `ApiError::InvalidRequest` | 400 | Fix request parameters |
| Server | `ApiError::ServerError` | 500-599 | Retry with backoff |
| Network | `Error::Network` | - | Check connectivity |
| Parse | `Error::Parse` | - | Report bug (malformed response) |

```rust
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("Authentication error: {0}")]
    Auth(#[from] AuthError),

    #[error("API error: {status} - {message}")]
    Api { status: u16, message: String, retry_after: Option<Duration> },

    #[error("Network error: {0}")]
    Network(#[from] reqwest::Error),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("Configuration error: {0}")]
    Config(String),

    #[error("Storage error: {0}")]
    Storage(String),
}
```

---

## Testing Strategy

### Unit Testing

- **Coverage Target**: 80%+
- **Focus Areas**:
  - Format conversion (request/response)
  - Token parsing (composite format)
  - Model family detection
  - Schema sanitization
  - Signature cache operations

### Integration Testing

- **Mock Server**: Use `wiremock` for Cloud Code API simulation
- **Test Scenarios**:
  - Complete OAuth flow
  - Non-streaming message exchange
  - Streaming with thinking blocks
  - Tool use round-trip
  - Error responses

### Example Tests

```rust
#[tokio::test]
async fn test_convert_simple_message() {
    let request = MessagesRequest {
        model: "claude-sonnet-4-5-thinking".to_string(),
        messages: vec![Message::user("Hello")],
        max_tokens: 1024,
        ..Default::default()
    };

    let google = convert_request(&request, &request.model).unwrap();

    assert_eq!(google.contents.len(), 1);
    assert_eq!(google.contents[0].role, "user");
}

#[tokio::test]
async fn test_composite_token_round_trip() {
    let token = TokenInfo::new(
        "access".into(),
        "refresh".into(),
        3600,
    ).with_project_ids("proj-123", Some("managed-456"));

    let (refresh, proj, managed) = token.parse_refresh_parts();
    assert_eq!(refresh, "refresh");
    assert_eq!(proj, Some("proj-123".to_string()));
    assert_eq!(managed, Some("managed-456".to_string()));
}
```

---

## Security Considerations

1. **Token Storage**: Default file storage uses 0600 permissions
2. **Logging**: Access/refresh tokens are never logged (use `#[sensitive]` macro)
3. **PKCE**: All OAuth flows use PKCE (no client secret exposure)
4. **State Validation**: OAuth state parameter validated to prevent CSRF
5. **TLS**: All API calls use HTTPS (enforced by reqwest)

---

## Performance Considerations

1. **Token Caching**: Access token cached in memory to avoid storage I/O
2. **Connection Pooling**: reqwest Client reused across requests
3. **Zero-Copy Parsing**: SSE parser avoids unnecessary string allocations
4. **Lazy Conversion**: Request conversion happens once, not on retry

---

## Feature Flags

```toml
[features]
default = []
keyring = ["dep:keyring"]        # System keyring support
cli = ["dep:clap", "dep:colored"] # CLI binary
full = ["keyring", "cli"]
```
