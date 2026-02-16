# Migrating OAuth Providers to the `oauth2` Crate

Guide for replacing hand-rolled OAuth HTTP plumbing with the [`oauth2`](https://docs.rs/oauth2/5.0.0) crate (v5.0). Based on the Gemini provider migration completed in this project.

## Why

Each manual provider implementation repeats ~150-200 lines of:
- URL construction with `urlencoding::encode`
- `reqwest` POST with form/JSON body
- Status code checking and error body parsing
- `TokenResponse` / `TokenErrorResponse` deserialization structs
- `invalid_grant` detection for refresh retry logic

The `oauth2` crate handles all of this with type-safe builders, automatic PKCE, and built-in error mapping.

## What Gets Removed Per Provider

| Removed | Replaced By |
|---------|-------------|
| `TokenResponse` struct | `oauth2::StandardTokenResponse` (via `TokenResponse` trait) |
| `TokenErrorResponse` struct | `oauth2::StandardErrorResponse<BasicErrorResponseType>` |
| Manual URL construction in `build_authorize_url` | `client.authorize_url().set_pkce_challenge().url()` |
| Manual `reqwest::Client::post().form()/json()` in `exchange_code` | `client.exchange_code().set_pkce_verifier().request_async()` |
| Manual `reqwest::Client::post()` in `refresh_token` | `client.exchange_refresh_token().request_async()` |
| `if error.error == "invalid_grant"` matching | `BasicErrorResponseType::InvalidGrant` enum variant |

## What Stays the Same

- `PROVIDER_ID`, `DEFAULT_*_URL`, `DEFAULT_SCOPES` constants
- `*OAuthConfig` struct and its constructors
- `TokenInfo` return type and composite token format
- `OAuthError` enum (the `map_token_error` helper maps into it)
- The 3 call sites in `mod.rs` (`start_*_flow`, `complete_*_flow`, `refresh_token`)

## Step-by-Step Migration Pattern

### 1. Add the dependency (once)

Already done:

```toml
# Cargo.toml [dependencies]
oauth2 = "5.0"
```

### 2. Replace imports in the provider module

```rust
// Remove:
use serde::Deserialize;

// Add:
use oauth2::TokenResponse as _;
use oauth2::basic::{BasicClient, BasicErrorResponseType};
use oauth2::{
    AuthType, AuthUrl, AuthorizationCode, ClientId, ClientSecret, CsrfToken,
    PkceCodeChallenge, PkceCodeVerifier, RedirectUrl, RefreshToken, Scope, TokenUrl,
};
```

> **Key detail**: `TokenResponse as _` brings the trait methods (`.access_token()`, `.refresh_token()`, `.expires_in()`) into scope without a named import. Without it you get "private field, not a method" errors.

### 3. Add the helpers

**Shared helpers** (in `src/oauth/mod.rs`):

```rust
/// Fully-configured BasicClient with auth and token endpoints set.
pub(crate) type OAuthClient = BasicClient<
    EndpointSet, EndpointNotSet, EndpointNotSet, EndpointNotSet, EndpointSet,
>;

/// Map oauth2 errors to OAuthError, preserving invalid_grant -> TokenExpired.
pub(crate) fn map_oauth_token_error<RE: std::error::Error + 'static>(
    provider: &str,
    err: oauth2::RequestTokenError<RE, oauth2::StandardErrorResponse<BasicErrorResponseType>>,
) -> OAuthError { /* ... */ }
```

**Per-provider helper** (thin delegate):

```rust
fn map_token_error<RE: std::error::Error + 'static>(err: ...) -> OAuthError {
    super::map_oauth_token_error(PROVIDER_ID, err)
}
```

**Per-provider `build_oauth2_client`**:

```rust
fn build_oauth2_client(config: &ProviderOAuthConfig) -> Result<OAuthClient, OAuthError> {
    let client = BasicClient::new(ClientId::new(config.client_id.clone()))
        .set_client_secret(ClientSecret::new(config.client_secret.clone())) // if needed
        .set_auth_uri(AuthUrl::new(config.auth_url.clone()).map_err(...)?)
        .set_token_uri(TokenUrl::new(config.token_url.clone()).map_err(...)?)
        .set_redirect_uri(RedirectUrl::new(config.redirect_uri.clone()).map_err(...)?)
        .set_auth_type(AuthType::RequestBody); // see provider notes below
    Ok(client)
}
```

**HTTP client**: `OAuthManager` creates a shared `reqwest::Client` with `redirect(Policy::none())` and passes it to provider functions. No per-call client construction.

### 4. Rewrite `build_authorize_url`

**Old signature** (manual):
```rust
pub fn build_authorize_url(config: &Config, pkce: &Pkce, state: &str) -> String
```

**New signature** (oauth2 crate generates PKCE internally):
```rust
pub fn build_authorize_url(config: &Config, state: &str) -> Result<(String, String), OAuthError>
//                                                                   ^^^url   ^^^verifier
```

```rust
pub fn build_authorize_url(config: &Config, state: &str) -> Result<(String, String), OAuthError> {
    let client = build_oauth2_client(config)?;
    let (pkce_challenge, pkce_verifier) = PkceCodeChallenge::new_random_sha256();

    let mut request = client
        .authorize_url(|| CsrfToken::new(state.to_string()))
        .set_pkce_challenge(pkce_challenge);
        // Add provider-specific extra params here (see notes below)

    for scope in &config.scopes {
        request = request.add_scope(Scope::new(scope.clone()));
    }

    let (url, _csrf) = request.url();
    Ok((url.to_string(), pkce_verifier.secret().to_string()))
}
```

### 5. Rewrite `exchange_code`

**Old signature**: `(http_client, config, code, verifier) -> Result<TokenInfo>`
**New signature**: `(http_client, config, code, verifier) -> Result<TokenInfo>` (uses shared HTTP client)

```rust
pub async fn exchange_code(
    http_client: &reqwest::Client,
    config: &Config,
    code: &str,
    verifier: &str,
) -> Result<TokenInfo, OAuthError> {
    let client = build_oauth2_client(config)?;
    let http_client = http_client.clone(); // cheap Arc clone for owned AsyncHttpClient

    let token_response = client
        .exchange_code(AuthorizationCode::new(code.to_string()))
        .set_pkce_verifier(PkceCodeVerifier::new(verifier.to_string()))
        .request_async(&http_client)
        .await
        .map_err(map_token_error)?;

    let access_token = token_response.access_token().secret().to_string();
    let refresh_token = token_response.refresh_token()
        .map(|rt| rt.secret().to_string())
        .ok_or_else(|| OAuthError::ExchangeFailed("No refresh token".to_string()))?;
    let expires_in = token_response.expires_in().map(|d| d.as_secs() as i64);

    Ok(TokenInfo::new(access_token, Some(refresh_token), expires_in, PROVIDER_ID))
}
```

### 6. Rewrite `refresh_token`

Same pattern as `exchange_code` but uses `exchange_refresh_token`. Preserve the composite token format (`refresh|project_id|managed_project_id`) by splitting before the request and re-attaching after.

```rust
pub async fn refresh_token(
    http_client: &reqwest::Client,
    config: &Config,
    refresh_token_value: &str,
) -> Result<TokenInfo, OAuthError> {
    let parts: Vec<&str> = refresh_token_value.split('|').collect();
    let base_refresh = parts[0];

    let client = build_oauth2_client(config)?;
    let http_client = http_client.clone(); // cheap Arc clone for owned AsyncHttpClient

    let token_response = client
        .exchange_refresh_token(&RefreshToken::new(base_refresh.to_string()))
        .request_async(&http_client)
        .await
        .map_err(map_token_error)?;

    let access_token = token_response.access_token().secret().to_string();
    let new_refresh = token_response.refresh_token()
        .map(|rt| rt.secret().to_string())
        .unwrap_or_else(|| base_refresh.to_string());
    let expires_in = token_response.expires_in().map(|d| d.as_secs() as i64);

    let mut token = TokenInfo::new(access_token, Some(new_refresh), expires_in, PROVIDER_ID);

    // Re-attach composite project IDs
    if let Some(project) = parts.get(1).filter(|s| !s.is_empty()) {
        let managed = parts.get(2).filter(|s| !s.is_empty()).map(|s| *s);
        token = token.with_project_ids(project, managed);
    }

    Ok(token)
}
```

### 7. Update `mod.rs` call sites (3 changes per provider)

| Call site | Change |
|-----------|--------|
| `start_*_flow()` | Remove `Pkce::generate()`, use new `build_authorize_url` returning `(url, verifier)` |
| `complete_*_flow()` | Pass `&self.http_client` as first arg to provider's `exchange_code` |
| `refresh_token()` | Pass `&self.http_client` as first arg to provider's `refresh_token` |

### 8. Delete dead code

Remove `TokenResponse` struct, `TokenErrorResponse` struct, and any `use serde::Deserialize` that was only used by them.

### 9. Update tests

- Sync tests: update `build_authorize_url` calls for `Result<(String, String)>` return type.
- Async tests: use `wiremock` to mock the token endpoint. Point `config.token_url` at `mock_server.uri()`.

## Provider-Specific Notes

### Gemini (Google) — Done

- **Auth type**: `AuthType::RequestBody` (Google expects form-encoded credentials, not Basic Auth)
- **Extra params**: `.add_extra_param("access_type", "offline")` and `.add_extra_param("prompt", "consent")`
- **Client secret**: Required
- **Refresh behavior**: Google typically does not return a new refresh token on refresh

### Claude (Anthropic) — Done

Claude's token endpoint requires **JSON-encoded** requests. The `oauth2` crate sends form-encoded by default, so the migration uses a custom `JsonHttpClient` wrapper (in `src/oauth/claude.rs`) that implements `oauth2::AsyncHttpClient`. It intercepts outgoing requests, detects `application/x-www-form-urlencoded` content type, re-serializes the body as JSON via `form_to_json()`, and sets `Content-Type: application/json` before executing with `reqwest::Client`.

- **Auth type**: No client secret (PKCE-only) — omit `.set_client_secret()`
- **Extra params**: `.add_extra_param("code", "true")`
- **Scopes**: `org:create_api_key`, `user:profile`, `user:inference`
- **HTTP client**: Accepts a shared `&reqwest::Client` from `OAuthManager`, wrapped in `JsonHttpClient` for the JSON conversion

### Copilot (GitHub) — Not Applicable

Copilot uses the **Device Authorization Grant** (RFC 8628), not the Authorization Code flow. The `oauth2` crate does support device code flow via `exchange_device_code()` and `exchange_device_access_token()`, but the current implementation is already concise (request device code, poll for token). Migration is optional and lower priority.

If migrated:
- **Auth type**: `AuthType::RequestBody`
- **No PKCE**: Device code flow doesn't use PKCE
- **Polling**: `client.exchange_device_access_token().request_async()` with retry loop

### Kiro (Amazon Q / AWS CodeWhisperer) — Completed: Trait-Based Architecture (No `oauth2` Crate)

Kiro uses **two distinct auth paths**, detected at startup based on whether credentials contain a `clientId`/`clientSecret`:

| Auth Type | When | Endpoint | `oauth2` crate? |
|-----------|------|----------|-----------------|
| **KIRO_DESKTOP** | No `clientId`/`clientSecret` (IDE users) | `prod.{region}.auth.desktop.kiro.dev/refreshToken` | No |
| **AWS_SSO_OIDC** | Has `clientId`/`clientSecret` (kiro-cli / enterprise) | `oidc.{region}.amazonaws.com/token` | Partially |

Reference implementation: `kiro-gateway/kiro/auth.py` (private Kiro Gateway repo, not publicly linked)

#### Auth type detection

The gateway determines which path to use based on credential shape (mirrors `_detect_auth_type()` in kiro-gateway):

```text
if credentials contain clientId + clientSecret → AWS_SSO_OIDC
else                                            → KIRO_DESKTOP
```

#### Credential sources (priority order)

1. **SQLite** — `~/.local/share/kiro-cli/data.sqlite3`, table `auth_kv`, keys:
   - `kirocli:social:token` (social login — Google, GitHub, Microsoft)
   - `kirocli:odic:token` (AWS SSO OIDC via kiro-cli)
   - `codewhisperer:odic:token` (legacy)
2. **JSON file** — `~/.aws/sso/cache/kiro-auth-token.json` or `GAUD_KIRO_CREDS_FILE`
3. **Environment variable** — `GAUD_KIRO_REFRESH_TOKEN`
4. **Config field** — `providers.kiro.refresh_token`

For AWS SSO OIDC, device registration (`clientId`/`clientSecret`) is loaded from SQLite keys:
- `kirocli:odic:device-registration`
- `codewhisperer:odic:device-registration`

#### KIRO_DESKTOP flow (proprietary, NOT standard OAuth2)

```http
POST https://prod.{region}.auth.desktop.kiro.dev/refreshToken
Content-Type: application/json
User-Agent: KiroIDE-0.7.45-{machine_fingerprint}

Request:  { "refreshToken": "eyJ..." }
Response: { "accessToken": "eyJ...", "refreshToken": "eyJ...", "expiresIn": 3600,
            "profileArn": "arn:aws:codewhisperer:..." }   ← all camelCase
```

- No client ID/secret
- No authorization URL or code exchange
- No standard OAuth2 error response format (HTTP status codes only)
- Response may include a new `refreshToken` (must be persisted)
- Response may include `profileArn` (must be preserved)

**Why the `oauth2` crate doesn't fit** for this path:

| Standard OAuth2 | Kiro Desktop Auth |
|-----------------|-------------------|
| `access_token` (snake_case) | `accessToken` (camelCase) |
| `error` / `error_description` structured errors | HTTP 400/401/403 only |
| Client credentials (ID + optional secret) | No client identity |
| Form-encoded token requests | JSON body |
| `invalid_grant` error code | No error codes |

#### AWS_SSO_OIDC flow (semi-standard, `oauth2` crate partially applicable)

```http
POST https://oidc.{region}.amazonaws.com/token
Content-Type: application/json

Request:  { "grantType": "refresh_token", "clientId": "...",
            "clientSecret": "...", "refreshToken": "eyJ..." }   ← camelCase!
Response: { "accessToken": "eyJ...", "refreshToken": "eyJ...",
            "expiresIn": 3600 }                                  ← camelCase!
```

This is _close_ to standard OAuth2 but with important deviations:
- **JSON body** instead of `application/x-www-form-urlencoded`
- **camelCase field names** (`grantType` instead of `grant_type`, `clientId` instead of `client_id`)
- **camelCase response** (`accessToken` instead of `access_token`)
- **Ephemeral client registration** — `clientId`/`clientSecret` expire and need re-registration

The `oauth2` crate's `exchange_refresh_token()` sends standard form-encoded `grant_type=refresh_token` with `client_id`/`client_secret` — AWS OIDC expects the same semantics but in JSON with camelCase. This means the crate would need a custom `AsyncHttpClient` implementation to rewrite the content type and field names, which is more work than just using reqwest directly.

#### Token lifecycle (both paths)

```text
KiroAuthManager
├── get_access_token()
│   ├── Fast path: token valid and not expiring within 10 min → return cached
│   └── Slow path: refresh() → POST to auth endpoint
├── refresh()
│   ├── POST with refreshToken
│   ├── Parse response (camelCase)
│   ├── Calculate expiry: now + expiresIn - 60s safety margin
│   ├── Cache in RwLock<Option<TokenState>>
│   └── Persist to file/SQLite (update same source we loaded from)
└── force_refresh() ← called on 403 from API
    └── Clears cached token, then refresh()
```

Refresh threshold: **10 minutes** before expiry (`TOKEN_REFRESH_THRESHOLD = 600s`)
Safety margin: **60 seconds** subtracted from reported `expiresIn`

#### Error handling and graceful degradation

The kiro-gateway reference has resilient error handling that gaud should mirror:

1. **On 400 (`invalid_request`) during refresh in SQLite mode**: Reload credentials from SQLite (kiro-cli may have refreshed in the background), retry once
2. **On refresh failure with valid cached token**: Use the existing access token until it actually expires
3. **On 403 from the API**: Force-refresh the token and retry the request once
4. **Thread safety**: `asyncio.Lock` in Python → `RwLock` in gaud (already implemented)

#### Implementation status

The Kiro provider has been refactored from a monolithic `providers/kiro.rs` into a modular `providers/kiro/` directory using **strategy** and **repository** patterns. All features from the kiro-gateway reference are implemented:

| Feature | kiro-gateway | gaud | Status |
|---------|-------------|------|--------|
| KIRO_DESKTOP refresh | Yes | `KiroDesktopStrategy` | Done |
| AWS_SSO_OIDC refresh | Yes | `AwsSsoOidcStrategy` | Done |
| Auth type detection | Yes | `KiroTokenInfo::detect_auth_type()` | Done |
| SQLite credential source | Yes | `SqliteStore` | Done |
| JSON file credential source | Yes | `JsonFileStore` | Done |
| Environment variable source | Yes | `EnvStore` | Done |
| Social login tokens | Yes (`kirocli:social:token`) | Yes (SQLite key list) | Done |
| Persist refreshed tokens to file/SQLite | Yes | `CredentialStore::save()` | Done |
| Force-refresh on 403 | Yes | `KiroClient` retry logic | Done |
| Graceful degradation on refresh failure | Yes | Uses cached token until expiry | Done |
| New refreshToken from response | Yes (persisted) | Yes | Done |
| Async-safe I/O | N/A (Python) | `spawn_blocking` for fs/SQLite | Done |
| Enterprise device registration | Yes | `load_enterprise_device_registration()` | Done |

#### Why the `oauth2` crate was not used

Both Kiro auth paths deviate too far from standard OAuth2 for the `oauth2` crate to add value:

1. **KIRO_DESKTOP**: Proprietary — no client ID, JSON body, camelCase fields, no standard error format
2. **AWS_SSO_OIDC**: Semi-standard — but uses JSON body with camelCase fields (`grantType`, `clientId`) instead of form-encoded snake_case. A custom `AsyncHttpClient` adapter would be needed to rewrite content type and field names, which is more complexity than using reqwest directly.

The trait-based architecture (`AuthStrategy` + `CredentialStore`) provides the same decoupling benefits that `oauth2` crate types would offer, while correctly handling the proprietary protocol details.

#### Architecture (trait-based decoupling)

```text
src/providers/kiro/
├── mod.rs          — KiroProvider (LlmProvider impl)
├── auth.rs         — KiroAuthManager, KiroTokenProvider trait, AutoDetectProvider
├── models.rs       — AuthType, CredentialSource, KiroTokenInfo, TokenUpdate
├── strategies.rs   — AuthStrategy trait, KiroDesktopStrategy, AwsSsoOidcStrategy
├── stores.rs       — CredentialStore trait, JsonFileStore, SqliteStore, EnvStore
└── client.rs       — KiroClient HTTP transport, machine fingerprint
```

Key traits:
- **`AuthStrategy`**: Strategy pattern for auth flows (`refresh()`, `can_handle()`)
- **`CredentialStore`**: Repository pattern for credential persistence (`load()`, `save()`, `can_handle()`)
- **`KiroTokenProvider`**: Token lifecycle abstraction (`get_access_token()`, `force_refresh()`)

## Testing Pattern

Each provider's wiremock tests follow the same structure:

```rust
#[tokio::test]
async fn test_exchange_code_success() {
    let mock_server = wiremock::MockServer::start().await;

    wiremock::Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::path("/"))
        .respond_with(
            wiremock::ResponseTemplate::new(200)
                .set_body_json(serde_json::json!({
                    "access_token": "test-access",
                    "token_type": "Bearer",
                    "expires_in": 3600,
                    "refresh_token": "test-refresh"
                })),
        )
        .mount(&mock_server)
        .await;

    let config = mock_config(&mock_server.uri()); // token_url -> mock server
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();
    let result = exchange_code(&client, &config, "code", "verifier").await;
    let token = result.unwrap();
    assert_eq!(token.access_token, "test-access");
}
```

Key test scenarios per provider:
- **Success** with all fields
- **Missing refresh token** (exchange should fail)
- **`invalid_grant` error** → `OAuthError::TokenExpired`
- **Other server errors** → `OAuthError::ExchangeFailed`
- **Refresh preserves composite format** (`refresh|project|managed`)

## Verification Checklist

```bash
cargo check                    # Compiles
cargo test -p gaud --lib       # All unit tests pass
cargo clippy -p gaud --no-deps # No new warnings in modified files
```
