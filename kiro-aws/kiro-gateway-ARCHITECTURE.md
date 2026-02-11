# Architecture Overview: Kiro Gateway

## 1. Purpose and System Goals

Kiro Gateway is a high-level proxy gateway implementing the **Adapter** structural design pattern.

The primary goal of the system is to provide transparent compatibility between several heterogeneous interfaces:

### Supported API Formats

| API | Endpoints | Status |
|-----|-----------|--------|
| **OpenAI** | `/v1/models`, `/v1/chat/completions` | ✅ Supported |
| **Anthropic** | `/v1/messages` | ✅ Supported |

### Architectural Model

```
┌─────────────────────────────────────────────────────────────────┐
│                           Clients                               │
│  ┌─────────────────────┐       ┌─────────────────────┐         │
│  │  OpenAI SDK/Tools   │       │ Anthropic SDK/Tools │         │
│  │  (Cursor, Cline,    │       │ (Claude Code,       │         │
│  │   Continue, etc.)   │       │  Anthropic SDK)     │         │
│  └──────────┬──────────┘       └──────────┬──────────┘         │
└─────────────┼──────────────────────────────┼───────────────────┘
              │                              │
              ▼                              ▼
┌─────────────────────────────────────────────────────────────────┐
│                        Kiro Gateway                             │
│  ┌─────────────────────┐       ┌─────────────────────┐         │
│  │  OpenAI Adapter     │       │  Anthropic Adapter  │         │
│  │  /v1/chat/...       │       │  /v1/messages       │         │
│  └──────────┬──────────┘       └──────────┬──────────┘         │
│             └──────────────┬───────────────┘                    │
│                            ▼                                    │
│             ┌─────────────────────────────┐                     │
│             │         Core Layer          │                     │
│             │   (Shared conversion logic) │                     │
│             └──────────────┬──────────────┘                     │
└────────────────────────────┼────────────────────────────────────┘
                             │
                             ▼
┌─────────────────────────────────────────────────────────────────┐
│                          Kiro API                               │
│                  (AWS CodeWhisperer Backend)                    │
└─────────────────────────────────────────────────────────────────┘
```

The system acts as a "translator," allowing any tools, libraries, and IDE plugins built for the OpenAI and Anthropic ecosystems to work with Claude models via the Kiro API.

**Both APIs run simultaneously** on a single server with no configuration switching required.

---

## 2. Project Structure

The project is organized as a modular Python package `kiro/`:

```
kiro-gateway/
├── main.py                    # Entry point, FastAPI app creation
├── requirements.txt           # Python dependencies
├── .env.example               # Example environment configuration
│
├── kiro/                      # Main package
│   ├── __init__.py            # Package exports, version
│   │
│   │   # ═══════════════════════════════════════════════════════
│   │   # SHARED LAYER - Reused across all APIs
│   │   # ═══════════════════════════════════════════════════════
│   ├── config.py              # Configuration and constants
│   ├── auth.py                # KiroAuthManager - token management
│   ├── cache.py               # ModelInfoCache - model cache
│   ├── http_client.py         # HTTP client with retry logic
│   ├── parsers.py             # AWS SSE stream parsers
│   ├── utils.py               # Utility helpers
│   ├── tokenizer.py           # Token counting (tiktoken)
│   ├── debug_logger.py        # Debug request logging
│   ├── exceptions.py          # Exception handlers
│   ├── thinking_parser.py     # Thinking block parser
│   │
│   │   # ═══════════════════════════════════════════════════════
│   │   # CORE LAYER - Shared core for all APIs
│   │   # ═══════════════════════════════════════════════════════
│   ├── converters_core.py     # Shared logic for building Kiro payload
│   ├── streaming_core.py      # Shared logic for parsing Kiro stream
│   │
│   │   # ═══════════════════════════════════════════════════════
│   │   # OPENAI API LAYER
│   │   # ═══════════════════════════════════════════════════════
│   ├── models_openai.py       # Pydantic models for OpenAI API
│   ├── converters_openai.py   # OpenAI → Kiro adapter
│   ├── routes_openai.py       # FastAPI routes for OpenAI
│   ├── streaming_openai.py    # Kiro → OpenAI SSE formatter
│   │
│   │   # ═══════════════════════════════════════════════════════
│   │   # ANTHROPIC API LAYER
│   │   # ═══════════════════════════════════════════════════════
│   ├── models_anthropic.py    # Pydantic models for Anthropic API
│   ├── converters_anthropic.py # Anthropic → Kiro adapter
│   ├── routes_anthropic.py    # FastAPI routes for Anthropic
│   └── streaming_anthropic.py # Kiro → Anthropic SSE formatter
│
├── tests/                     # Tests
│   ├── conftest.py            # Pytest fixtures
│   ├── unit/                  # Unit tests
│   └── integration/           # Integration tests
│
├── docs/                      # Documentation
│   ├── ru/                    # Russian version
│   └── en/                    # English version
│
└── debug_logs/                # Debug logs (generated when DEBUG_MODE=all or DEBUG_MODE=errors)
```

