# Configuration Guide

Gaud is configured through a TOML file and optional environment variable overrides.

## Config File Location

By default, Gaud looks for `llm-proxy.toml` in the current working directory. Override with:

```bash
# CLI flag
gaud --config /etc/gaud/config.toml

# Environment variable
GAUD_CONFIG=/etc/gaud/config.toml gaud
```

If the config file is missing, Gaud starts with built-in defaults and logs a warning.

## Override Precedence

```
Environment variables  (highest priority)
        |
    TOML file
        |
  Built-in defaults   (lowest priority)
```

When a setting is overridden by an environment variable, the web UI Settings page displays that setting as read-only with an indicator showing which env var is active.

## TOML File Format

### Server

```toml
[server]
host = "127.0.0.1"       # Bind address
port = 8400               # Listen port
cors_origins = []         # Allowed CORS origins (empty = allow all)
```

### Database

```toml
[database]
path = "gaud.db"          # Path to the SQLite database file
```

### Authentication

```toml
[auth]
enabled = true                    # Master auth switch
default_admin_name = "admin"      # Username for the bootstrap admin
# bootstrap_key = "sk-prx-..."   # Pre-set admin key (optional)

[auth.tls_client_cert]
enabled = false                   # Enable TLS client cert auth
# ca_cert_path = "/etc/ssl/ca.pem"  # Informational only
require_cert = false              # Reject requests without a valid cert
# header_name = "X-Client-Cert-CN"  # Header from reverse proxy
```

When `auth.enabled` is `false`, all API routes are accessible without authentication. This is useful for local development but should never be used in production.

### Providers

```toml
[providers]
routing_strategy = "priority"                    # priority | round_robin | least_used | random
token_storage_dir = "~/.local/share/gaud/tokens" # Where OAuth tokens are stored
storage_backend = "file"                         # file | keyring | memory

[providers.claude]
client_id = "YOUR_ANTHROPIC_CLIENT_ID"
# auth_url = "https://console.anthropic.com/oauth/authorize"
# token_url = "https://console.anthropic.com/v1/oauth/token"
# callback_port = 19284

[providers.gemini]
client_id = "YOUR_GOOGLE_CLIENT_ID"
client_secret = "YOUR_GOOGLE_CLIENT_SECRET"
# auth_url = "https://accounts.google.com/o/oauth2/v2/auth"
# token_url = "https://oauth2.googleapis.com/token"
# callback_port = 19285

[providers.copilot]
# client_id = "Iv1.b507a08c87ecfe98"   # Default GitHub Copilot client ID
```

Omit a provider section entirely to disable it. For example, remove `[providers.claude]` to disable Claude routing.

### Budget

```toml
[budget]
enabled = true                    # Enable per-user budget tracking
warning_threshold_percent = 80    # Warn at this usage percentage
```

### Logging

```toml
[logging]
level = "info"         # trace | debug | info | warn | error
json = false           # Output logs in JSON format
log_content = false    # Log request/response content (verbose)
```

The `RUST_LOG` environment variable takes precedence over the config file log level.

## Complete Environment Variable Reference

