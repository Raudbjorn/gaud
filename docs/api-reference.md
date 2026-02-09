# API Reference

Gaud exposes an OpenAI-compatible API, admin endpoints for user/key/budget management, and a health check endpoint.

## Base URL

```
http://127.0.0.1:8400
```

## Authentication

Most endpoints require a Bearer token in the `Authorization` header:

```
Authorization: Bearer sk-prx-YOUR_KEY_HERE
```

The health check endpoint (`GET /health`) does not require authentication.

## Endpoints Overview

| Method | Path | Auth | Description |
|---|---|---|---|
| `GET` | `/health` | None | System and provider health |
| `POST` | `/v1/chat/completions` | Bearer | Chat completion (streaming + non-streaming) |
| `GET` | `/v1/models` | Bearer | List available models |
| `POST` | `/v1/embeddings` | Bearer | Embeddings (not yet implemented) |
| `POST` | `/admin/users` | Admin | Create a user |
| `GET` | `/admin/users` | Admin | List all users |
| `DELETE` | `/admin/users/{id}` | Admin | Delete a user |
| `POST` | `/admin/users/{id}/keys` | Admin | Create an API key |
| `GET` | `/admin/users/{id}/keys` | Admin | List API keys for a user |
| `DELETE` | `/admin/keys/{id}` | Admin | Revoke an API key |
| `PUT` | `/admin/budgets/{user_id}` | Admin | Set budget for a user |
| `GET` | `/admin/budgets/{user_id}` | Admin | Get budget for a user |
| `GET` | `/admin/usage` | Admin | Query usage logs |
| `GET` | `/admin/settings` | Admin | Get all configuration settings |
| `PUT` | `/admin/settings` | Admin | Update a configuration setting |

---

## GET /health

Returns system status and per-provider health. No authentication required.

```bash
curl http://127.0.0.1:8400/health
```

**Response:**

```json
{
  "status": "ok",
  "providers": [
    {
      "provider": "claude",
      "healthy": true,
      "models": [
        "claude-sonnet-4-20250514",
        "claude-haiku-3-5-20241022",
        "claude-opus-4-20250514"
      ],
      "latency_ms": 450
    },
    {
      "provider": "copilot",
      "healthy": true,
      "models": ["gpt-4o", "gpt-4-turbo", "o1", "o3-mini"],
      "latency_ms": 200
    }
  ]
}
```

| Field | Description |
|---|---|
| `status` | Always `"ok"` |
| `providers[].provider` | Provider ID |
| `providers[].healthy` | `false` when the circuit breaker is Open |
| `providers[].models` | Models available through this provider |
| `providers[].latency_ms` | Average response latency in milliseconds (null if no requests) |

---

## POST /v1/chat/completions

OpenAI-compatible chat completion. Supports both streaming and non-streaming responses.

### Non-Streaming

```bash
curl http://127.0.0.1:8400/v1/chat/completions \
  -H "Authorization: Bearer sk-prx-YOUR_KEY" \
  -H "Content-Type: application/json" \
  -d '{
    "model": "claude-sonnet-4-20250514",
    "messages": [
      {"role": "system", "content": "You are a helpful assistant."},
      {"role": "user", "content": "What is 2+2?"}
    ],
    "temperature": 0.7,
    "max_tokens": 1024
  }'
```

**Response:**

```json
{
  "id": "msg_abc123",
  "object": "chat.completion",
  "created": 1700000000,
  "model": "claude-sonnet-4-20250514",
  "choices": [
    {
      "index": 0,
      "message": {
        "role": "assistant",
        "content": "2 + 2 = 4.",
        "tool_calls": null
      },
      "finish_reason": "stop"
    }
  ],
  "usage": {
    "prompt_tokens": 25,
    "completion_tokens": 8,
    "total_tokens": 33
  }
}
```

### Streaming

Set `"stream": true` to receive Server-Sent Events (SSE):

```bash
curl http://127.0.0.1:8400/v1/chat/completions \
  -H "Authorization: Bearer sk-prx-YOUR_KEY" \
  -H "Content-Type: application/json" \
  -d '{
    "model": "gemini-2.5-flash",
    "messages": [{"role": "user", "content": "Tell me a joke."}],
    "stream": true
  }'
```