### Organization Principle: Shared Core + Thin Adapters

The architecture is built on the principle of **maximum code reuse**:

| Layer | Purpose | Files |
|-------|---------|-------|
| **Shared Layer** | Infrastructure independent of API format | `auth.py`, `http_client.py`, `cache.py`, `parsers.py`, `tokenizer.py` |
| **Core Layer** | Shared business logic for conversion | `converters_core.py`, `streaming_core.py` |
| **API Layer** | Thin adapters for specific formats | `*_openai.py`, `*_anthropic.py` |

---

## 3. Architecture Topology and Components

The system is built on the async `FastAPI` framework and uses an event-driven lifecycle model (`Lifespan Events`).

### 3.1. Entry Point (`main.py`)

`main.py` is responsible for:

1. **Logging configuration** — setting up Loguru with colored output
2. **Configuration validation** — `validate_configuration()` checks:
   - Presence of `.env` file
   - Presence of credentials (`REFRESH_TOKEN` or `KIRO_CREDS_FILE`)
3. **Lifespan Manager** — creates and initializes:
   - `KiroAuthManager` for token management
   - `ModelInfoCache` for model caching
4. **Error handler registration** — `validation_exception_handler` for 422 errors
5. **Router registration** — `app.include_router(router)`

### 3.2. Configuration Module (`kiro/config.py`)

Centralized storage for all settings:

| Parameter | Description | Default |
|-----------|-------------|---------|
| `PROXY_API_KEY` | API key for proxy access | `changeme_proxy_secret` |
| `REFRESH_TOKEN` | Kiro refresh token | from `.env` |
| `PROFILE_ARN` | AWS CodeWhisperer profile ARN | from `.env` |
| `REGION` | AWS region | `us-east-1` |
| `KIRO_CREDS_FILE` | Path to JSON credentials file | from `.env` |
| `TOKEN_REFRESH_THRESHOLD` | Time before token refresh | 600 sec (10 min) |
| `MAX_RETRIES` | Max retry attempts | 3 |
| `BASE_RETRY_DELAY` | Base retry delay | 1.0 sec |
| `MODEL_CACHE_TTL` | Model cache TTL | 3600 sec (1 hour) |
| `DEFAULT_MAX_INPUT_TOKENS` | Default max input tokens | 200000 |
| `TOOL_DESCRIPTION_MAX_LENGTH` | Max tool description length | 10000 chars |
| `DEBUG_MODE` | Debug logging mode | `off` (off/errors/all) |
| `DEBUG_DIR` | Debug log directory | `debug_logs` |
| `APP_VERSION` | Application version | `0.0.0` |

**Helper functions:**
- `get_kiro_refresh_url(region)` — URL for token refresh
- `get_kiro_api_host(region)` — main API host
- `get_kiro_q_host(region)` — Q API host
- `get_internal_model_id(external_model)` — model name conversion

