# Tasks: antigravity-gate-rs

## Implementation Overview

This implementation follows a foundation-first approach, building from core types through authentication, then format conversion, and finally the high-level client API. Each phase produces working, testable code.

**Estimated Phases**: 6 major phases, ~25 tasks
**Dependency Order**: Types → Storage → Auth → Convert → Transport → Client

---

## Implementation Plan

### Phase 1: Project Foundation

- [ ] **1. Set up project structure**

- [ ] 1.1 Initialize Cargo project with workspace structure
  - Create `Cargo.toml` with metadata, features, dependencies
  - Set up `src/lib.rs` with module declarations
  - Configure `rustfmt.toml` and `clippy.toml`
  - Add MIT license and README.md stub
  - _Requirements: Compatibility NFR_

- [ ] 1.2 Define core error types
  - Create `src/error.rs` with `Error` enum using thiserror
  - Implement `AuthError`, `ApiError` variants
  - Add `Result<T>` type alias
  - Write unit tests for error display
  - _Requirements: 3.6, 3.7, 4.5_

- [ ] 1.3 Define constants and configuration
  - Create `src/constants.rs` with API endpoints
  - Add OAuth configuration (client ID, URLs, scopes)
  - Implement `get_model_family()` and `is_thinking_model()` functions
  - Add endpoint fallback arrays
  - Write unit tests for model detection
  - _Requirements: 6.1_

---

### Phase 2: Data Models

- [ ] **2. Implement request/response types**

- [ ] 2.1 Create Anthropic request models
  - Create `src/models/request.rs` with `MessagesRequest`
  - Implement `Message`, `Role`, `MessageContent` types
  - Add `SystemPrompt` enum (string or blocks)
  - Add `ThinkingConfig` struct
  - Write unit tests for serialization
  - _Requirements: 3.1, 3.2, 3.3, 3.4, 3.5_

- [ ] 2.2 Create content block types
  - Create `src/models/content.rs` with `ContentBlock` enum
  - Implement `Text`, `ToolUse`, `ToolResult`, `Thinking` variants
  - Add `Image` and `Document` variants for multimodal
  - Write serde tests for tagged enum serialization
  - _Requirements: 5.2, 5.3, 5.6_

- [ ] 2.3 Create tool definition types
  - Create `src/models/tools.rs` with `Tool` struct
  - Implement `ToolChoice` enum (Auto, Any, Tool)
  - Add JSON Schema types for input_schema
  - Write unit tests
  - _Requirements: 3.3, 8.7_

- [ ] 2.4 Create Anthropic response models
  - Create `src/models/response.rs` with `MessagesResponse`
  - Implement `Usage`, `StopReason` types
  - Add helper methods: `text()`, `tool_calls()`, `thinking()`
  - Write unit tests
  - _Requirements: 3.1, 5.4_

- [ ] 2.5 Create streaming event types
  - Create `src/models/stream.rs` with `StreamEvent` enum
  - Implement all event variants (MessageStart, ContentBlockDelta, etc.)
  - Add `ContentDelta` enum for delta types
  - Write deserialization tests
  - _Requirements: 4.2, 4.3, 4.4, 4.6_

- [ ] 2.6 Create Google API models (internal)
  - Create `src/models/google.rs` (pub(crate))
  - Implement `GoogleRequest`, `GoogleResponse`, `Content`, `Part`
  - Add `FunctionDeclaration`, `FunctionCall`, `FunctionResponse`
  - Add `GoogleThinkingConfig` with model-specific variants
  - Write serialization tests
  - _Requirements: 5.1, 5.4, 5.5_

---

### Phase 3: Token Storage

- [ ] **3. Implement storage backends**

- [ ] 3.1 Define TokenStorage trait
  - Create `src/storage/mod.rs` with `TokenStorage` trait
  - Define async methods: load, save, remove, exists
  - Add `name()` method for debugging
  - _Requirements: 7.1, 7.2, 7.3_

- [ ] 3.2 Implement TokenInfo with composite format
  - Create `src/auth/token.rs` with `TokenInfo` struct
  - Implement `parse_refresh_parts()` for composite token parsing
  - Implement `with_project_ids()` for composite token creation
  - Add `is_expired()` and `time_until_expiry()` methods
  - Write round-trip tests for composite format
  - _Requirements: 7.5, 7.6_

- [ ] 3.3 Implement FileTokenStorage
  - Create `src/storage/file.rs` with `FileTokenStorage`
  - Implement load/save with JSON format (matching claude-gate-rs schema)
  - Set file permissions to 0600 on Unix
  - Create parent directories with 0700 permissions
  - Write integration tests with temp files
  - _Requirements: 7.1, Security NFRs_