**Response (SSE stream):**

```
data: {"id":"msg_abc","object":"chat.completion.chunk","created":1700000000,"model":"gemini-2.5-flash","choices":[{"index":0,"delta":{"role":null,"content":"Why","tool_calls":null},"finish_reason":null}],"usage":null}

data: {"id":"msg_abc","object":"chat.completion.chunk","created":1700000000,"model":"gemini-2.5-flash","choices":[{"index":0,"delta":{"role":null,"content":" did","tool_calls":null},"finish_reason":null}],"usage":null}

data: [DONE]
```

The stream ends with `data: [DONE]`.

### Request Body

| Field | Type | Required | Description |
|---|---|---|---|
| `model` | string | Yes | Model identifier (e.g., `claude-sonnet-4-20250514`, `gpt-4o`) |
| `messages` | array | Yes | Array of message objects |
| `temperature` | float | No | Sampling temperature (0.0 - 2.0) |
| `max_tokens` | integer | No | Maximum tokens to generate (default: 8192) |
| `stream` | boolean | No | Enable SSE streaming (default: false) |
| `top_p` | float | No | Nucleus sampling parameter |
| `stop` | string or array | No | Stop sequence(s) |
| `tools` | array | No | Tool/function definitions |
| `tool_choice` | string or object | No | Tool selection strategy |

### Message Object

| Field | Type | Required | Description |
|---|---|---|---|
| `role` | string | Yes | `system`, `user`, `assistant`, or `tool` |
| `content` | string or array | Yes | Text content or multipart content array |
| `name` | string | No | Participant name |
| `tool_calls` | array | No | Tool calls made by the assistant |
| `tool_call_id` | string | No | ID of the tool call this message responds to |

### Tool Calling

```bash
curl http://127.0.0.1:8400/v1/chat/completions \
  -H "Authorization: Bearer sk-prx-YOUR_KEY" \
  -H "Content-Type: application/json" \
  -d '{
    "model": "gpt-4o",
    "messages": [{"role": "user", "content": "What is the weather in London?"}],
    "tools": [
      {
        "type": "function",
        "function": {
          "name": "get_weather",
          "description": "Get current weather for a city",
          "parameters": {
            "type": "object",
            "properties": {
              "city": {"type": "string"}
            },
            "required": ["city"]
          }
        }
      }
    ]
  }'
```

---

## GET /v1/models

List all available models across all configured providers. Compatible with the OpenAI models endpoint.

```bash
curl http://127.0.0.1:8400/v1/models \
  -H "Authorization: Bearer sk-prx-YOUR_KEY"
```

**Response:**

```json
{
  "object": "list",
  "data": [
    {
      "id": "claude-sonnet-4-20250514",
      "object": "model",
      "created": 1700000000,
      "owned_by": "claude"
    },
    {
      "id": "gemini-2.5-flash",
      "object": "model",
      "created": 1700000000,
      "owned_by": "gemini"
    },
    {
      "id": "gpt-4o",
      "object": "model",
      "created": 1700000000,
      "owned_by": "copilot"
    }
  ]
}
```

---

## POST /v1/embeddings

Placeholder endpoint. Returns `501 Not Implemented`.

```bash
curl -X POST http://127.0.0.1:8400/v1/embeddings \
  -H "Authorization: Bearer sk-prx-YOUR_KEY" \
  -H "Content-Type: application/json" \
  -d '{"input": "Hello world", "model": "text-embedding-ada-002"}'
```

**Response (501):**

```json
{
  "error": {
    "message": "Embeddings are not yet supported. This feature will be available in a future release.",
    "type": "not_implemented_error",
    "code": "not_implemented"
  }
}
```

---

## POST /admin/users

Create a new user. Admin only.

```bash
curl -X POST http://127.0.0.1:8400/admin/users \
  -H "Authorization: Bearer sk-prx-YOUR_ADMIN_KEY" \
  -H "Content-Type: application/json" \
  -d '{"name": "alice", "role": "member"}'
```

**Request Body:**

| Field | Type | Required | Description |
|---|---|---|---|
| `name` | string | Yes | Username |
| `role` | string | Yes | `admin` or `member` |

**Response:**

```json
{
  "id": "usr_abc123",
  "name": "alice",
  "role": "member",
  "created_at": "2025-01-15 10:30:00"
}
```