### 3.3. Pydantic Models (`kiro/models_openai.py`)

#### Models for `/v1/models`

| Model | Description |
|-------|-------------|
| `OpenAIModel` | AI model description (id, object, created, owned_by) |
| `ModelList` | List of models for the endpoint response |

#### Models for `/v1/chat/completions`

| Model | Description |
|-------|-------------|
| `ChatMessage` | Chat message (role, content, tool_calls, tool_call_id) |
| `ToolFunction` | Tool function description (name, description, parameters) |
| `Tool` | OpenAI-format tool (type, function) |
| `ChatCompletionRequest` | Generation request (model, messages, stream, tools, ...) |

#### Response Models

| Model | Description |
|-------|-------------|
| `ChatCompletionChoice` | Single response choice |
| `ChatCompletionUsage` | Token usage info (prompt_tokens, completion_tokens, credits_used) |
| `ChatCompletionResponse` | Full response (non-streaming) |
| `ChatCompletionChunk` | Streaming chunk |
| `ChatCompletionChunkDelta` | Delta changes in a chunk |
| `ChatCompletionChunkChoice` | Choice in a streaming chunk |

### 3.4. State Management Layer

#### KiroAuthManager (`kiro/auth.py`)

**Role:** Stateful singleton encapsulating Kiro token management logic.

**Capabilities:**
- Load credentials from `.env` or JSON file
- Support `expiresAt` for token expiration checking
- Automatic token refresh 10 minutes before expiration
- Save updated tokens back to JSON file
- Support for different AWS regions
- Generate unique machine fingerprint for User-Agent

**Concurrency Control:** Uses `asyncio.Lock` to protect against race conditions.

**Key methods:**
- `get_access_token()` — returns a valid token, refreshing if necessary
- `force_refresh()` — force token refresh (on 403)
- `is_token_expiring_soon()` — check token expiration time

**Properties:**
- `profile_arn` — profile ARN
- `region` — AWS region
- `api_host` — API host for the region
- `q_host` — Q API host for the region
- `fingerprint` — unique machine fingerprint

```python
# Usage example
auth_manager = KiroAuthManager(
    refresh_token="your_token",
    region="us-east-1",
    creds_file="~/.aws/sso/cache/kiro-auth-token.json"
)
token = await auth_manager.get_access_token()
```

#### ModelInfoCache (`kiro/cache.py`)

**Role:** Thread-safe store for model configurations.

**Population Strategy:**
- Lazy loading via `/ListAvailableModels`
- Cache TTL: 1 hour
- Fallback to a static model list

**Key methods:**
- `update(models_data)` — update cache
- `get(model_id)` — get model info
- `get_max_input_tokens(model_id)` — get token limit for a model
- `is_empty()` / `is_stale()` — check cache state
- `get_all_model_ids()` — list all model IDs

### 3.5. Utility Helpers (`kiro/utils.py`)

| Function | Description |
|----------|-------------|
| `get_machine_fingerprint()` | SHA256 hash of `{hostname}-{username}-kiro-gateway` |
| `get_kiro_headers(auth_manager, token)` | Build headers for Kiro API requests |
| `generate_completion_id()` | ID in format `chatcmpl-{uuid_hex}` |
| `generate_conversation_id()` | UUID for a conversation |
| `generate_tool_call_id()` | ID in format `call_{uuid_hex[:8]}` |

### 3.6. Conversion Layer (`kiro/converters_openai.py`)

#### Message Conversion

OpenAI messages are converted into a Kiro `conversationState`:

1. **System prompt** — appended to the first user message
2. **Message history** — fully passed in the `history` array
3. **Adjacent message merging** — messages with the same role are merged
4. **Tool calls** — support for OpenAI tools format
5. **Tool results** — correct passing of tool call results

#### Handling Long Tool Descriptions

**Problem:** The Kiro API returns a 400 error when `toolSpecification.description` is too long.

