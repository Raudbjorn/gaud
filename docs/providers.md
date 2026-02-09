# Providers Guide

Gaud routes requests to LLM providers based on the model name in the request. Each provider is connected via OAuth and accessed through its native API. Gaud translates between the OpenAI request/response format and each provider's format automatically.

## Supported Providers

| Provider | ID | OAuth Flow | API | Supported Models |
|---|---|---|---|---|
| Claude (Anthropic) | `claude` | PKCE Authorization Code | Anthropic Messages API | `claude-sonnet-4-20250514`, `claude-haiku-3-5-20241022`, `claude-opus-4-20250514` |
| Gemini (Google) | `gemini` | PKCE + Client Secret | Google Generative AI | `gemini-2.5-flash`, `gemini-2.5-pro`, `gemini-2.0-flash` |
| GitHub Copilot | `copilot` | Device Code (RFC 8628) | GitHub Copilot Chat API | `gpt-4o`, `gpt-4-turbo`, `o1`, `o3-mini` |
| Kiro (AWS) | `kiro` | Kiro Gateway (refresh token) | Amazon Q / CodeWhisperer | `kiro:auto`, `kiro:claude-sonnet-4`, `kiro:claude-sonnet-4.5`, `kiro:claude-haiku-4.5`, `kiro:claude-opus-4.5`, `kiro:claude-3.7-sonnet` |

## Model Name Routing

All requests go through a single endpoint (`POST /v1/chat/completions`). Gaud uses the `model` field in the request to determine which provider handles it via prefix matching:

| Model Prefix | Provider | Example |
|---|---|---|
| `kiro:*` | Kiro | `kiro:claude-sonnet-4`, `kiro:auto` |
| `claude-*` | Claude | `claude-sonnet-4-20250514` |
| `gemini-*` | Gemini | `gemini-2.5-flash` |
| `gpt-*`, `o1*`, `o3*` | Copilot | `gpt-4o`, `o3-mini` |

If no prefix matches, Gaud falls back to any registered provider whose `supports_model()` method returns true.

### Model Overlap and Disambiguation

Several providers may offer access to the same underlying model family (e.g., Claude models are available through both the direct Anthropic API and through Kiro/AWS). Gaud handles this through **namespace prefixes**:

- **Direct access** uses the provider's native model IDs: `claude-sonnet-4-20250514`
- **Kiro-routed access** uses the `kiro:` prefix: `kiro:claude-sonnet-4`

These are treated as **distinct model identifiers** by the router. A request for `claude-sonnet-4-20250514` always routes to the `claude` provider, while `kiro:claude-sonnet-4` always routes to the `kiro` provider. There is no ambiguity.

When multiple providers _do_ register the same model ID (e.g., a backup provider), the router builds a candidate list:

1. **Primary provider** (from prefix matching) is tried first
2. **Fallback providers** (any other provider whose `supports_model()` returns true) are tried in order
3. The candidate list is then reordered according to the active [routing strategy](#routing-strategies)
4. For non-streaming requests, providers are tried in order until one succeeds
5. For streaming requests, only the first candidate is used (streams cannot be spliced mid-response)

The `GET /v1/models` endpoint lists all available models with their `owned_by` field indicating which provider serves each one, making it easy for clients to discover and select the right model identifier.

## Claude (Anthropic)

### Setup

1. Create an OAuth application at [console.anthropic.com](https://console.anthropic.com)
2. Note your Client ID
3. Configure in `llm-proxy.toml`:

```toml
[providers.claude]
client_id = "YOUR_ANTHROPIC_CLIENT_ID"
# auth_url = "https://console.anthropic.com/oauth/authorize"  # default
# callback_port = 19284                                        # default
```

### OAuth Flow

Claude uses PKCE (Proof Key for Code Exchange) without a client secret:

1. Gaud generates a PKCE code verifier and challenge (S256)
2. User is directed to Anthropic's authorization URL with `code=true`
3. After authorization, Anthropic redirects to `http://localhost:{callback_port}/oauth/callback/claude`
4. Gaud exchanges the authorization code for access and refresh tokens
5. Tokens are stored in the configured storage backend

**Scopes:** `org:create_api_key`, `user:profile`, `user:inference`

**Token format:** JSON-encoded requests to the token endpoint (not form-encoded).

### API Translation

Gaud converts OpenAI-format requests to the Anthropic Messages API:

- `system` messages are extracted into the Anthropic `system` parameter
- `max_tokens` defaults to 8,192 if not specified
- `stop` sequences are mapped to `stop_sequences`
- Tool calls are converted between OpenAI and Anthropic formats
- Streaming uses Anthropic's SSE event types (`message_start`, `content_block_delta`, `message_delta`, etc.)

## Gemini (Google)

### Setup

1. Create an OAuth application in the [Google Cloud Console](https://console.cloud.google.com)
2. Note your Client ID and Client Secret
3. Configure in `llm-proxy.toml`:

```toml
[providers.gemini]
client_id = "YOUR_GOOGLE_CLIENT_ID"
client_secret = "YOUR_GOOGLE_CLIENT_SECRET"
# auth_url = "https://accounts.google.com/o/oauth2/v2/auth"    # default
# token_url = "https://oauth2.googleapis.com/token"             # default
# callback_port = 19285                                          # default
```

### OAuth Flow

Gemini uses PKCE authorization code flow with a client secret:

1. Gaud generates a PKCE code verifier and challenge (S256)
2. User is directed to Google's authorization URL with `access_type=offline` and `prompt=consent`
3. After authorization, Google redirects to `http://localhost:{callback_port}/oauth/callback/gemini`
4. Gaud exchanges the code using form-encoded requests (with both PKCE verifier and client secret)
5. Tokens are stored in the configured storage backend

**Scope:** `https://www.googleapis.com/auth/generative-language`

**Note:** The `access_type=offline` and `prompt=consent` parameters ensure Google returns a refresh token.

### API Translation

Gaud converts OpenAI-format requests to the Google Generative AI format:

- `system` messages are mapped to `systemInstruction`
- Assistant messages use `role: "model"` instead of `role: "assistant"`
- `max_tokens` maps to `maxOutputTokens` in `generationConfig` (default: 8,192)
- Tool calls use `functionCall`/`functionResponse` parts
- Image data URIs are converted to `inlineData` format
- Streaming uses the `streamGenerateContent?alt=sse` endpoint

## GitHub Copilot

### Setup

1. A GitHub Copilot subscription is required (Individual, Business, or Enterprise)
2. Configuration in `llm-proxy.toml`:

```toml
[providers.copilot]
# client_id = "Iv1.b507a08c87ecfe98"  # Default GitHub Copilot client ID
```

The default `client_id` is the official GitHub Copilot client ID. Override it only if you have a custom GitHub OAuth application.

### OAuth Flow

Copilot uses the Device Code flow (RFC 8628), which works without a browser redirect:

1. Gaud requests a device code from `https://github.com/login/device/code`
2. The user is shown a `user_code` and `verification_uri`
3. The user visits `https://github.com/login/device` and enters the code
4. Gaud polls `https://github.com/login/oauth/access_token` until authorized
5. The polling respects the server-specified interval and backs off on `slow_down` responses
6. The resulting token is stored in the configured storage backend

**Note:** Copilot tokens are long-lived and do not use refresh tokens. Re-authentication is done via a new device code flow.

### API Translation

The GitHub Copilot Chat API is nearly OpenAI-compatible. Minimal conversion is needed:

- Request format is passed through with minor adjustments
- Copilot-specific headers are added: `editor-version: gaud/0.1.0`, `copilot-integration-id: gaud`
- Streaming uses standard SSE with `data: [DONE]` sentinel
- Tool calls pass through natively

### Pricing

Models accessed through Copilot are subscription-based with no per-token charges:

| Model | Input $/M tokens | Output $/M tokens |
|---|---|---|
| gpt-4o | $0.00 | $0.00 |
| gpt-4-turbo | $0.00 | $0.00 |
| o1 | $0.00 | $0.00 |
| o3-mini | $0.00 | $0.00 |

## Kiro (AWS)

### Setup

Kiro connects through the [kiro-gateway](https://github.com/anthropics/kiro-gateway) client using an AWS refresh token. Authentication is managed internally by the gateway client.

Configure in `llm-proxy.toml`:

```toml
[providers.kiro]
# Option 1: Path to Kiro credentials JSON file
credentials_file = "~/.kiro/credentials.json"

# Option 2: Direct refresh token (overrides credentials_file)
# refresh_token = "YOUR_KIRO_REFRESH_TOKEN"

# AWS region (default: us-east-1)
# region = "us-east-1"
```

The most convenient method is the environment variable:

```bash
export GAUD_KIRO_REFRESH_TOKEN="your-refresh-token"
```

### Authentication

Kiro uses an AWS-based refresh token flow managed by the kiro-gateway client library. No browser-based OAuth flow is needed. The client automatically handles token refresh.

Credential sources are checked in this order:
1. `refresh_token` in config (or `GAUD_KIRO_REFRESH_TOKEN` env var)
2. `credentials_file` path
3. `KIRO_REFRESH_TOKEN` env var (kiro-gateway native)

### API Translation

Kiro's API is similar to Anthropic's but routed through AWS infrastructure:

- The `kiro:` prefix is stripped before sending to the Kiro API (e.g., `kiro:claude-sonnet-4` becomes `claude-sonnet-4`)
- `kiro:auto` lets the Kiro gateway select the best model automatically
- Request/response format follows the Anthropic Messages API pattern
- Streaming uses SSE with Anthropic-style event types

### Pricing

Models accessed through Kiro are billed through your AWS account. Pricing varies by region and agreement.

## Routing Strategies

Configure how Gaud selects among multiple providers that support the same model:

```toml
[providers]
routing_strategy = "priority"  # priority | round_robin | least_used | random
```

| Strategy | Behavior |
|---|---|
| `priority` | Use providers in registration order. First healthy provider wins. This is the default. |
| `round_robin` | Cycle through providers in rotation. |
| `least_used` | Pick the provider with the fewest total requests. |
| `random` | Pick a random healthy provider (Fisher-Yates shuffle). |

### Automatic Fallback

When a provider fails (for non-streaming requests), Gaud automatically tries the next candidate provider in the strategy order. Streaming requests do not fall back because a partially delivered stream cannot be seamlessly spliced.

## Token Storage Backends

Configure where OAuth tokens are persisted:

```toml
[providers]
storage_backend = "file"                         # file | keyring | memory
token_storage_dir = "~/.local/share/gaud/tokens" # Used by "file" backend
```

| Backend | Description | Persistence |
|---|---|---|
| `file` | JSON files in `token_storage_dir` (one per provider). Default. | Survives restarts |
| `keyring` | System keyring (requires the `system-keyring` feature). | Survives restarts |
| `memory` | In-memory only. Tokens are lost on restart. | None |

Token files are stored as `{provider}.json` in the token storage directory (e.g., `~/.local/share/gaud/tokens/claude.json`).

## Circuit Breaker Health Monitoring

Each registered provider has an independent circuit breaker that tracks failures and prevents cascading failures.

### States

```
Closed (normal) --[3 consecutive failures]--> Open (reject all)
Open --[30s timeout expires]--> HalfOpen (allow probe)
HalfOpen --[2 consecutive successes]--> Closed
HalfOpen --[any failure]--> Open
```

| State | Description | Requests Allowed |
|---|---|---|
| `Closed` | Normal operation. | All |
| `Open` | Provider is failing. No requests are sent until the timeout expires. | None |
| `HalfOpen` | Testing recovery. A limited number of probe requests are allowed. | Probe only |

### Default Thresholds

| Parameter | Value |
|---|---|
| `failure_threshold` | 3 consecutive failures to trip Open |
| `success_threshold` | 2 consecutive successes in HalfOpen to return to Closed |
| `timeout_duration` | 30 seconds in Open before transitioning to HalfOpen |

### Health Check Endpoint

The `GET /health` endpoint reports per-provider circuit breaker state:

```json
{
  "status": "ok",
  "providers": [
    {
      "provider": "claude",
      "healthy": true,
      "models": ["claude-sonnet-4-20250514", "claude-haiku-3-5-20241022", "claude-opus-4-20250514"],
      "latency_ms": 450
    },
    {
      "provider": "gemini",
      "healthy": true,
      "models": ["gemini-2.5-flash", "gemini-2.5-pro", "gemini-2.0-flash"],
      "latency_ms": 320
    },
    {
      "provider": "copilot",
      "healthy": false,
      "models": ["gpt-4o", "gpt-4-turbo", "o1", "o3-mini"],
      "latency_ms": null
    }
  ]
}
```

A provider is reported as `healthy: false` when its circuit breaker is in the `Open` state. The `latency_ms` field shows the average response latency across successful requests.

## Model Pricing

Gaud includes an embedded pricing database for cost calculation. Costs are tracked per request in the usage log.

### Claude Models

| Model | Input $/M tokens | Output $/M tokens | Context Window | Max Output |
|---|---|---|---|---|
| claude-opus-4-20250514 | $15.00 | $75.00 | 200K | 32K |
| claude-sonnet-4-20250514 | $3.00 | $15.00 | 200K | 64K |
| claude-haiku-3-5-20241022 | $0.80 | $4.00 | 200K | 8K |

### Gemini Models

| Model | Input $/M tokens | Output $/M tokens | Context Window | Max Output |
|---|---|---|---|---|
| gemini-2.5-pro | $1.25 | $10.00 | 1M | 65K |
| gemini-2.5-flash | $0.15 | $0.60 | 1M | 65K |
| gemini-2.0-flash | $0.10 | $0.40 | 1M | 8K |

## Disabling a Provider

Omit the provider section entirely from the TOML config to disable it:

```toml
# Only Claude and Copilot are enabled; Gemini is disabled.
[providers.claude]
client_id = "your-anthropic-client-id"

[providers.copilot]
# Uses default client ID
```
