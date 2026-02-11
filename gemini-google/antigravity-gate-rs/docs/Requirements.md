# Requirements: antigravity-gate-rs

## Introduction

antigravity-gate-rs is a Rust library crate that provides programmatic access to Google's Cloud Code API, enabling developers to use Claude and Gemini models with Google OAuth credentials. It is the library equivalent of antigravity-claude-proxy, similar to how claude-gate-rs is the library equivalent of claude-gate.

The library addresses the need for embedding Cloud Code API access directly into Rust applications without running a separate proxy server. This enables use cases like:
- Building custom AI assistants with thinking model support
- Integrating Claude/Gemini into Rust CLI tools
- Creating Tauri desktop applications with LLM capabilities
- Server-side AI processing in Rust web services

The primary value proposition is providing a type-safe, async-native Rust API that handles OAuth authentication, format conversion, and streaming responses transparently.

---

## Requirements

### Requirement 1: Google OAuth Authentication

**User Story:** As a developer, I want to authenticate with Google OAuth, so that I can access Cloud Code API using my Google account credentials.

#### Acceptance Criteria

1. WHEN developer calls `start_oauth_flow()` THEN system SHALL return an authorization URL with PKCE code challenge
2. WHEN developer provides authorization code to `complete_oauth_flow()` THEN system SHALL exchange code for access and refresh tokens
3. WHEN access token expires THEN system SHALL automatically refresh using stored refresh token
4. WHEN refresh token is invalid THEN system SHALL return `AuthError::TokenExpired` with clear message
5. WHEN developer calls `is_authenticated()` THEN system SHALL return true if valid tokens exist, false otherwise
6. WHEN developer calls `logout()` THEN system SHALL remove all stored credentials

---

### Requirement 2: Project Discovery

**User Story:** As a developer, I want the library to automatically discover my Cloud Code project ID, so that I don't need to configure it manually.

#### Acceptance Criteria

1. WHEN authenticated user makes first API request THEN system SHALL call `loadCodeAssist` to discover project ID
2. WHEN `loadCodeAssist` returns project data THEN system SHALL extract `cloudaicompanionProject` (managed project ID)
3. WHEN no managed project exists THEN system SHALL call `onboardUser` to create one
4. WHEN project discovery fails THEN system SHALL fall back to default project ID
5. WHEN project ID is discovered THEN system SHALL cache it in the composite token format
6. WHEN user has subscription THEN system SHALL extract tier (free/pro/ultra) from `paidTier` field

---

### Requirement 3: Message API - Non-Streaming

**User Story:** As a developer, I want to send messages and receive complete responses, so that I can integrate AI capabilities into my application.

#### Acceptance Criteria

1. WHEN developer calls `messages().send()` with valid request THEN system SHALL return complete `MessagesResponse`
2. WHEN request includes system prompt THEN system SHALL convert to `systemInstruction` format
3. WHEN request includes tools THEN system SHALL convert to `functionDeclarations` format
4. WHEN request specifies Claude model THEN system SHALL use Claude-specific parameters
5. WHEN request specifies Gemini model THEN system SHALL use Gemini-specific parameters
6. WHEN API returns error THEN system SHALL return typed `Error` with status code and message
7. WHEN rate limited THEN system SHALL return `Error::RateLimit` with retry-after information

---

### Requirement 4: Message API - Streaming

**User Story:** As a developer, I want to receive streaming responses, so that I can display incremental output to users.

#### Acceptance Criteria

1. WHEN developer calls `messages().send_stream()` THEN system SHALL return `Stream<Item = Result<StreamEvent>>`
2. WHEN SSE event `content_block_delta` arrives THEN system SHALL yield `StreamEvent::ContentBlockDelta`
3. WHEN SSE event `content_block_start` arrives THEN system SHALL yield `StreamEvent::ContentBlockStart`
4. WHEN SSE event `message_stop` arrives THEN system SHALL yield `StreamEvent::MessageStop` and complete stream
5. WHEN connection drops THEN system SHALL yield `Error::Connection` and terminate stream
6. WHEN thinking block arrives THEN system SHALL include thinking content in appropriate event

---

### Requirement 5: Format Conversion

**User Story:** As a developer, I want to use Anthropic Messages API format, so that I can write familiar code regardless of underlying API.

#### Acceptance Criteria

1. WHEN request contains Anthropic `messages` array THEN system SHALL convert to Google `contents` array
2. WHEN request contains `tool_result` blocks THEN system SHALL convert to `functionResponse` parts
3. WHEN request contains `tool_use` blocks THEN system SHALL convert to `functionCall` parts
4. WHEN response contains Google format THEN system SHALL convert to Anthropic `MessagesResponse`
5. WHEN response contains `functionCall` THEN system SHALL convert to Anthropic `tool_use` content block
6. WHEN response contains thinking parts THEN system SHALL convert to Anthropic `thinking` content block
7. WHEN tool schema contains unsupported JSON Schema features THEN system SHALL sanitize schema