- [ ] 3.4 Implement MemoryTokenStorage
  - Create `src/storage/memory.rs` with `MemoryTokenStorage`
  - Use `Arc<RwLock<Option<TokenInfo>>>` for thread safety
  - Implement `with_token()` constructor for testing
  - Write unit tests
  - _Requirements: 7.3_

- [ ] 3.5 Implement CallbackStorage
  - Create `src/storage/callback.rs` with `CallbackStorage`
  - Support boxed async closures for load/save/remove
  - Add `FileSource` and `EnvSource` patterns (from claude-gate-rs)
  - Write unit tests with mock callbacks
  - _Requirements: 7.2_

- [ ] 3.6 Implement KeyringTokenStorage (feature-gated)
  - Create `src/storage/keyring.rs` behind `keyring` feature
  - Use `keyring` crate for cross-platform support
  - Implement `is_available()` check
  - Write conditional tests
  - _Requirements: 7.4_

---

### Phase 4: Authentication

- [ ] **4. Implement OAuth and project discovery**

- [ ] 4.1 Implement OAuthConfig
  - Create `src/auth/oauth.rs` with `OAuthConfig` struct
  - Define Google OAuth endpoints and scopes
  - Add `OAuthFlowState` for PKCE state tracking
  - _Requirements: 1.1_

- [ ] 4.2 Implement PKCE flow
  - Add `start_authorization()` with code_verifier and code_challenge
  - Generate random state parameter
  - Build authorization URL with all parameters
  - Write unit tests for PKCE generation
  - _Requirements: 1.1, Security NFRs_

- [ ] 4.3 Implement token exchange
  - Add `exchange_code()` method
  - POST to Google token endpoint with PKCE verifier
  - Parse token response into `TokenInfo`
  - Validate state parameter if provided
  - Write integration test with mock server
  - _Requirements: 1.2_

- [ ] 4.4 Implement token refresh
  - Add `refresh_token()` method
  - Handle composite token format (extract base refresh token)
  - Preserve project IDs through refresh
  - Write unit tests
  - _Requirements: 1.3, 1.4_

- [ ] 4.5 Implement OAuthFlow orchestrator
  - Create `OAuthFlow<S: TokenStorage>` struct
  - Add `get_access_token()` with automatic refresh
  - Implement `is_authenticated()` check
  - Implement `logout()` to clear tokens
  - Write integration tests
  - _Requirements: 1.3, 1.5, 1.6_

- [ ] 4.6 Implement project discovery
  - Create `src/auth/project.rs` with `discover_project()`
  - Call `loadCodeAssist` API with fallback endpoints
  - Parse `cloudaicompanionProject` and `paidTier`
  - Implement `onboard_user()` for new accounts
  - Write mock server tests
  - _Requirements: 2.1, 2.2, 2.3, 2.4, 2.5, 2.6_

---

### Phase 5: Format Conversion

- [ ] **5. Implement Anthropic ↔ Google conversion**

- [ ] 5.1 Implement message content conversion
  - Create `src/convert/content.rs`
  - Convert `ContentBlock` to Google `Part`
  - Handle text, tool_use→functionCall, tool_result→functionResponse
  - Handle thinking blocks with signature preservation
  - Write comprehensive unit tests
  - _Requirements: 5.2, 5.3, 5.6_

- [ ] 5.2 Implement schema sanitization
  - Create `src/convert/schema.rs`
  - Remove unsupported JSON Schema keywords for Google API
  - Handle nested object/array schemas
  - Write unit tests with edge cases
  - _Requirements: 5.7_

- [ ] 5.3 Implement request conversion
  - Create `src/convert/request.rs` with `convert_request()`
  - Convert messages to contents array
  - Convert system prompt to systemInstruction
  - Convert tools to functionDeclarations
  - Handle thinking config per model family
  - Write extensive unit tests
  - _Requirements: 5.1, 5.2, 5.3, 6.2, 6.3, 6.4_

- [ ] 5.4 Implement response conversion
  - Create `src/convert/response.rs` with `convert_response()`
  - Convert Google candidates to Anthropic content
  - Convert functionCall to tool_use blocks
  - Convert thinking parts with signature handling
  - Write unit tests
  - _Requirements: 5.4, 5.5, 5.6_

- [ ] 5.5 Implement signature cache
  - Create `src/convert/thinking.rs` with `SignatureCache`
  - Use LRU cache with TTL (2 hours)
  - Implement `store()`, `get()`, `get_or_sentinel()`
  - Handle cross-model signature recovery
  - Write unit tests for cache behavior
  - _Requirements: 6.5, 6.6, 6.7_

