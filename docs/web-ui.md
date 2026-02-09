# Web UI Guide

Gaud includes a built-in web dashboard for managing OAuth connections, users, API keys, usage logs, budgets, and settings. The UI is rendered from embedded HTML templates using minijinja -- no separate frontend build step is required.

## Accessing the Dashboard

Open your browser to:

```
http://127.0.0.1:8400/ui/dashboard
```

Visiting the root URL (`/`) redirects to the dashboard automatically.

## Login

The login page is at `/ui/login`. Enter your API key (the full `sk-prx-*` key) to authenticate. The key is stored in `sessionStorage` and sent as the `Authorization: Bearer` header with all AJAX requests.

When authentication is disabled (`auth.enabled = false`), the web UI does not require login.

## Pages

### Dashboard (`/ui/dashboard`)

The main overview page. Displays:

- **Provider Status Table** -- Shows each registered provider, its health status (circuit breaker state), available models, and average latency.
- **Quick Stats** -- Summary of total requests, active users, and budget consumption across the system.

Data is loaded via AJAX calls to the API endpoints and refreshed on page load.

### OAuth Management (`/ui/oauth`)

Manage OAuth connections to upstream LLM providers. Shows:

- **Provider Cards** -- One card per configured provider (Claude, Gemini, Copilot), showing:
  - Whether the provider is configured in `llm-proxy.toml`
  - Whether a valid OAuth token exists
  - A "Connect" button to start the OAuth flow

#### Connecting a Provider

1. Click "Connect" for the desired provider
2. For Claude and Gemini: a popup opens to the provider's authorization page. After granting access, the popup closes and the status updates.
3. For Copilot: a device code and verification URL are displayed. Visit the URL, enter the code, and the page polls until authorization completes.

#### OAuth Status

The UI checks token status by calling:

```
GET /ui/api/oauth/status/{provider}
```

Response:

```json
{
  "provider": "claude",
  "configured": true,
  "authenticated": true
}
```

#### Starting a Flow

The UI initiates OAuth by calling:

```
POST /ui/api/oauth/start/{provider}
```

Response:

```json
{
  "provider": "claude",
  "auth_url": "https://console.anthropic.com/oauth/authorize?...",
  "message": "Open the auth_url to begin authorization"
}
```

#### OAuth Callback

After the provider redirects back, the callback page at `/oauth/callback/{provider}` displays a success or failure message:

- **Success:** "OAuth Completed" with the provider name and an auto-close prompt.
- **Failure:** "OAuth Failed" with the error details from the provider.

### User Management (`/ui/users`)

Manage proxy users and their API keys. Provides:

- **Users Table** -- Lists all users with their ID, name, role, and creation date.
- **Create User** -- Form to add a new user with a name and role (`admin` or `member`).
- **Delete User** -- Remove a user and all their API keys.
- **API Keys** -- For each user, view and manage API keys:
  - List existing keys (showing prefix and label, not the full key)
  - Create new keys (the full plaintext key is shown exactly once)
  - Revoke keys (takes effect immediately)

Data is loaded from `GET /admin/users` and key management uses the `/admin/users/{id}/keys` and `/admin/keys/{id}` endpoints.

### Usage Logs (`/ui/usage`)

View request history across all users. Features:

- **Usage Table** -- Columns: user, provider, model, input tokens, output tokens, cost, latency, status, timestamp.
- **Filters** -- Filter by user ID, provider, and date range.
- **Pagination** -- Navigate through results with page controls.

Data is loaded from `GET /admin/usage` with query parameters for filtering and pagination.

### Budget Management (`/ui/budgets`)

Configure and monitor per-user spending limits. Shows:

- **Budgets Table** -- Lists all users with their monthly limit, daily limit, current monthly spend, and current daily spend.
- **Set Budget** -- Form to configure monthly and/or daily limits for a user.
- **Budget Progress** -- Visual indicators showing how much of each limit has been consumed.

Data is loaded from `GET /admin/budgets/{user_id}` and updated via `PUT /admin/budgets/{user_id}`.

Budget periods reset automatically:
- Monthly counters reset on the first of each month
- Daily counters reset at midnight UTC

### Settings (`/ui/settings`)

View and edit Gaud configuration settings. Shows:

- **Settings List** -- All configuration settings with their current effective values.
- **Environment Override Indicators** -- When a setting is overridden by an environment variable:
  - The input field is disabled (read-only)
  - A label shows which `GAUD_*` environment variable controls the setting
  - The displayed value reflects the env var override, not the TOML file value
- **Edit Controls** -- Settings that are not env-overridden can be edited inline. Changes are written to the TOML config file.

Data is loaded from `GET /admin/settings` and updated via `PUT /admin/settings`.

Changes made through the settings page require a server restart to take effect. The UI displays a notice after saving.

## Route Summary

### Page Routes (HTML)

| Path | Description |
|---|---|
| `/` | Redirects to `/ui/dashboard` |
| `/ui/login` | Login page (no auth required) |
| `/ui/dashboard` | Main dashboard |
| `/ui/oauth` | OAuth management |
| `/ui/users` | User management |
| `/ui/usage` | Usage logs |
| `/ui/budgets` | Budget management |
| `/ui/settings` | Configuration settings |

### OAuth Routes

| Path | Method | Description |
|---|---|---|
| `/oauth/callback/{provider}` | GET | OAuth callback from provider (no auth) |
| `/ui/api/oauth/start/{provider}` | POST | Start an OAuth flow |
| `/ui/api/oauth/status/{provider}` | GET | Check OAuth status for a provider |

### Data Routes

The web UI loads data by calling the admin API endpoints documented in the [API Reference](api-reference.md):

- `GET /admin/users` -- User list
- `GET /admin/users/{id}/keys` -- API keys per user
- `GET /admin/budgets/{user_id}` -- Budget data
- `GET /admin/usage` -- Usage logs
- `GET /admin/settings` -- Configuration settings
- `GET /health` -- Provider health status

## Templates

The web UI uses embedded HTML templates rendered with [minijinja](https://github.com/mitsuhiko/minijinja). Templates are compiled into the binary -- no external files are needed at runtime.

| Template | Used By |
|---|---|
| `layout` | Base layout with navigation bar (shared by all pages) |
| `login` | Login page |
| `dashboard` | Main dashboard |
| `oauth` | OAuth management |
| `oauth_callback` | OAuth callback result (success/failure) |
| `users` | User management |
| `usage` | Usage logs |
| `budgets` | Budget management |
| `settings` | Configuration settings |

All page templates extend the `layout` template, which provides a consistent navigation bar with links to Dashboard, OAuth, Users, Usage, Budgets, and Settings.

## Access Control

| Page | Required Role |
|---|---|
| Login page | None |
| OAuth callback | None |
| Dashboard | Any authenticated user (read-only for members) |
| OAuth, Users, Usage, Budgets, Settings | Admin |

When `auth.enabled = false`, all pages are accessible without login and admin endpoints are unrestricted.