---

## GET /admin/users

List all users. Admin only.

```bash
curl http://127.0.0.1:8400/admin/users \
  -H "Authorization: Bearer sk-prx-YOUR_ADMIN_KEY"
```

**Response:**

```json
[
  {
    "id": "usr_abc123",
    "name": "admin",
    "role": "admin",
    "created_at": "2025-01-15 10:00:00"
  },
  {
    "id": "usr_def456",
    "name": "alice",
    "role": "member",
    "created_at": "2025-01-15 10:30:00"
  }
]
```

---

## DELETE /admin/users/{id}

Delete a user by ID. Admin only.

```bash
curl -X DELETE http://127.0.0.1:8400/admin/users/usr_def456 \
  -H "Authorization: Bearer sk-prx-YOUR_ADMIN_KEY"
```

**Response:**

```json
{
  "deleted": true
}
```

---

## POST /admin/users/{id}/keys

Create a new API key for a user. Admin only. The full plaintext key is returned exactly once.

```bash
curl -X POST http://127.0.0.1:8400/admin/users/usr_def456/keys \
  -H "Authorization: Bearer sk-prx-YOUR_ADMIN_KEY" \
  -H "Content-Type: application/json" \
  -d '{"label": "development"}'
```

**Request Body:**

| Field | Type | Required | Description |
|---|---|---|---|
| `label` | string | No | Human-readable label for the key |

**Response:**

```json
{
  "id": "key_xyz789",
  "user_id": "usr_def456",
  "key_prefix": "sk-prx-aBcDeFgH...",
  "label": "development",
  "created_at": "2025-01-15 11:00:00",
  "plaintext": "sk-prx-aBcDeFgH12345678901234567890ab"
}
```

The `plaintext` field contains the full API key. Save it immediately -- it will not be shown again.

---

## GET /admin/users/{id}/keys

List all API keys for a user (without plaintext values). Admin only.

```bash
curl http://127.0.0.1:8400/admin/users/usr_def456/keys \
  -H "Authorization: Bearer sk-prx-YOUR_ADMIN_KEY"
```

**Response:**

```json
[
  {
    "id": "key_xyz789",
    "user_id": "usr_def456",
    "key_prefix": "sk-prx-aBcDeFgH...",
    "label": "development",
    "created_at": "2025-01-15 11:00:00",
    "last_used": "2025-01-16 09:15:00"
  }
]
```

---

## DELETE /admin/keys/{id}

Revoke an API key. Takes effect immediately. Admin only.

```bash
curl -X DELETE http://127.0.0.1:8400/admin/keys/key_xyz789 \
  -H "Authorization: Bearer sk-prx-YOUR_ADMIN_KEY"
```

**Response:**

```json
{
  "deleted": true
}
```

---

## PUT /admin/budgets/{user_id}

Set or update budget limits for a user. Admin only.

```bash
curl -X PUT http://127.0.0.1:8400/admin/budgets/usr_def456 \
  -H "Authorization: Bearer sk-prx-YOUR_ADMIN_KEY" \
  -H "Content-Type: application/json" \
  -d '{"monthly_limit": 100.0, "daily_limit": 10.0}'
```

**Request Body:**

| Field | Type | Required | Description |
|---|---|---|---|
| `monthly_limit` | float | No | Monthly spending limit in USD |
| `daily_limit` | float | No | Daily spending limit in USD |

**Response:**

Returns the full budget object including current spend:

```json
{
  "user_id": "usr_def456",
  "monthly_limit": 100.0,
  "daily_limit": 10.0,
  "monthly_spend": 12.50,
  "daily_spend": 3.20,
  "last_monthly_reset": "2025-01-01 00:00:00",
  "last_daily_reset": "2025-01-16 00:00:00"
}
```

---

## GET /admin/budgets/{user_id}

Get budget information for a user. Admin only.

```bash
curl http://127.0.0.1:8400/admin/budgets/usr_def456 \
  -H "Authorization: Bearer sk-prx-YOUR_ADMIN_KEY"
```

**Response:**