**Solution:** Tool Documentation Reference Pattern
- If `description ≤ TOOL_DESCRIPTION_MAX_LENGTH` → keep as-is
- If `description > TOOL_DESCRIPTION_MAX_LENGTH`:
  - In `toolSpecification.description` → reference: `"[Full documentation in system prompt under '## Tool: {name}']"`
  - A section `"## Tool: {name}"` with full documentation is added to the system prompt

**Function:** `process_tools_with_long_descriptions(tools)` → `(processed_tools, tool_documentation)`

#### Key Functions

| Function | Description |
|----------|-------------|
| `extract_text_content(content)` | Extract text from various content formats |
| `merge_adjacent_messages(messages)` | Merge adjacent messages with the same role |
| `build_kiro_history(messages, model_id)` | Build the history array for Kiro |
| `build_kiro_payload(request_data, conversation_id, profile_arn)` | Full payload for request |

#### Model Mapping

External model names are mapped to internal Kiro IDs:

| External Name | Internal Kiro ID |
|---------------|-----------------|
| `claude-opus-4-5` | `claude-opus-4.5` |
| `claude-opus-4-5-20251101` | `claude-opus-4.5` |
| `claude-haiku-4-5` | `claude-haiku-4.5` |
| `claude-haiku-4.5` | `claude-haiku-4.5` (pass-through) |
| `claude-sonnet-4-5` | `CLAUDE_SONNET_4_5_20250929_V1_0` |
| `claude-sonnet-4-5-20250929` | `CLAUDE_SONNET_4_5_20250929_V1_0` |
| `claude-sonnet-4` | `CLAUDE_SONNET_4_20250514_V1_0` |
| `claude-sonnet-4-20250514` | `CLAUDE_SONNET_4_20250514_V1_0` |
| `claude-3-7-sonnet-20250219` | `CLAUDE_3_7_SONNET_20250219_V1_0` |
| `auto` | `claude-sonnet-4.5` (alias) |

### 3.7. Parsing Layer (`kiro/parsers.py`)

#### AwsEventStreamParser

Advanced AWS SSE format parser with support for:

- **Bracket counting** — correct parsing of nested JSON objects
- **Content deduplication** — filtering repeated events
- **Tool calls** — parsing structured and bracket-style tool calls
- **Escape sequences** — decoding `\n` and others

#### Event Types

| Event | Description |
|-------|-------------|
| `content` | Text content of the response |
| `tool_start` | Start of a tool call (name, toolUseId) |
| `tool_input` | Continued input for a tool call |
| `tool_stop` | End of a tool call |
| `usage` | Credit usage information |
| `context_usage` | Percentage of context used |

#### Helper Functions

| Function | Description |
|----------|-------------|
| `find_matching_brace(text, start_pos)` | Find closing brace accounting for nesting |
| `parse_bracket_tool_calls(response_text)` | Parse `[Called func with args: {...}]` style |
| `deduplicate_tool_calls(tool_calls)` | Remove duplicate tool calls |

### 3.8. Streaming (`kiro/streaming_openai.py`)

#### stream_kiro_to_openai

Async generator for converting the Kiro stream to OpenAI format.

**Functionality:**
- Parse AWS SSE stream via `AwsEventStreamParser`
- Format OpenAI `chat.completion.chunk`
- Handle tool calls (structured and bracket-style)
- Calculate usage based on `contextUsagePercentage`
- Debug logging via `debug_logger`

#### collect_stream_response

Collects the complete response from the streaming output for non-streaming mode.

### 3.9. HTTP Client (`kiro/http_client.py`)

#### KiroHttpClient

Automatic error handling with exponential backoff:

| Error Code | Action |
|------------|--------|
| `403` | Refresh token via `force_refresh()` + retry |
| `429` | Exponential backoff: `BASE_RETRY_DELAY * (2 ** attempt)` |
| `5xx` | Exponential backoff (up to `MAX_RETRIES` attempts) |
| Timeout | Exponential backoff |