| Environment Variable | TOML Path | Type | Default | Description |
|---|---|---|---|---|
| `GAUD_SERVER_HOST` | `server.host` | string | `127.0.0.1` | Server bind address |
| `GAUD_SERVER_PORT` | `server.port` | integer | `8400` | Server listen port |
| `GAUD_SERVER_CORS_ORIGINS` | `server.cors_origins` | comma-separated | (empty) | Allowed CORS origins |
| `GAUD_DATABASE_PATH` | `database.path` | path | `gaud.db` | SQLite database file path |
| `GAUD_AUTH_ENABLED` | `auth.enabled` | bool | `true` | Enable API authentication |
| `GAUD_AUTH_ADMIN_NAME` | `auth.default_admin_name` | string | `admin` | Bootstrap admin username |
| `GAUD_AUTH_BOOTSTRAP_KEY` | `auth.bootstrap_key` | string | (none) | Pre-set bootstrap admin API key |
| `GAUD_AUTH_TLS_ENABLED` | `auth.tls_client_cert.enabled` | bool | `false` | Enable TLS client cert auth |
| `GAUD_AUTH_TLS_CA_CERT` | `auth.tls_client_cert.ca_cert_path` | path | (none) | CA cert path (informational) |
| `GAUD_AUTH_TLS_REQUIRE` | `auth.tls_client_cert.require_cert` | bool | `false` | Require client certificates |
| `GAUD_AUTH_TLS_HEADER` | `auth.tls_client_cert.header_name` | string | `X-Client-Cert-CN` | Header name for client cert CN |
| `GAUD_PROVIDERS_ROUTING` | `providers.routing_strategy` | string | `priority` | Routing strategy |
| `GAUD_PROVIDERS_TOKEN_DIR` | `providers.token_storage_dir` | path | `~/.local/share/gaud/tokens` | Token storage directory |
| `GAUD_PROVIDERS_STORAGE_BACKEND` | `providers.storage_backend` | string | `file` | Token storage backend |
| `GAUD_BUDGET_ENABLED` | `budget.enabled` | bool | `true` | Enable budget enforcement |
| `GAUD_BUDGET_WARNING_THRESHOLD` | `budget.warning_threshold_percent` | integer | `80` | Budget warning threshold (%) |
| `GAUD_LOG_LEVEL` | `logging.level` | string | `info` | Log level |
| `GAUD_LOG_JSON` | `logging.json` | bool | `false` | JSON log output |
| `GAUD_LOG_CONTENT` | `logging.log_content` | bool | `false` | Log request content |

Boolean env vars accept: `1`, `true`, `yes`, `on` (truthy) or `0`, `false`, `no`, `off` (falsy).

## Example Configurations

### Minimal Local Development

```toml
[server]
host = "127.0.0.1"
port = 8400

[auth]
enabled = false

[providers.copilot]
# Uses default GitHub Copilot client ID
```

### Production with Environment Overrides

```toml
# config.toml -- base config
[server]
host = "0.0.0.0"
port = 8400

[database]
path = "/var/lib/gaud/gaud.db"

[providers]
routing_strategy = "round_robin"
token_storage_dir = "/var/lib/gaud/tokens"

[budget]
enabled = true
warning_threshold_percent = 90

[logging]
level = "info"
json = true
```

```bash
# Override sensitive values via env vars
export GAUD_AUTH_BOOTSTRAP_KEY="sk-prx-your-secret-key"
export GAUD_SERVER_CORS_ORIGINS="https://app.example.com"
gaud --config /etc/gaud/config.toml
```

### Multi-Provider Setup

```toml
[providers]
routing_strategy = "priority"
storage_backend = "file"

[providers.claude]
client_id = "your-anthropic-client-id"

[providers.gemini]
client_id = "your-google-client-id"
client_secret = "your-google-client-secret"

[providers.copilot]
# Default client_id is used if omitted
```

### TLS Client Certificate Auth (behind nginx)

```toml
[auth]
enabled = true

[auth.tls_client_cert]
enabled = true
require_cert = true
header_name = "X-SSL-Client-CN"
ca_cert_path = "/etc/ssl/client-ca.pem"
```

With nginx configuration:

```nginx
server {
    listen 443 ssl;
    ssl_client_certificate /etc/ssl/client-ca.pem;
    ssl_verify_client on;

    location / {
        proxy_pass http://127.0.0.1:8400;
        proxy_set_header X-SSL-Client-CN $ssl_client_s_dn_cn;
    }
}
```

## Web UI Settings Page

The Settings page in the web dashboard (`/ui/dashboard`) displays all configuration settings with their current effective values. When a setting is overridden by an environment variable:

- The input field is disabled (read-only)
- A label shows the env var name that controls the setting
- The displayed value reflects the env var override, not the TOML file value

Settings that are not overridden can be edited through the web UI, which writes changes back to the TOML config file.
