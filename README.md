# Gaud

**Multi-user LLM proxy with OpenAI-compatible API**

Gaud sits between your applications and multiple LLM providers, presenting a single OpenAI-compatible endpoint. It handles OAuth authentication with upstream providers, routes requests by model name, tracks per-user usage and budgets, and provides a web dashboard for administration.

## Features

- **OpenAI-compatible API** -- Drop-in replacement for `/v1/chat/completions`, `/v1/models`, and `/v1/embeddings`. Works with any client that speaks the OpenAI protocol.
- **Multi-provider routing** -- Routes requests to Claude (Anthropic), Gemini (Google), or GitHub Copilot based on model name. Supports priority, round-robin, least-used, and random routing strategies with automatic fallback.
- **OAuth provider authentication** -- PKCE flow for Claude and Gemini, Device Code flow for GitHub Copilot. Tokens are stored on disk, in the system keyring, or in memory.
- **Per-user budget management** -- Set monthly and daily spending limits per user. The proxy rejects requests when budgets are exceeded and warns when approaching thresholds.
- **API key authentication** -- Argon2-hashed API keys with `sk-prx-*` prefix. Admin and member roles. Optional TLS client certificate auth via reverse proxy headers.
- **Web dashboard** -- Built-in HTML UI for managing OAuth connections, users, API keys, usage logs, budgets, and settings.
- **Environment variable overrides** -- Every TOML setting can be overridden by a `GAUD_*` environment variable. The web UI shows which settings are locked by env vars.
- **Circuit breaker health monitoring** -- Tracks provider failures and automatically stops sending requests to unhealthy providers until they recover.

## Quick Start

### Prerequisites

- Rust 1.85+ (2024 edition)
- SQLite (bundled via `rusqlite`)

### Build and Run

```bash
# Clone and build
git clone https://github.com/your-org/gaud.git
cd gaud
cargo build --release

# Copy and edit the example config
cp llm-proxy.toml my-config.toml
# Edit my-config.toml with your provider credentials

# Run
./target/release/gaud --config my-config.toml
```

On first run, Gaud creates an admin user and prints the API key to stdout:

```
=========================================================
  GAUD first-run bootstrap
---------------------------------------------------------
  Admin user : admin
  API key    : sk-prx-aBcDeFgH12345678901234567890ab
---------------------------------------------------------
  Save this key now -- it will not be shown again.
=========================================================
```

### Verify It Works

```bash
# Health check (no auth required)
curl http://127.0.0.1:8400/health

# List models (auth required)
curl http://127.0.0.1:8400/v1/models \
  -H "Authorization: Bearer sk-prx-YOUR_KEY"

# Chat completion
curl http://127.0.0.1:8400/v1/chat/completions \
  -H "Authorization: Bearer sk-prx-YOUR_KEY" \
  -H "Content-Type: application/json" \
  -d '{
    "model": "claude-sonnet-4-20250514",
    "messages": [{"role": "user", "content": "Hello!"}]
  }'
```

### Web Dashboard

Open `http://127.0.0.1:8400/ui/dashboard` in your browser. Log in with your admin API key.

## Configuration

Gaud reads configuration from a TOML file (default: `llm-proxy.toml`). Every setting can be overridden by environment variables prefixed with `GAUD_`.

**Precedence:** Environment variables > TOML file > built-in defaults

| Environment Variable | TOML Key | Default | Description |
|---|---|---|---|
| `GAUD_SERVER_HOST` | `server.host` | `127.0.0.1` | Bind address |
| `GAUD_SERVER_PORT` | `server.port` | `8400` | Listen port |
| `GAUD_DATABASE_PATH` | `database.path` | `gaud.db` | SQLite database path |
| `GAUD_AUTH_ENABLED` | `auth.enabled` | `true` | Enable/disable authentication |
| `GAUD_AUTH_ADMIN_NAME` | `auth.default_admin_name` | `admin` | Default admin username |
| `GAUD_PROVIDERS_ROUTING` | `providers.routing_strategy` | `priority` | Routing strategy |
| `GAUD_BUDGET_ENABLED` | `budget.enabled` | `true` | Enable budget tracking |
| `GAUD_LOG_LEVEL` | `logging.level` | `info` | Log level |

See [docs/configuration.md](docs/configuration.md) for the complete reference.

## Architecture

```
                    Clients (curl, SDKs, Claude Code, etc.)
                              |
                              v
                   +---------------------+
                   |   Gaud HTTP Server   |
                   |   (Axum + Tower)     |
                   +---------------------+
                   |  CORS | Tracing |    |
                   |  Request ID layers   |
                   +----------+-----------+
                              |
              +---------------+---------------+
              |               |               |
         /v1/* API       /admin/*        /ui/* Web UI
         (auth mw)       (auth mw)      (templates)
              |               |
              v               v
    +------------------+  +----------+
    | Provider Router  |  | Admin    |
    | (model routing,  |  | Handlers |
    |  circuit breaker)|  +----------+
    +--+------+------+-+       |
       |      |      |        v
       v      v      v    +--------+
    Claude  Gemini  Copilot| SQLite |
   (Anthropic)(Google)(GitHub)| (users,|
       |      |      |    | keys,  |
       v      v      v    | budgets|
    OAuth Token Storage    | usage) |
    (file/keyring/memory)  +--------+
```

## Documentation

- [Configuration Guide](docs/configuration.md) -- TOML format, env vars, example configs
- [Authentication](docs/authentication.md) -- API keys, roles, TLS client certs
- [Providers](docs/providers.md) -- Claude, Gemini, Copilot setup and routing
- [API Reference](docs/api-reference.md) -- All HTTP endpoints with examples
- [Web UI](docs/web-ui.md) -- Dashboard, OAuth, user management
- [srrldb & Semantic Cache](src/srrldb/README.md) -- Embedded SurrealDB wrapper and two-tier LLM response cache

## License

MIT License. See [LICENSE](LICENSE) for details.