```json
{
  "user_id": "usr_def456",
  "monthly_limit": 100.0,
  "daily_limit": 10.0,
  "monthly_spend": 12.50,
  "daily_spend": 3.20,
  "last_monthly_reset": "2025-01-01 00:00:00",
  "last_daily_reset": "2025-01-16 00:00:00"
}
```

Returns `404` if no budget is configured for the user.

---

## GET /admin/usage

Query usage logs with filtering and pagination. Admin only.

```bash
curl "http://127.0.0.1:8400/admin/usage?user_id=usr_def456&page=1&per_page=20" \
  -H "Authorization: Bearer sk-prx-YOUR_ADMIN_KEY"
```

**Query Parameters:**

| Parameter | Type | Default | Description |
|---|---|---|---|
| `user_id` | string | (all) | Filter by user ID |
| `provider` | string | (all) | Filter by provider (e.g., `claude`) |
| `from` | string | (none) | Start date (ISO 8601) |
| `to` | string | (none) | End date (ISO 8601) |
| `page` | integer | 1 | Page number |
| `per_page` | integer | 50 | Results per page (max 500) |

**Response:**

```json
{
  "data": [
    {
      "id": "log_abc123",
      "user_id": "usr_def456",
      "request_id": "req_xyz789",
      "provider": "claude",
      "model": "claude-sonnet-4-20250514",
      "input_tokens": 150,
      "output_tokens": 200,
      "cost": 0.0035,
      "latency_ms": 1200,
      "status": "success",
      "created_at": "2025-01-16 09:15:00"
    }
  ],
  "page": 1,
  "per_page": 20,
  "total": 142
}
```

---

## GET /admin/settings

Get all configuration settings with their current values and env var override status. Admin only.

```bash
curl http://127.0.0.1:8400/admin/settings \
  -H "Authorization: Bearer sk-prx-YOUR_ADMIN_KEY"
```

**Response:**

```json
[
  {
    "key": "server.host",
    "value": "127.0.0.1",
    "env_var": "GAUD_SERVER_HOST",
    "overridden": false
  },
  {
    "key": "server.port",
    "value": 8400,
    "env_var": "GAUD_SERVER_PORT",
    "overridden": true
  }
]
```

| Field | Description |
|---|---|
| `key` | TOML setting path |
| `value` | Current effective value |
| `env_var` | The environment variable that controls this setting |
| `overridden` | `true` if the value comes from an env var (read-only) |

---

## PUT /admin/settings

Update a configuration setting. Admin only. Settings overridden by environment variables cannot be changed.

```bash
curl -X PUT http://127.0.0.1:8400/admin/settings \
  -H "Authorization: Bearer sk-prx-YOUR_ADMIN_KEY" \
  -H "Content-Type: application/json" \
  -d '{"key": "logging.level", "value": "debug"}'
```

**Request Body:**

| Field | Type | Required | Description |
|---|---|---|---|
| `key` | string | Yes | Setting path (e.g., `server.host`, `auth.enabled`) |
| `value` | any | Yes | New value (string, number, or boolean) |

**Response:**

```json
{
  "message": "Setting saved. Restart the server to apply changes.",
  "key": "logging.level"
}
```

**Error (env var override):**

```json
{
  "error": {
    "message": "Setting 'server.port' is overridden by environment variable 'GAUD_SERVER_PORT'. Unset the variable and restart to edit.",
    "type": "bad_request",
    "code": "bad_request"
  }
}
```

---

## Error Responses

All errors follow the OpenAI error format:

```json
{
  "error": {
    "message": "Authentication required: Missing Authorization header",
    "type": "authentication_error",
    "code": "invalid_api_key"
  }
}
```

### Error Codes

| HTTP Status | Type | When |
|---|---|---|
| 400 | `bad_request` | Invalid request body or parameters |
| 401 | `authentication_error` | Missing or invalid API key |
| 403 | `permission_error` | Member attempting admin action |
| 404 | `not_found` | Resource does not exist |
| 429 | `rate_limit_error` | Budget exceeded |
| 500 | `internal_error` | Server error |
| 501 | `not_implemented_error` | Feature not yet available (e.g., embeddings) |

### Budget Warning Header

When a request succeeds but the user is approaching their budget limit, Gaud includes:

```
X-Budget-Warning: Monthly budget is 85% consumed
```

This header is added when usage exceeds the configured `warning_threshold_percent` (default: 80%).