---

### Requirement 6: Thinking Model Support

**User Story:** As a developer, I want to use thinking models (Claude thinking, Gemini 3+), so that I can access extended reasoning capabilities.

#### Acceptance Criteria

1. WHEN model name contains "thinking" or is Gemini 3+ THEN system SHALL enable thinking mode
2. WHEN Claude thinking model is used THEN system SHALL set `thinkingConfig.include_thoughts = true`
3. WHEN Gemini thinking model is used THEN system SHALL set `thinkingConfig.includeThoughts = true`
4. WHEN `thinking.budget_tokens` is specified THEN system SHALL pass to API
5. WHEN thinking response arrives THEN system SHALL preserve `signature` field for Claude
6. WHEN thinking response arrives THEN system SHALL preserve `thoughtSignature` field for Gemini
7. WHEN unsigned thinking blocks exist in conversation THEN system SHALL attempt signature recovery from cache

---

### Requirement 7: Token Storage

**User Story:** As a developer, I want flexible token storage options, so that I can integrate with my application's security model.

#### Acceptance Criteria

1. WHEN using `FileTokenStorage` THEN system SHALL store tokens at specified path with 0600 permissions
2. WHEN using `CallbackStorage` THEN system SHALL delegate to provided load/save/remove callbacks
3. WHEN using `MemoryTokenStorage` THEN system SHALL store tokens in memory only
4. IF keyring feature is enabled THEN system SHALL support system keyring storage
5. WHEN storing tokens THEN system SHALL use composite format: `refresh|projectId|managedProjectId`
6. WHEN loading tokens THEN system SHALL parse composite format and extract all components

---

### Requirement 8: Builder API

**User Story:** As a developer, I want a fluent builder API, so that I can construct requests with minimal boilerplate.

#### Acceptance Criteria

1. WHEN calling `client.messages()` THEN system SHALL return `MessagesRequestBuilder`
2. WHEN calling `.model(name)` THEN system SHALL set model for request
3. WHEN calling `.user_message(text)` THEN system SHALL add user message with text content
4. WHEN calling `.assistant_message(text)` THEN system SHALL add assistant message
5. WHEN calling `.system(text)` THEN system SHALL set system prompt
6. WHEN calling `.max_tokens(n)` THEN system SHALL set output token limit
7. WHEN calling `.tools([...])` THEN system SHALL attach tool definitions
8. WHEN calling `.tool_choice(choice)` THEN system SHALL set tool calling mode
9. WHEN calling `.thinking_budget(n)` THEN system SHALL set thinking token budget

---

## Non-Functional Requirements

### Performance

1. WHEN making API request THEN system SHALL add no more than 10ms overhead for format conversion
2. WHEN streaming THEN system SHALL yield events within 50ms of receipt
3. WHEN caching tokens THEN system SHALL avoid disk I/O on cached token access

### Security

1. WHEN storing tokens to file THEN system SHALL set permissions to 0600 (owner read/write only)
2. WHEN creating token directory THEN system SHALL set permissions to 0700
3. WHEN logging THEN system SHALL NOT log access tokens or refresh tokens
4. WHEN OAuth state is provided THEN system SHALL validate against expected state

### Reliability

1. WHEN primary endpoint fails THEN system SHALL retry on fallback endpoint
2. WHEN network error occurs THEN system SHALL return `Error::Network` (not panic)
3. WHEN API returns malformed JSON THEN system SHALL return `Error::Parse` with context

### Compatibility

1. system SHALL compile on Linux, macOS, and Windows
2. system SHALL support Rust 1.75+ (async trait stabilization)
3. system SHALL expose both sync and async APIs where practical

---

## Constraints and Assumptions

### Constraints

1. Google OAuth client ID/secret are public (same as Antigravity app)
2. Cloud Code API may change without notice (undocumented)
3. Thinking signatures are model-family specific (Claude vs Gemini incompatible)
4. Maximum output tokens vary by model (Gemini: 16384)

### Assumptions

1. Users have Google accounts with Cloud Code access
2. Network connectivity to Google APIs is available
3. System has writable filesystem for default token storage
4. OAuth callback can be received on localhost (for interactive flows)

---

## Glossary

| Term | Definition |
|------|------------|
| Cloud Code | Google's API for accessing Claude and Gemini models |
| Composite Token | Format: `refreshToken\|projectId\|managedProjectId` |
| Thinking Model | Model with extended reasoning (Claude thinking, Gemini 3+) |
| Thinking Signature | Cryptographic marker for thinking block continuity |
| PKCE | Proof Key for Code Exchange (OAuth extension for public clients) |