**Delay formula:** `1s, 2s, 4s` (with `BASE_RETRY_DELAY=1.0`)

**Methods:**
- `request_with_retry(method, url, json_data, stream)` — request with retry
- `close()` — close the client

Supports async context manager (`async with`).

### 3.10. Routes (`kiro/routes_openai.py`)

| Endpoint | Method | Description |
|----------|--------|-------------|
| `/` | GET | Health check (status, message, version) |
| `/health` | GET | Detailed health check (status, timestamp, version) |
| `/v1/models` | GET | List available models (requires API key) |
| `/v1/chat/completions` | POST | Chat completions (requires API key) |

**Authentication:** Bearer token in `Authorization` header.

### 3.11. Exception Handling (`kiro/exceptions.py`)

| Function | Description |
|----------|-------------|
| `sanitize_validation_errors(errors)` | Convert bytes to strings for JSON serialization |
| `validation_exception_handler(request, exc)` | Handler for Pydantic validation errors (422) |

### 3.12. Debug Logger (`kiro/debug_logger.py`)

**Class:** `DebugLogger` (singleton)

**Activation:** `DEBUG_MODE=all` or `DEBUG_MODE=errors` in `.env`

**Methods:**

| Method | Description |
|--------|-------------|
| `prepare_new_request()` | Clear directory for a new request |
| `save_request_body(data)` | Save incoming request body |
| `save_kiro_request(data)` | Save request sent to Kiro API |
| `save_raw_stream(data)` | Save raw Kiro response stream |
| `save_modified_stream(data)` | Save transformed stream (OpenAI format) |
| `save_error_info(error)` | Save error details |

**Output files:**
- `request_body.json` — incoming request
- `kiro_request_body.json` — request to Kiro API
- `response_stream_raw.txt` — raw Kiro stream
- `response_stream_modified.txt` — transformed stream (OpenAI format)

### 3.13. Tokenizer (`kiro/tokenizer.py`)

**Problem:** The Kiro API does not directly return token counts. Instead, it only provides `context_usage_percentage` — the percentage of the model's context window used.

