# Authentication Guide

Gaud supports API key authentication, TLS client certificate authentication, and can optionally run with auth disabled for development.

## API Key Authentication

### How It Works

1. An admin creates a user and generates an API key
2. The key is displayed exactly once in `sk-prx-*` format
3. Clients send the key in the `Authorization: Bearer` header
4. Gaud hashes the key with SHA-256 + Argon2id and verifies against stored hashes
5. On success, the authenticated user identity is attached to the request

### Key Format

All API keys follow the format:

```
sk-prx-{32 alphanumeric characters}
```

Example: `sk-prx-aBcDeFgH12345678901234567890ab`

The `sk-prx-` prefix distinguishes Gaud proxy keys from upstream provider keys. Only a truncated prefix (e.g., `sk-prx-aBcDeFgH...`) is stored for display purposes; the full key is never stored in plaintext.

### Using an API Key

```bash
curl http://127.0.0.1:8400/v1/chat/completions \
  -H "Authorization: Bearer sk-prx-YOUR_KEY_HERE" \
  -H "Content-Type: application/json" \
  -d '{"model": "claude-sonnet-4-20250514", "messages": [{"role": "user", "content": "Hi"}]}'
```

### Key Security

- Keys are hashed using SHA-256 followed by Argon2id before storage
- The plaintext key is shown exactly once at creation time
- The `last_used` timestamp is updated on each successful authentication
- Revoking a key immediately invalidates it

## Roles

Gaud has two user roles:

| Role | API Access | Admin Endpoints | Web UI Admin |
|---|---|---|---|
| `admin` | Full | Full | Full |
| `member` | `/v1/*` only | None | Read-only |

### Admin Role

Admins can:
- Create and delete users
- Generate and revoke API keys for any user
- Set and view budgets
- Query usage logs across all users
- Manage settings through the web UI

### Member Role

Members can:
- Use the `/v1/chat/completions`, `/v1/models`, and `/v1/embeddings` endpoints
- View the web dashboard (read-only)

Members cannot access any `/admin/*` endpoint. Attempts return HTTP 403.

## Bootstrap Admin

On first run (when no users exist), Gaud automatically:

1. Creates an admin user with the name from `auth.default_admin_name` (default: `admin`)
2. Generates an API key and prints it to stdout
3. If `auth.bootstrap_key` is set, uses that value instead of generating a random key

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

To pre-set the bootstrap key (useful for automated deployments):

```toml
[auth]
bootstrap_key = "sk-prx-myPredeterminedKey1234567890ab"
```

Or via environment variable:

```bash
GAUD_AUTH_BOOTSTRAP_KEY="sk-prx-myPredeterminedKey1234567890ab"
```

## Disabling Authentication

For local development, authentication can be disabled entirely:

```toml
[auth]
enabled = false
```

Or:

```bash
GAUD_AUTH_ENABLED=false
```

When auth is disabled:
- All API routes are accessible without a Bearer token
- Admin endpoints are accessible to all requests
- A synthetic admin user identity is injected for audit logging
- The web UI does not require login

**Never disable auth in production.**

## TLS Client Certificate Authentication

When Gaud runs behind a TLS-terminating reverse proxy (nginx, Envoy, Caddy, etc.), the proxy can verify client certificates and pass the certificate Common Name (CN) to Gaud via a header.

### How It Works

```
Client (with TLS cert)
    |
    v
[Reverse Proxy]  -- verifies client cert
    |                 extracts CN
    v                 sets header
[Gaud]           -- reads CN from header
                     maps CN to user
```

1. The reverse proxy terminates TLS and verifies the client certificate against a CA
2. The proxy sets a header (default: `X-Client-Cert-CN`) with the certificate's Common Name
3. Gaud reads this header and authenticates the request as the user whose name matches the CN

### Configuration

```toml
[auth.tls_client_cert]
enabled = true
require_cert = false               # true = reject requests without cert
header_name = "X-Client-Cert-CN"   # Header set by the reverse proxy
ca_cert_path = "/etc/ssl/ca.pem"   # Informational only (proxy does verification)
```

| Setting | Description |
|---|---|
| `enabled` | Turn on TLS client cert auth |
| `require_cert` | When `true`, requests without a valid cert header are rejected. When `false`, falls back to API key auth |
| `header_name` | The header name set by the reverse proxy. Default: `X-Client-Cert-CN` |
| `ca_cert_path` | Path to the CA certificate. This is informational only -- the reverse proxy handles actual verification |

### Environment Variable Overrides

```bash
GAUD_AUTH_TLS_ENABLED=true
GAUD_AUTH_TLS_REQUIRE=true
GAUD_AUTH_TLS_HEADER=X-SSL-Client-CN
GAUD_AUTH_TLS_CA_CERT=/etc/ssl/ca.pem
```

### Auth Flow with TLS Client Certs

```
Request arrives
    |
    v
Is TLS client cert auth enabled?
    |
    +-- No --> Use API key auth (Bearer token)
    |
    +-- Yes --> Is cert header present?
                    |
                    +-- Yes --> Map CN to user, authenticate
                    |
                    +-- No --> Is require_cert = true?
                                |
                                +-- Yes --> Reject (401)
                                |
                                +-- No --> Fall back to API key auth
```

### Example: nginx Configuration

```nginx
server {
    listen 443 ssl;

    ssl_certificate     /etc/ssl/server.pem;
    ssl_certificate_key /etc/ssl/server-key.pem;
    ssl_client_certificate /etc/ssl/client-ca.pem;
    ssl_verify_client on;

    location / {
        proxy_pass http://127.0.0.1:8400;
        proxy_set_header Host $host;
        proxy_set_header X-Client-Cert-CN $ssl_client_s_dn_cn;
    }
}
```

## Error Responses

All authentication errors follow the OpenAI error format:

```json
{
  "error": {
    "message": "Authentication required: Missing Authorization header",
    "type": "authentication_error",
    "code": "invalid_api_key"
  }
}
```

| HTTP Status | Error Type | Scenario |
|---|---|---|
| 401 | `authentication_error` | Missing or invalid API key |
| 403 | `permission_error` | Member attempting admin action |
| 429 | `rate_limit_error` | Budget exceeded |