- [ ] 5.6 Implement stream event conversion
  - Add `convert_stream_event()` function
  - Map Google SSE events to Anthropic `StreamEvent`
  - Handle thinking deltas
  - Write unit tests
  - _Requirements: 4.2, 4.3, 4.4, 4.6_

---

### Phase 6: HTTP Transport and Client

- [ ] **6. Implement HTTP client and high-level API**

- [ ] 6.1 Implement HTTP client wrapper
  - Create `src/transport/http.rs` with request building
  - Add header construction with model-specific headers
  - Implement request wrapping for Cloud Code format
  - Handle endpoint fallback on failure
  - Write unit tests
  - _Requirements: 3.1, Reliability NFRs_

- [ ] 6.2 Implement SSE stream parser
  - Create `src/transport/sse.rs` with `SseStream`
  - Parse SSE format (event:, data:, double newline)
  - Implement `Stream<Item = Result<StreamEvent>>`
  - Handle connection errors gracefully
  - Write unit tests with mock streams
  - _Requirements: 4.1, 4.5_

- [ ] 6.3 Implement CloudCodeClient
  - Create `src/client.rs` with `CloudCodeClient<S>`
  - Integrate OAuthFlow for authentication
  - Add project ID caching and discovery
  - Implement `request()` and `request_stream()` methods
  - Write integration tests
  - _Requirements: 1.1-1.6, 2.1-2.6_

- [ ] 6.4 Implement MessagesRequestBuilder
  - Add builder struct with fluent API
  - Implement all builder methods (model, messages, tools, etc.)
  - Add `send()` for non-streaming
  - Add `send_stream()` for streaming
  - Write unit tests for builder
  - _Requirements: 8.1-8.9_

- [ ] 6.5 Implement CloudCodeClientBuilder
  - Add builder for client construction
  - Support custom OAuth config
  - Support custom base URL (for testing)
  - Support custom timeout
  - _Requirements: Compatibility NFRs_

---

### Phase 7: Integration and Polish

- [ ] **7. Testing, examples, and documentation**

- [ ] 7.1 Create example programs
  - Create `examples/basic_usage.rs` - simple message
  - Create `examples/streaming.rs` - streaming response
  - Create `examples/tool_use.rs` - function calling
  - Create `examples/auth_flow.rs` - OAuth walkthrough
  - All examples should compile and have doc comments
  - _Requirements: All_

- [ ] 7.2 Write integration tests
  - Create `tests/integration.rs` with wiremock
  - Test complete OAuth flow
  - Test message exchange with mock Cloud Code
  - Test streaming with mock SSE
  - Test error handling scenarios
  - _Requirements: All_

- [ ] 7.3 Add documentation
  - Write module-level rustdoc for all public modules
  - Document all public types and methods
  - Add usage examples in rustdoc
  - Generate and review docs with `cargo doc`
  - _Requirements: All_

- [ ] 7.4 Final polish
  - Run `cargo clippy` and fix all warnings
  - Run `cargo fmt` for consistent formatting
  - Verify all tests pass: `cargo test --all-features`
  - Test on Linux, macOS, Windows (CI)
  - Update README with usage instructions
  - _Requirements: Compatibility NFRs_

---

## Dependency Graph

```
Phase 1 (Foundation)
    │
    ▼
Phase 2 (Models) ◄──────────────────┐
    │                               │
    ▼                               │
Phase 3 (Storage) ──► Phase 4 (Auth)│
                          │         │
                          ▼         │
                    Phase 5 (Convert)
                          │
                          ▼
                    Phase 6 (Client)
                          │
                          ▼
                    Phase 7 (Polish)
```

---

## Notes

### Porting from claude-gate-rs

The following can be directly ported with minimal changes:
- `TokenStorage` trait and implementations
- `CallbackStorage` with FileSource/EnvSource patterns
- `MemoryTokenStorage`
- Error handling patterns
- Builder API patterns

### Porting from antigravity-claude-proxy

The following need translation from JavaScript to Rust:
- `convertAnthropicToGoogle()` → `convert_request()`
- `convertGoogleToAnthropic()` → `convert_response()`
- `sanitizeSchema()` → `sanitize_schema()`
- Thinking signature cache logic
- Model family detection

### Testing Strategy

- Unit tests: In-module `#[cfg(test)]` blocks
- Integration tests: `tests/` directory with wiremock
- Examples: `examples/` directory (also serve as smoke tests)
- Doc tests: Embedded in rustdoc comments