**Solution:** A tokenizer module based on `tiktoken` (OpenAI's Rust library) for fast token counting.

**Characteristics:**
- Uses `cl100k_base` encoding (GPT-4), which closely approximates Claude's tokenization
- Correction factor `CLAUDE_CORRECTION_FACTOR = 1.15` for improved accuracy
- Lazy initialization to speed up imports
- Fallback to a rough estimate if tiktoken is unavailable

**Token calculation formula:**
```
total_tokens      = context_usage_percentage × max_input_tokens  (from Kiro API)
completion_tokens = tiktoken(response)                            (our count)
prompt_tokens     = total_tokens - completion_tokens              (subtraction)
```

**Key functions:**

| Function | Description |
|----------|-------------|
| `count_tokens(text)` | Count tokens in text |
| `count_message_tokens(messages)` | Count tokens in a list of messages |
| `count_tools_tokens(tools)` | Count tokens in tool definitions |
| `estimate_request_tokens(messages, tools)` | Full token estimate for a request |

**Debug log example:**
```
[Usage] claude-opus-4-5: prompt_tokens=142211 (subtraction), completion_tokens=769 (tiktoken), total_tokens=142980 (API Kiro)
```

**Accuracy:** ~97–99.7% compared to API-reported values.

### 3.14. Kiro API Endpoints

All URLs are dynamically constructed based on the region:

- **Token Refresh:** `POST https://prod.{region}.auth.desktop.kiro.dev/refreshToken`
- **List Models:** `GET https://q.{region}.amazonaws.com/ListAvailableModels`
- **Generate Response:** `POST https://codewhisperer.{region}.amazonaws.com/generateAssistantResponse`

---

## 4. Detailed Data Flow

### 4.1. Overall Diagram (Multi-API)

```
┌─────────────────────────────────────────────────────────────────┐
│                           CLIENTS                               │
│  ┌─────────────────────┐       ┌─────────────────────┐         │
│  │   OpenAI Client     │       │  Anthropic Client   │         │
│  └──────────┬──────────┘       └──────────┬──────────┘         │
└─────────────┼──────────────────────────────┼───────────────────┘
              │                              │
              │ POST /v1/chat/completions    │ POST /v1/messages
              ▼                              ▼
┌─────────────────────────────────────────────────────────────────┐
│                         API LAYER                               │
│  ┌─────────────────────┐       ┌─────────────────────┐         │
│  │  routes_openai.py   │       │ routes_anthropic.py │         │
│  │  Security Gate      │       │ Security Gate       │         │
│  └──────────┬──────────┘       └──────────┬──────────┘         │
│             │                              │                    │
│             ▼                              ▼                    │
│  ┌─────────────────────┐       ┌─────────────────────┐         │
│  │converters_openai.py │       │converters_anthropic │         │
│  │ Extract system from │       │ System is already a │         │
│  │ messages array      │       │ separate field      │         │
│  └──────────┬──────────┘       └──────────┬──────────┘         │
└─────────────┼──────────────────────────────┼───────────────────┘
              │                              │
              └──────────────┬───────────────┘
                             │
                             ▼
┌─────────────────────────────────────────────────────────────────┐
│                         CORE LAYER                              │
│             ┌─────────────────────────────┐                     │
│             │     converters_core.py      │                     │
│             │   build_kiro_payload()      │                     │
│             │   build_kiro_history()      │                     │
│             │   process_tools()           │                     │
│             └──────────────┬──────────────┘                     │
└────────────────────────────┼────────────────────────────────────┘
                             │
                             ▼
┌─────────────────────────────────────────────────────────────────┐
│                        SHARED LAYER                             │
│  ┌─────────────────┐  ┌─────────────────┐  ┌─────────────────┐ │
│  │ KiroAuthManager │  │ KiroHttpClient  │  │  ModelInfoCache │ │
│  │   (auth.py)     │  │(http_client.py) │  │   (cache.py)    │ │
│  └────────┬────────┘  └────────┬────────┘  └─────────────────┘ │
└───────────┼────────────────────┼────────────────────────────────┘
            │                    │
            │                    │ POST /generateAssistantResponse
            │                    ▼
            │         ┌─────────────────────────────────────────┐
            │         │               Kiro API                  │
            │         └──────────────────┬──────────────────────┘
            │                            │
            │                            │ AWS SSE Stream
            │                            ▼
┌───────────┼────────────────────────────────────────────────────┐
│           │              CORE LAYER                            │
│           │  ┌─────────────────────────────┐                   │
│           │  │     streaming_core.py       │                   │
│           │  │   parse_kiro_stream()       │                   │
│           │  │   → KiroEvent objects       │                   │
│           │  └──────────────┬──────────────┘                   │
└────────────────────────────┼───────────────────────────────────┘
                             │
              ┌──────────────┴───────────────┐
              │                              │
              ▼                              ▼
┌─────────────────────────────────────────────────────────────────┐
│                        OUTPUT LAYER                             │
│  ┌─────────────────────┐       ┌─────────────────────┐         │
│  │streaming_openai.py  │       │streaming_anthropic  │         │
│  │ format_openai_sse() │       │format_anthropic_sse │         │
│  │                     │       │                     │         │
│  │ data: {...}         │       │ event: type         │         │
│  │ data: [DONE]        │       │ data: {...}         │         │
│  └──────────┬──────────┘       └──────────┬──────────┘         │
└─────────────┼──────────────────────────────┼───────────────────┘
              │                              │
              ▼                              ▼
┌─────────────────────────────────────────────────────────────────┐
│                           CLIENTS                               │
│  ┌─────────────────────┐       ┌─────────────────────┐         │
│  │   OpenAI Client     │       │  Anthropic Client   │         │
│  └─────────────────────┘       └─────────────────────┘         │
└─────────────────────────────────────────────────────────────────┘
```

### 4.2. OpenAI API Flow

```
OpenAI Client
     │ POST /v1/chat/completions
     ▼
routes_openai.py ──► converters_openai.py ──► converters_core.py
     │                                              │
     │                                              ▼
     │                                        Kiro Payload
     │                                              │
     ▼                                              ▼
KiroAuthManager ──────────────────────────► KiroHttpClient
                                                   │
                                                   ▼
                                              Kiro API
                                                   │
                                                   ▼
streaming_core.py ◄─────────────────────── AWS SSE Stream
     │
     ▼
streaming_openai.py
     │
     ▼
OpenAI SSE Format ──────────────────────► OpenAI Client
```

### 4.3. Anthropic API Flow

```
Anthropic Client
     │ POST /v1/messages
     ▼
routes_anthropic.py ──► converters_anthropic.py ──► converters_core.py
     │                                                    │
     │                                                    ▼
     │                                              Kiro Payload
     │                                                    │
     ▼                                                    ▼
KiroAuthManager ──────────────────────────────────► KiroHttpClient
                                                         │
                                                         ▼
                                                    Kiro API
                                                         │
                                                         ▼
streaming_core.py ◄─────────────────────────────── AWS SSE Stream
     │
     ▼
streaming_anthropic.py
     │
     ▼
Anthropic SSE Format ──────────────────────────► Anthropic Client
```

---

## 5. Available Models

| Model | Description | Credits |
|-------|-------------|---------|
| `claude-opus-4-5` | Top-tier model | ~2.2 |
| `claude-opus-4-5-20251101` | Top-tier model (versioned) | ~2.2 |
| `claude-sonnet-4-5` | Enhanced model | ~1.3 |
| `claude-sonnet-4-5-20250929` | Enhanced model (versioned) | ~1.3 |
| `claude-sonnet-4` | Balanced model | ~1.3 |
| `claude-sonnet-4-20250514` | Balanced model (versioned) | ~1.3 |
| `claude-haiku-4-5` | Fast model | ~0.4 |
| `claude-3-7-sonnet-20250219` | Legacy model | ~1.0 |

---

## 6. Configuration

### Environment Variables (.env)

```env
# Required
REFRESH_TOKEN="your_kiro_refresh_token"
PROXY_API_KEY="your_proxy_secret"

# Optional
PROFILE_ARN="arn:aws:codewhisperer:..."
KIRO_REGION="us-east-1"
KIRO_CREDS_FILE="~/.aws/sso/cache/kiro-auth-token.json"

# Debugging
DEBUG_MODE="off"  # off/errors/all
DEBUG_DIR="debug_logs"

# Limits
TOOL_DESCRIPTION_MAX_LENGTH="10000"
```

### JSON Credentials File (optional)

```json
{
  "accessToken": "eyJ...",
  "refreshToken": "eyJ...",
  "expiresAt": "2025-01-12T23:00:00.000Z",
  "profileArn": "arn:aws:codewhisperer:us-east-1:...",
  "region": "us-east-1"
}
```

---

## 7. API Endpoints

### 7.1. General Endpoints

| Endpoint | Method | Description |
|----------|--------|-------------|
| `/` | GET | Health check |
| `/health` | GET | Detailed health check |

### 7.2. OpenAI-Compatible Endpoints

| Endpoint | Method | Description |
|----------|--------|-------------|
| `/v1/models` | GET | List available models |
| `/v1/chat/completions` | POST | Chat completions (streaming/non-streaming) |

**Authentication:** `Authorization: Bearer {PROXY_API_KEY}`

### 7.3. Anthropic-Compatible Endpoints

| Endpoint | Method | Description |
|----------|--------|-------------|
| `/v1/messages` | POST | Messages API (streaming/non-streaming) |

**Authentication:** `x-api-key: {PROXY_API_KEY}` + `anthropic-version: 2023-06-01`

### 7.4. Format Comparison

| Aspect | OpenAI | Anthropic |
|--------|--------|-----------|
| System prompt | In `messages` with `role: "system"` | Separate `system` field |
| Content | String or array | Always an array of content blocks |
| Stop reason | `finish_reason: "stop"` | `stop_reason: "end_turn"` |
| Usage | `prompt_tokens`, `completion_tokens` | `input_tokens`, `output_tokens` |
| Streaming | `data: {...}\n\n` + `data: [DONE]` | `event: type\ndata: {...}\n\n` |
| Tool format | `{type: "function", function: {...}}` | `{name: "...", input_schema: {...}}` |

---

## 8. Implementation Details

### Tool Calling

Supports OpenAI-compatible tool format:

```json
{
  "tools": [{
    "type": "function",
    "function": {
      "name": "get_weather",
      "description": "Get weather for a location",
      "parameters": {
        "type": "object",
        "properties": {
          "location": {"type": "string"}
        }
      }
    }
  }]
}
```

### Streaming

Full SSE streaming support with correct OpenAI format:

```
data: {"id":"chatcmpl-...","object":"chat.completion.chunk",...}

data: [DONE]
```

### Debugging

With `DEBUG_MODE=all` or `DEBUG_MODE=errors`, all requests and responses are logged to `debug_logs/`:
- `request_body.json` — incoming request
- `kiro_request_body.json` — request sent to Kiro API
- `response_stream_raw.txt` — raw Kiro stream
- `response_stream_modified.txt` — transformed stream

---

## 9. Extensibility

### Adding a New API Format

The modular architecture makes it easy to add support for other API formats. Thanks to the Core Layer, most of the logic is already implemented.

#### Steps to Add a New Format (e.g., Gemini)

1. **Create models** — `models_gemini.py`
   ```python
   class GeminiRequest(BaseModel):
       """Pydantic model for Gemini request."""
       contents: List[GeminiContent]
       ...
   ```

2. **Create conversion adapter** — `converters_gemini.py`
   ```python
   from kiro.converters_core import build_kiro_payload

   def gemini_to_kiro(request: GeminiRequest, ...) -> dict:
       """Converts a Gemini request into a Kiro payload."""
       system_prompt = extract_system_instruction(request)
       messages = convert_gemini_contents(request.contents)
       tools = convert_gemini_tools(request.tools)

       return build_kiro_payload(
           messages=messages,
           system_prompt=system_prompt,
           tools=tools,
           ...
       )
   ```

3. **Create streaming formatter** — `streaming_gemini.py`
   ```python
   from kiro.streaming_core import parse_kiro_stream

   async def stream_to_gemini(response, ...) -> AsyncGenerator[str, None]:
       """Formats Kiro events as Gemini SSE."""
       async for event in parse_kiro_stream(response):
           yield format_gemini_chunk(event)
   ```

4. **Create routes** — `routes_gemini.py`
   ```python
   router = APIRouter()

   @router.post("/v1beta/models/{model}:generateContent")
   async def generate_content(request: GeminiRequest):
       ...
   ```

5. **Register in main.py**
   ```python
   from kiro.routes_gemini import router as gemini_router
   app.include_router(gemini_router)
   ```

### What Is Reused Automatically

When adding a new format, the following components work out of the box:

| Component | Functionality |
|-----------|---------------|
| `auth.py` | Kiro token management |
| `http_client.py` | HTTP with retry logic |
| `cache.py` | Model cache |
| `parsers.py` | AWS SSE parsing |
| `tokenizer.py` | Token counting |
| `converters_core.py` | Building Kiro payload |
| `streaming_core.py` | Parsing Kiro stream |

---

## 10. Dependencies

Key project dependencies (from `requirements.txt`):

| Package | Purpose |
|---------|---------|
| `fastapi` | Async web framework |
| `uvicorn` | ASGI server |
| `httpx` | Async HTTP client |
| `pydantic` | Data validation and models |
| `python-dotenv` | Loading environment variables |
| `loguru` | Advanced logging |
| `tiktoken` | Fast token counting |
