# LiteLLM Integration Guide

This guide explains how to integrate [LiteLLM](https://docs.litellm.ai/) -- an open-source LLM proxy supporting 100+ providers -- as a backend provider in Gaud. LiteLLM acts as a unified gateway that translates between Gaud's OpenAI-compatible API and dozens of upstream LLM providers, each with their own API formats.

## Architecture

```
                         Gaud (port 8400)
                              |
         +--------------------+--------------------+
         |                    |                    |
    Direct Providers     LiteLLM Proxy       Direct Providers
    (Claude, Gemini,     (port 4000)         (Kiro/AWS)
     Copilot via OAuth)       |
                    +---------+---------+
                    |         |         |
                 OpenAI   Anthropic   Bedrock
                 Groq     Mistral     Cohere
                 Azure    Replicate   Together
                 ...100+ providers...
```

**Key idea**: Gaud connects to LiteLLM as a single upstream provider via its OpenAI-compatible API. LiteLLM handles the per-provider authentication, format translation, load balancing, and retry logic internally. Models are namespaced with a `litellm:` prefix (e.g. `litellm:gpt-4o`) to avoid collisions with Gaud's direct providers.

## Quick Start

### 1. Start LiteLLM

**Docker Compose** (recommended):

```bash
cd litellm/
docker compose up -d
```

This starts LiteLLM on port 4000, PostgreSQL on 5432, and Prometheus on 9090.

**Direct install**:

```bash
pip install litellm[proxy]
litellm --config proxy_server_config.yaml --port 4000
```

### 2. Configure Gaud

Add to your `llm-proxy.toml`:

```toml
[providers.litellm]
url = "http://localhost:4000"
api_key = "sk-1234"          # LiteLLM master key
discover_models = true         # Auto-fetch available models at startup
timeout_secs = 120             # Request timeout
```

Or use environment variables:

```bash
export GAUD_LITELLM_URL=http://localhost:4000
export GAUD_LITELLM_API_KEY=sk-1234
```

Setting `GAUD_LITELLM_URL` automatically creates the LiteLLM provider config if none exists in the TOML file.

### 3. Send Requests

```bash
# Use the litellm: prefix to route through LiteLLM
curl http://localhost:8400/v1/chat/completions \
  -H "Authorization: Bearer sk-prx-YOUR_GAUD_KEY" \
  -H "Content-Type: application/json" \
  -d '{
    "model": "litellm:gpt-4o",
    "messages": [{"role": "user", "content": "Hello!"}]
  }'
```

The `litellm:` prefix is stripped before forwarding to LiteLLM, which receives `gpt-4o` and routes it to the appropriate upstream provider.

## Web UI Configuration

LiteLLM settings are fully configurable from the Gaud dashboard at `/ui/settings`. The settings appear under the **LiteLLM** section.

### Available Settings

| Setting | Key | Description | Default |
|---|---|---|---|
| LiteLLM URL | `providers.litellm.url` | Base URL of the LiteLLM proxy | (none) |
| API Key | `providers.litellm.api_key` | Master key or virtual key | (none) |
| Discover Models | `providers.litellm.discover_models` | Auto-fetch models at startup | `true` |
| Timeout | `providers.litellm.timeout_secs` | Request timeout in seconds | `120` |

### Setup via Web UI

1. Navigate to **Settings** (`/ui/settings`)
2. Scroll to the **LiteLLM** section
3. Enter the LiteLLM proxy URL (e.g. `http://localhost:4000`)
4. Optionally enter the API key (shown as `********` when set)
5. Toggle model discovery on/off
6. Adjust timeout if needed
7. **Restart Gaud** for changes to take effect (provider registration happens at startup)

### Settings API

Settings can also be updated programmatically:

```bash
# Set the LiteLLM URL
curl -X PUT http://localhost:8400/admin/settings \
  -H "Authorization: Bearer sk-prx-YOUR_ADMIN_KEY" \
  -H "Content-Type: application/json" \
  -d '{"key": "providers.litellm.url", "value": "http://litellm:4000"}'

# Set the API key
curl -X PUT http://localhost:8400/admin/settings \
  -H "Authorization: Bearer sk-prx-YOUR_ADMIN_KEY" \
  -H "Content-Type: application/json" \
  -d '{"key": "providers.litellm.api_key", "value": "sk-1234"}'

# Enable model discovery
curl -X PUT http://localhost:8400/admin/settings \
  -H "Authorization: Bearer sk-prx-YOUR_ADMIN_KEY" \
  -H "Content-Type: application/json" \
  -d '{"key": "providers.litellm.discover_models", "value": "true"}'
```

Clearing the URL field removes the LiteLLM provider configuration entirely. The API key field preserves its value when `********` is submitted (masked value).

### Environment Variable Overrides

When a setting is overridden by an environment variable, the web UI shows it as disabled with the env var name displayed:

| Env Var | Overrides |
|---|---|
| `GAUD_LITELLM_URL` | `providers.litellm.url` |
| `GAUD_LITELLM_API_KEY` | `providers.litellm.api_key` |
| `GAUD_LITELLM_DISCOVER` | `providers.litellm.discover_models` |
| `GAUD_LITELLM_TIMEOUT` | `providers.litellm.timeout_secs` |

## Model Routing

### Namespace Prefixes

Gaud uses namespace prefixes to route requests to the correct provider:

| Prefix | Provider | Example |
|---|---|---|
| `litellm:*` | LiteLLM Proxy | `litellm:gpt-4o`, `litellm:anthropic/claude-sonnet-4` |
| `kiro:*` | Kiro (Amazon Q) | `kiro:claude-sonnet-4` |
| `claude-*` | Claude (direct) | `claude-sonnet-4` |
| `gemini-*` | Gemini (direct) | `gemini-2.5-pro` |
| `gpt-*`, `o1*`, `o3*` | Copilot (direct) | `gpt-4o`, `o1-preview` |

The `litellm:` prefix is always stripped before the request reaches LiteLLM. This means:
- `litellm:gpt-4o` → LiteLLM receives `gpt-4o`
- `litellm:anthropic/claude-3-haiku` → LiteLLM receives `anthropic/claude-3-haiku`
- `litellm:bedrock/us.anthropic.claude-3-5-sonnet-20241022-v2:0` → LiteLLM receives the full bedrock path

### Auto-Discovery

When `discover_models` is `true` (the default), Gaud fetches the model list from LiteLLM's `GET /v1/models` endpoint at startup. Discovered models are available with the `litellm:` prefix. If discovery fails (e.g. LiteLLM is not running yet), only manually listed models are available, and a warning is logged.

### Model Overlap with Direct Providers

When the same model is available through both LiteLLM and a direct provider, the prefix determines which path is used:

```bash
# Routes through LiteLLM → OpenAI
curl -d '{"model": "litellm:gpt-4o", ...}'

# Routes through Copilot provider directly (OAuth)
curl -d '{"model": "gpt-4o", ...}'
```

This is intentional -- it lets you compare latency, cost, or availability between routing paths.

### Manual Model List

If auto-discovery is disabled or you want to pre-register models, list them in the config:

```toml
[providers.litellm]
url = "http://localhost:4000"
discover_models = false
models = [
  "litellm:gpt-4o",
  "litellm:gpt-4o-mini",
  "litellm:claude-sonnet-4",
  "litellm:anthropic/claude-3-haiku",
]
```

## LiteLLM Configuration Reference

### proxy_server_config.yaml

LiteLLM's own configuration is a YAML file (`proxy_server_config.yaml`). Here are the key sections:

#### Model List

```yaml
model_list:
  - model_name: gpt-4o
    litellm_params:
      model: openai/gpt-4o
      api_key: os.environ/OPENAI_API_KEY
      rpm: 480          # Rate limit (requests per minute)
      timeout: 300      # Timeout in seconds
      stream_timeout: 60

  - model_name: claude-sonnet
    litellm_params:
      model: anthropic/claude-sonnet-4
      api_key: os.environ/ANTHROPIC_API_KEY
```

The `os.environ/` prefix tells LiteLLM to read the value from environment variables.

#### Provider Format Reference

| Provider | model format | Auth |
|---|---|---|
| OpenAI | `openai/gpt-4o` | `OPENAI_API_KEY` |
| Anthropic | `anthropic/claude-sonnet-4` | `ANTHROPIC_API_KEY` |
| Azure OpenAI | `azure/<deployment-name>` | `AZURE_API_KEY` + `AZURE_API_BASE` |
| AWS Bedrock | `bedrock/<model-id>` | AWS credentials |
| Google Vertex | `vertex_ai/<model-id>` | GCP credentials |
| Google AI Studio | `gemini/<model-name>` | `GOOGLE_API_KEY` |
| Groq | `groq/<model-name>` | `GROQ_API_KEY` |
| Mistral | `mistral/<model-name>` | `MISTRAL_API_KEY` |
| Cohere | `cohere/<model-name>` | `COHERE_API_KEY` |
| Together | `together_ai/<model-name>` | `TOGETHER_API_KEY` |
| Replicate | `replicate/<model-id>` | `REPLICATE_API_TOKEN` |
| Ollama | `ollama/<model-name>` | (none, local) |
| HuggingFace | `huggingface/<model-id>` | `HF_TOKEN` |
| SageMaker | `sagemaker/<endpoint>` | AWS credentials |
| Fireworks | `fireworks_ai/<model-name>` | `FIREWORKS_API_KEY` |
| Deepseek | `deepseek/<model-name>` | `DEEPSEEK_API_KEY` |
| Perplexity | `perplexity/<model-name>` | `PERPLEXITY_API_KEY` |
| OpenRouter | `openrouter/<model-name>` | `OPENROUTER_API_KEY` |

#### Wildcard Routing

LiteLLM supports wildcard routing to forward any model from a provider:

```yaml
model_list:
  # Route any anthropic/* model
  - model_name: "anthropic/*"
    litellm_params:
      model: "anthropic/*"
      api_key: os.environ/ANTHROPIC_API_KEY

  # Route any bedrock/* model
  - model_name: "bedrock/*"
    litellm_params:
      model: "bedrock/*"

  # Catch-all: route any unknown model to OpenAI
  - model_name: "*"
    litellm_params:
      model: "openai/*"
      api_key: os.environ/OPENAI_API_KEY
```

With wildcard routing, you can send `litellm:anthropic/claude-3-haiku` through Gaud and LiteLLM will handle routing it to the Anthropic API.

#### Model Aliases and Load Balancing

```yaml
model_list:
  # Two deployments of the same model for load balancing
  - model_name: gpt-4o
    litellm_params:
      model: openai/gpt-4o
      api_key: os.environ/OPENAI_API_KEY_1
  - model_name: gpt-4o
    litellm_params:
      model: azure/gpt-4o-deployment
      api_key: os.environ/AZURE_API_KEY

router_settings:
  routing_strategy: usage-based-routing-v2
  # Options: simple-shuffle, least-busy, usage-based-routing-v2,
  #          latency-based-routing
```

#### Settings

```yaml
litellm_settings:
  drop_params: true          # Drop unsupported params instead of erroring
  num_retries: 5             # Retry failed requests
  request_timeout: 600       # Global timeout (seconds)
  telemetry: false           # Disable telemetry
  success_callback: ["prometheus"]  # Observability

general_settings:
  master_key: sk-1234        # Auth key for the proxy
  store_model_in_db: true    # Allow adding models via LiteLLM UI
```

### Dynamic Model Management

LiteLLM supports adding/removing models at runtime via its API:

```bash
# Add a new model
curl -X POST http://localhost:4000/model/new \
  -H "Authorization: Bearer sk-1234" \
  -H "Content-Type: application/json" \
  -d '{
    "model_name": "deepseek-chat",
    "litellm_params": {
      "model": "deepseek/deepseek-chat",
      "api_key": "sk-deepseek-..."
    }
  }'

# List models
curl http://localhost:4000/v1/models \
  -H "Authorization: Bearer sk-1234"

# Delete a model
curl -X POST http://localhost:4000/model/delete \
  -H "Authorization: Bearer sk-1234" \
  -H "Content-Type: application/json" \
  -d '{"id": "model-id-here"}'
```

After adding models to LiteLLM, restart Gaud (or wait for the next model discovery cycle) to make them available through the `litellm:` prefix.

### Virtual Keys and Budgets

LiteLLM has its own virtual key and budget system:

```bash
# Create a virtual key with a budget
curl -X POST http://localhost:4000/key/generate \
  -H "Authorization: Bearer sk-1234" \
  -H "Content-Type: application/json" \
  -d '{
    "max_budget": 50.0,
    "budget_duration": "30d",
    "models": ["gpt-4o", "claude-sonnet-4"]
  }'
```

Note: Gaud has its own user/budget system that operates independently. When both are active:
- **Gaud budgets** control spend per Gaud user across all providers
- **LiteLLM budgets** control spend per LiteLLM virtual key across LiteLLM-routed models

## Health Monitoring

### Health Checks

Gaud checks LiteLLM's health via `GET /health/liveliness` with a 5-second timeout. This is used by:
- The provider status display in the dashboard
- The circuit breaker that protects against cascading failures

### Circuit Breaker

Gaud's circuit breaker monitors LiteLLM request failures:

| State | Behavior |
|---|---|
| **Closed** | Requests flow normally |
| **Open** (3 consecutive failures) | Requests immediately fail with 503 for 30 seconds |
| **Half-Open** (after 30s) | One test request is allowed through |
| **Closed** (2 successes in half-open) | Normal operation resumes |

The circuit breaker treats HTTP 5xx responses and timeouts as failures. HTTP 4xx responses (bad request, auth error) do not trip the breaker.

### Monitoring Endpoints

```bash
# Gaud health (includes provider status)
curl http://localhost:8400/health

# LiteLLM health (direct)
curl http://localhost:4000/health/liveliness

# LiteLLM model health
curl http://localhost:4000/health \
  -H "Authorization: Bearer sk-1234"
```

## Docker Compose Deployment

Complete example for running Gaud + LiteLLM + PostgreSQL together:

```yaml
# docker-compose.yml
services:
  gaud:
    image: gaud:latest
    ports:
      - "8400:8400"
    environment:
      GAUD_LITELLM_URL: "http://litellm:4000"
      GAUD_LITELLM_API_KEY: "sk-1234"
    depends_on:
      litellm:
        condition: service_healthy
    volumes:
      - ./llm-proxy.toml:/app/llm-proxy.toml
      - gaud_data:/app/data

  litellm:
    image: docker.litellm.ai/berriai/litellm:main-stable
    ports:
      - "4000:4000"
    environment:
      DATABASE_URL: "postgresql://llmproxy:dbpassword9090@db:5432/litellm"
      STORE_MODEL_IN_DB: "True"
      OPENAI_API_KEY: "${OPENAI_API_KEY}"
      ANTHROPIC_API_KEY: "${ANTHROPIC_API_KEY}"
    volumes:
      - ./proxy_server_config.yaml:/app/config.yaml
    command: ["--config=/app/config.yaml"]
    depends_on:
      db:
        condition: service_healthy
    healthcheck:
      test: ["CMD-SHELL", "python3 -c \"import urllib.request; urllib.request.urlopen('http://localhost:4000/health/liveliness')\""]
      interval: 30s
      timeout: 10s
      retries: 3
      start_period: 40s

  db:
    image: postgres:16
    environment:
      POSTGRES_DB: litellm
      POSTGRES_USER: llmproxy
      POSTGRES_PASSWORD: dbpassword9090
    volumes:
      - postgres_data:/var/lib/postgresql/data
    healthcheck:
      test: ["CMD-SHELL", "pg_isready -d litellm -U llmproxy"]
      interval: 1s
      timeout: 5s
      retries: 10

  prometheus:
    image: prom/prometheus
    ports:
      - "9090:9090"
    volumes:
      - prometheus_data:/prometheus
      - ./prometheus.yml:/etc/prometheus/prometheus.yml

volumes:
  gaud_data:
  postgres_data:
  prometheus_data:
```

## Observability

LiteLLM supports several observability backends:

```yaml
litellm_settings:
  success_callback: ["prometheus"]    # Prometheus metrics
  # success_callback: ["langfuse"]    # Langfuse tracing
  # success_callback: ["lunary"]      # Lunary analytics
```

With Prometheus configured, LiteLLM exposes metrics at `GET /metrics`:

- `litellm_requests_total` -- Total requests by model, status
- `litellm_request_duration_seconds` -- Request latency histogram
- `litellm_tokens_total` -- Token usage by model, direction (input/output)
- `litellm_spend_total` -- Cost tracking by model

### Per-Team Observability

```yaml
litellm_settings:
  default_team_settings:
    - team_id: team-1
      success_callback: ["langfuse"]
      langfuse_public_key: os.environ/LANGFUSE_PROJECT1_PUBLIC
      langfuse_secret: os.environ/LANGFUSE_PROJECT1_SECRET
    - team_id: team-2
      success_callback: ["langfuse"]
      langfuse_public_key: os.environ/LANGFUSE_PROJECT2_PUBLIC
      langfuse_secret: os.environ/LANGFUSE_PROJECT2_SECRET
```

## Troubleshooting

### LiteLLM not reachable

**Symptom**: Gaud logs `LiteLLM provider init failed` or health checks fail.

**Fix**: Verify LiteLLM is running and the URL is correct:
```bash
curl http://localhost:4000/health/liveliness
# Should return: "I'm alive!"
```

If using Docker, ensure the services are on the same network and use the container name (`http://litellm:4000`), not `localhost`.

### Model discovery returns empty list

**Symptom**: No `litellm:*` models appear in Gaud's model list.

**Fix**: Check that LiteLLM has models configured:
```bash
curl http://localhost:4000/v1/models -H "Authorization: Bearer sk-1234"
```

If using `store_model_in_db: True`, models may need to be added via the LiteLLM UI or API before they appear. Also ensure the API key has the `model:read` permission.

### Authentication errors (401/403)

**Symptom**: Requests to LiteLLM return 401 or 403.

**Fix**: Verify the API key matches LiteLLM's `master_key` in `general_settings`:
```yaml
general_settings:
  master_key: sk-1234
```

Ensure the same key is set in Gaud's config:
```toml
[providers.litellm]
api_key = "sk-1234"
```

### Streaming responses hang or timeout

**Symptom**: Streaming requests start but never complete.

**Fix**:
1. Increase `timeout_secs` in Gaud's LiteLLM config (default: 120)
2. Check LiteLLM's `stream_timeout` per-model setting
3. Ensure no proxy/firewall is buffering SSE responses

### Circuit breaker keeps tripping

**Symptom**: Gaud returns 503 for LiteLLM requests even though LiteLLM is up.

**Fix**: The circuit breaker trips after 3 consecutive failures. Check:
1. LiteLLM logs for upstream provider errors
2. Whether the upstream provider's API key is valid
3. Network connectivity between Gaud and LiteLLM

The circuit breaker resets after 30 seconds. If the underlying issue is resolved, requests will resume automatically.

## Gaud Configuration Reference

### TOML Configuration

```toml
[providers.litellm]
# Base URL of the LiteLLM proxy (required)
url = "http://localhost:4000"

# API key for authenticating to LiteLLM (optional)
# Use the master_key from LiteLLM's general_settings
api_key = "sk-1234"

# Auto-discover models from GET /v1/models at startup (default: true)
discover_models = true

# Manually listed models (always available even if discovery fails)
models = [
  "litellm:gpt-4o",
  "litellm:claude-sonnet-4",
]

# Request timeout in seconds (default: 120)
timeout_secs = 120
```

### Environment Variables

| Variable | Description | Creates config if missing |
|---|---|---|
| `GAUD_LITELLM_URL` | LiteLLM proxy URL | Yes |
| `GAUD_LITELLM_API_KEY` | API key | No |
| `GAUD_LITELLM_DISCOVER` | Enable model discovery (`true`/`false`) | No |
| `GAUD_LITELLM_TIMEOUT` | Timeout in seconds | No |
