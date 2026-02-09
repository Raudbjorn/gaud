//! Embedded HTML templates for the Gaud web UI.
//!
//! All templates are defined as `&str` constants and rendered via minijinja.
//! The UI uses a dark theme with inline CSS -- no external assets required.

/// Base layout template. All pages extend this.
pub const LAYOUT: &str = r#"<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="utf-8">
    <meta name="viewport" content="width=device-width, initial-scale=1">
    <title>{% block title %}Gaud{% endblock %} - LLM Proxy</title>
    <style>
        :root {
            --bg-primary: #0f1117;
            --bg-secondary: #1a1d27;
            --bg-tertiary: #242736;
            --border: #2e3245;
            --text-primary: #e1e4ed;
            --text-secondary: #8b8fa3;
            --text-muted: #5f6375;
            --accent: #6366f1;
            --accent-hover: #818cf8;
            --success: #22c55e;
            --warning: #f59e0b;
            --danger: #ef4444;
            --info: #3b82f6;
            --radius: 8px;
            --shadow: 0 1px 3px rgba(0,0,0,0.4);
        }
        *, *::before, *::after { box-sizing: border-box; margin: 0; padding: 0; }
        body {
            font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, sans-serif;
            background: var(--bg-primary);
            color: var(--text-primary);
            line-height: 1.6;
            min-height: 100vh;
        }
        a { color: var(--accent); text-decoration: none; }
        a:hover { color: var(--accent-hover); }

        /* Navigation */
        .navbar {
            background: var(--bg-secondary);
            border-bottom: 1px solid var(--border);
            padding: 0 1.5rem;
            display: flex;
            align-items: center;
            height: 56px;
            position: sticky;
            top: 0;
            z-index: 100;
        }
        .navbar-brand {
            font-size: 1.25rem;
            font-weight: 700;
            color: var(--text-primary);
            margin-right: 2rem;
            letter-spacing: -0.02em;
        }
        .navbar-brand span { color: var(--accent); }
        .nav-links { display: flex; gap: 0.25rem; flex: 1; }
        .nav-link {
            padding: 0.5rem 0.875rem;
            border-radius: var(--radius);
            color: var(--text-secondary);
            font-size: 0.875rem;
            font-weight: 500;
            transition: all 0.15s;
        }
        .nav-link:hover { color: var(--text-primary); background: var(--bg-tertiary); }
        .nav-link.active { color: var(--accent); background: rgba(99,102,241,0.1); }
        .nav-user {
            font-size: 0.8125rem;
            color: var(--text-muted);
        }
        .nav-user .logout-btn {
            background: none;
            border: none;
            color: var(--text-secondary);
            cursor: pointer;
            font-size: 0.8125rem;
            padding: 0.25rem 0.5rem;
            border-radius: 4px;
            margin-left: 0.5rem;
        }
        .nav-user .logout-btn:hover { color: var(--danger); background: rgba(239,68,68,0.1); }

        /* Main content */
        .container {
            max-width: 1200px;
            margin: 0 auto;
            padding: 1.5rem;
        }
        .page-header {
            margin-bottom: 1.5rem;
        }
        .page-header h1 {
            font-size: 1.5rem;
            font-weight: 600;
            letter-spacing: -0.02em;
        }
        .page-header p {
            color: var(--text-secondary);
            font-size: 0.875rem;
            margin-top: 0.25rem;
        }

        /* Cards */
        .card {
            background: var(--bg-secondary);
            border: 1px solid var(--border);
            border-radius: var(--radius);
            padding: 1.25rem;
            box-shadow: var(--shadow);
        }
        .card-header {
            font-size: 0.875rem;
            font-weight: 600;
            text-transform: uppercase;
            letter-spacing: 0.05em;
            color: var(--text-secondary);
            margin-bottom: 1rem;
            padding-bottom: 0.75rem;
            border-bottom: 1px solid var(--border);
        }
        .card-grid {
            display: grid;
            grid-template-columns: repeat(auto-fit, minmax(280px, 1fr));
            gap: 1rem;
        }

        /* Stat blocks */
        .stat { text-align: center; padding: 1rem 0.5rem; }
        .stat-value {
            font-size: 2rem;
            font-weight: 700;
            line-height: 1.2;
            letter-spacing: -0.03em;
        }
        .stat-label {
            font-size: 0.75rem;
            color: var(--text-secondary);
            text-transform: uppercase;
            letter-spacing: 0.05em;
            margin-top: 0.25rem;
        }
        .stat-value.success { color: var(--success); }
        .stat-value.warning { color: var(--warning); }
        .stat-value.danger { color: var(--danger); }
        .stat-value.info { color: var(--info); }

        /* Tables */
        .table-wrap { overflow-x: auto; }
        table {
            width: 100%;
            border-collapse: collapse;
            font-size: 0.875rem;
        }
        th {
            text-align: left;
            padding: 0.625rem 0.75rem;
            font-weight: 600;
            color: var(--text-secondary);
            border-bottom: 1px solid var(--border);
            font-size: 0.75rem;
            text-transform: uppercase;
            letter-spacing: 0.05em;
        }
        td {
            padding: 0.625rem 0.75rem;
            border-bottom: 1px solid var(--border);
            color: var(--text-primary);
        }
        tr:last-child td { border-bottom: none; }
        tr:hover td { background: rgba(255,255,255,0.02); }

        /* Badges */
        .badge {
            display: inline-block;
            padding: 0.125rem 0.5rem;
            border-radius: 9999px;
            font-size: 0.75rem;
            font-weight: 600;
            letter-spacing: 0.02em;
        }
        .badge-success { background: rgba(34,197,94,0.15); color: var(--success); }
        .badge-warning { background: rgba(245,158,11,0.15); color: var(--warning); }
        .badge-danger { background: rgba(239,68,68,0.15); color: var(--danger); }
        .badge-info { background: rgba(59,130,246,0.15); color: var(--info); }
        .badge-muted { background: rgba(95,99,117,0.2); color: var(--text-muted); }

        /* Buttons */
        .btn {
            display: inline-flex;
            align-items: center;
            gap: 0.375rem;
            padding: 0.5rem 1rem;
            border: 1px solid var(--border);
            border-radius: var(--radius);
            background: var(--bg-tertiary);
            color: var(--text-primary);
            font-size: 0.875rem;
            font-weight: 500;
            cursor: pointer;
            transition: all 0.15s;
        }
        .btn:hover { border-color: var(--text-muted); background: var(--bg-secondary); }
        .btn-primary { background: var(--accent); border-color: var(--accent); color: #fff; }
        .btn-primary:hover { background: var(--accent-hover); border-color: var(--accent-hover); }
        .btn-danger { background: var(--danger); border-color: var(--danger); color: #fff; }
        .btn-danger:hover { background: #dc2626; border-color: #dc2626; }
        .btn-sm { padding: 0.25rem 0.625rem; font-size: 0.8125rem; }
        .btn:disabled { opacity: 0.5; cursor: not-allowed; }

        /* Forms */
        .form-group { margin-bottom: 1rem; }
        .form-label {
            display: block;
            font-size: 0.8125rem;
            font-weight: 500;
            color: var(--text-secondary);
            margin-bottom: 0.375rem;
        }
        .form-input {
            width: 100%;
            padding: 0.5rem 0.75rem;
            background: var(--bg-primary);
            border: 1px solid var(--border);
            border-radius: var(--radius);
            color: var(--text-primary);
            font-size: 0.875rem;
            transition: border-color 0.15s;
        }
        .form-input:focus {
            outline: none;
            border-color: var(--accent);
            box-shadow: 0 0 0 2px rgba(99,102,241,0.25);
        }

        /* Alerts */
        .alert {
            padding: 0.75rem 1rem;
            border-radius: var(--radius);
            font-size: 0.875rem;
            margin-bottom: 1rem;
        }
        .alert-info { background: rgba(59,130,246,0.1); border: 1px solid rgba(59,130,246,0.25); color: var(--info); }
        .alert-success { background: rgba(34,197,94,0.1); border: 1px solid rgba(34,197,94,0.25); color: var(--success); }
        .alert-warning { background: rgba(245,158,11,0.1); border: 1px solid rgba(245,158,11,0.25); color: var(--warning); }
        .alert-danger { background: rgba(239,68,68,0.1); border: 1px solid rgba(239,68,68,0.25); color: var(--danger); }

        /* Progress bar */
        .progress {
            height: 6px;
            background: var(--bg-primary);
            border-radius: 3px;
            overflow: hidden;
            margin-top: 0.5rem;
        }
        .progress-bar {
            height: 100%;
            border-radius: 3px;
            transition: width 0.3s ease;
        }
        .progress-bar.success { background: var(--success); }
        .progress-bar.warning { background: var(--warning); }
        .progress-bar.danger { background: var(--danger); }

        /* Utility */
        .text-success { color: var(--success); }
        .text-warning { color: var(--warning); }
        .text-danger { color: var(--danger); }
        .text-info { color: var(--info); }
        .text-muted { color: var(--text-muted); }
        .text-secondary { color: var(--text-secondary); }
        .mt-1 { margin-top: 0.5rem; }
        .mt-2 { margin-top: 1rem; }
        .mt-3 { margin-top: 1.5rem; }
        .mb-1 { margin-bottom: 0.5rem; }
        .mb-2 { margin-bottom: 1rem; }
        .flex { display: flex; }
        .flex-wrap { flex-wrap: wrap; }
        .items-center { align-items: center; }
        .justify-between { justify-content: space-between; }
        .gap-1 { gap: 0.5rem; }
        .gap-2 { gap: 1rem; }
        .hidden { display: none; }
        .mono { font-family: 'SF Mono', SFMono-Regular, Consolas, monospace; font-size: 0.8125rem; }
        .empty-state {
            text-align: center;
            padding: 3rem 1rem;
            color: var(--text-muted);
        }
        .empty-state p { font-size: 0.875rem; margin-top: 0.5rem; }

        /* Responsive */
        @media (max-width: 768px) {
            .navbar { padding: 0 1rem; }
            .nav-links { gap: 0; }
            .nav-link { padding: 0.5rem 0.5rem; font-size: 0.8125rem; }
            .container { padding: 1rem; }
            .card-grid { grid-template-columns: 1fr; }
            .nav-user { display: none; }
        }
    </style>
</head>
<body>
    {% block body %}{% endblock %}

    <script>
        // Shared utilities
        const GAUD = {
            getApiKey() {
                return sessionStorage.getItem('gaud_api_key') || '';
            },
            setApiKey(key) {
                sessionStorage.setItem('gaud_api_key', key);
            },
            clearApiKey() {
                sessionStorage.removeItem('gaud_api_key');
            },
            isLoggedIn() {
                return !!this.getApiKey();
            },
            headers() {
                return {
                    'Authorization': 'Bearer ' + this.getApiKey(),
                    'Content-Type': 'application/json',
                };
            },
            async apiFetch(url, options = {}) {
                const resp = await fetch(url, {
                    ...options,
                    headers: { ...this.headers(), ...(options.headers || {}) },
                });
                if (resp.status === 401) {
                    this.clearApiKey();
                    window.location.href = '/ui/login';
                    return null;
                }
                return resp;
            },
            logout() {
                this.clearApiKey();
                window.location.href = '/ui/login';
            },
            requireAuth() {
                if (!this.isLoggedIn()) {
                    window.location.href = '/ui/login';
                    return false;
                }
                return true;
            },
            formatCost(cost) {
                if (cost === 0) return '$0.00';
                if (cost < 0.01) return '$' + cost.toFixed(4);
                return '$' + cost.toFixed(2);
            },
            formatNumber(n) {
                if (n >= 1_000_000) return (n / 1_000_000).toFixed(1) + 'M';
                if (n >= 1_000) return (n / 1_000).toFixed(1) + 'K';
                return n.toString();
            },
            timeAgo(dateStr) {
                const date = new Date(dateStr);
                const now = new Date();
                const secs = Math.floor((now - date) / 1000);
                if (secs < 60) return secs + 's ago';
                if (secs < 3600) return Math.floor(secs / 60) + 'm ago';
                if (secs < 86400) return Math.floor(secs / 3600) + 'h ago';
                return Math.floor(secs / 86400) + 'd ago';
            }
        };
    </script>
    {% block scripts %}{% endblock %}
</body>
</html>"#;

/// Login page template.
pub const LOGIN: &str = r#"{% extends "layout" %}
{% block title %}Login{% endblock %}
{% block body %}
<div style="display:flex;align-items:center;justify-content:center;min-height:100vh;padding:1rem;">
    <div class="card" style="width:100%;max-width:400px;">
        <div style="text-align:center;margin-bottom:1.5rem;">
            <h1 style="font-size:1.5rem;font-weight:700;letter-spacing:-0.02em;">
                <span style="color:var(--accent);">gaud</span>
            </h1>
            <p class="text-secondary" style="font-size:0.875rem;margin-top:0.25rem;">LLM Proxy Dashboard</p>
        </div>
        <div id="login-error" class="alert alert-danger hidden"></div>
        <form id="login-form">
            <div class="form-group">
                <label class="form-label" for="api-key">API Key</label>
                <input class="form-input mono" type="password" id="api-key"
                       placeholder="sk-prx-..." autocomplete="off" autofocus>
            </div>
            <button type="submit" class="btn btn-primary" style="width:100%;">Sign In</button>
        </form>
        <p class="text-muted" style="text-align:center;font-size:0.75rem;margin-top:1rem;">
            Enter your API key to access the dashboard.
        </p>
    </div>
</div>
{% endblock %}
{% block scripts %}
<script>
    document.getElementById('login-form').addEventListener('submit', async (e) => {
        e.preventDefault();
        const key = document.getElementById('api-key').value.trim();
        const errEl = document.getElementById('login-error');
        errEl.classList.add('hidden');

        if (!key) {
            errEl.textContent = 'Please enter an API key.';
            errEl.classList.remove('hidden');
            return;
        }

        try {
            const resp = await fetch('/health', {
                headers: { 'Authorization': 'Bearer ' + key }
            });
            // Health endpoint doesn't require auth, so we test against admin/users
            const testResp = await fetch('/admin/users', {
                headers: { 'Authorization': 'Bearer ' + key }
            });
            if (testResp.ok || testResp.status === 403) {
                // 200 = admin, 403 = valid key but not admin -- both mean valid
                GAUD.setApiKey(key);
                window.location.href = '/ui/dashboard';
            } else {
                errEl.textContent = 'Invalid API key. Please try again.';
                errEl.classList.remove('hidden');
            }
        } catch (err) {
            errEl.textContent = 'Connection error. Is the server running?';
            errEl.classList.remove('hidden');
        }
    });

    // If already logged in, redirect
    if (GAUD.isLoggedIn()) {
        window.location.href = '/ui/dashboard';
    }
</script>
{% endblock %}"#;

/// Dashboard page template.
pub const DASHBOARD: &str = r#"{% extends "layout" %}
{% block title %}Dashboard{% endblock %}
{% block body %}
<nav class="navbar">
    <a class="navbar-brand" href="/ui/dashboard"><span>gaud</span></a>
    <div class="nav-links">
        <a class="nav-link active" href="/ui/dashboard">Dashboard</a>
        <a class="nav-link" href="/ui/oauth">OAuth</a>
        <a class="nav-link" href="/ui/users">Users</a>
        <a class="nav-link" href="/ui/usage">Usage</a>
        <a class="nav-link" href="/ui/budgets">Budgets</a>
        <a class="nav-link" href="/ui/settings">Settings</a>
    </div>
    <div class="nav-user">
        <span id="nav-username"></span>
        <button class="logout-btn" onclick="GAUD.logout()">Logout</button>
    </div>
</nav>
<div class="container">
    <div class="page-header">
        <h1>Dashboard</h1>
        <p>Overview of your LLM proxy instance</p>
    </div>

    <!-- Stats row -->
    <div class="card-grid mb-2">
        <div class="card">
            <div class="stat">
                <div class="stat-value info" id="stat-providers">--</div>
                <div class="stat-label">Active Providers</div>
            </div>
        </div>
        <div class="card">
            <div class="stat">
                <div class="stat-value success" id="stat-requests">--</div>
                <div class="stat-label">Total Requests</div>
            </div>
        </div>
        <div class="card">
            <div class="stat">
                <div class="stat-value warning" id="stat-cost">--</div>
                <div class="stat-label">Total Cost</div>
            </div>
        </div>
        <div class="card">
            <div class="stat">
                <div class="stat-value" id="stat-tokens" style="color:var(--text-primary);">--</div>
                <div class="stat-label">Total Tokens</div>
            </div>
        </div>
    </div>

    <!-- Provider status -->
    <div class="card mb-2">
        <div class="card-header">Provider Status</div>
        <div class="table-wrap">
            <table>
                <thead>
                    <tr>
                        <th>Provider</th>
                        <th>Status</th>
                        <th>Models</th>
                        <th>OAuth</th>
                    </tr>
                </thead>
                <tbody id="provider-table">
                    <tr><td colspan="4" class="text-muted">Loading...</td></tr>
                </tbody>
            </table>
        </div>
    </div>

    <!-- Recent usage -->
    <div class="card">
        <div class="card-header">Recent Activity</div>
        <div class="table-wrap">
            <table>
                <thead>
                    <tr>
                        <th>Time</th>
                        <th>User</th>
                        <th>Provider</th>
                        <th>Model</th>
                        <th>Tokens</th>
                        <th>Cost</th>
                        <th>Status</th>
                    </tr>
                </thead>
                <tbody id="recent-usage">
                    <tr><td colspan="7" class="text-muted">Loading...</td></tr>
                </tbody>
            </table>
        </div>
    </div>
</div>
{% endblock %}
{% block scripts %}
<script>
    if (!GAUD.requireAuth()) throw new Error('Not authenticated');

    async function loadDashboard() {
        try {
            // Load health/status
            const healthResp = await fetch('/health');
            if (healthResp.ok) {
                const health = await healthResp.json();
                const providers = health.providers || [];
                const activeCount = providers.filter(p => p.healthy).length;
                document.getElementById('stat-providers').textContent = activeCount + '/' + providers.length;

                let rows = '';
                for (const p of providers) {
                    const statusBadge = p.healthy
                        ? '<span class="badge badge-success">Healthy</span>'
                        : '<span class="badge badge-danger">Unhealthy</span>';
                    const models = (p.models || []).join(', ') || '--';
                    const oauthBadge = p.authenticated
                        ? '<span class="badge badge-success">Connected</span>'
                        : '<span class="badge badge-muted">Not connected</span>';
                    rows += '<tr><td>' + p.provider + '</td><td>' + statusBadge +
                            '</td><td class="mono" style="font-size:0.75rem;">' + models +
                            '</td><td>' + oauthBadge + '</td></tr>';
                }
                document.getElementById('provider-table').innerHTML = rows || '<tr><td colspan="4" class="text-muted">No providers configured</td></tr>';
            }

            // Load usage stats
            const usageResp = await GAUD.apiFetch('/admin/usage?limit=10');
            if (usageResp && usageResp.ok) {
                const data = await usageResp.json();
                const entries = data.entries || data || [];
                const summary = data.summary || {};

                document.getElementById('stat-requests').textContent =
                    GAUD.formatNumber(summary.total_requests || entries.length);
                document.getElementById('stat-cost').textContent =
                    GAUD.formatCost(summary.total_cost || 0);
                document.getElementById('stat-tokens').textContent =
                    GAUD.formatNumber(summary.total_tokens || 0);

                let rows = '';
                for (const e of entries.slice(0, 10)) {
                    const statusBadge = e.status === 'success'
                        ? '<span class="badge badge-success">OK</span>'
                        : '<span class="badge badge-danger">' + (e.status || 'error') + '</span>';
                    const tokens = (e.input_tokens || 0) + (e.output_tokens || 0);
                    rows += '<tr>' +
                        '<td class="mono">' + GAUD.timeAgo(e.created_at) + '</td>' +
                        '<td>' + (e.user_name || e.user_id || '--') + '</td>' +
                        '<td>' + (e.provider || '--') + '</td>' +
                        '<td class="mono">' + (e.model || '--') + '</td>' +
                        '<td class="mono">' + GAUD.formatNumber(tokens) + '</td>' +
                        '<td class="mono">' + GAUD.formatCost(e.cost || 0) + '</td>' +
                        '<td>' + statusBadge + '</td></tr>';
                }
                document.getElementById('recent-usage').innerHTML =
                    rows || '<tr><td colspan="7" class="text-muted">No activity yet</td></tr>';
            }
        } catch (err) {
            console.error('Dashboard load error:', err);
        }
    }

    loadDashboard();
    setInterval(loadDashboard, 30000);
</script>
{% endblock %}"#;

/// OAuth management page template.
pub const OAUTH: &str = r#"{% extends "layout" %}
{% block title %}OAuth{% endblock %}
{% block body %}
<nav class="navbar">
    <a class="navbar-brand" href="/ui/dashboard"><span>gaud</span></a>
    <div class="nav-links">
        <a class="nav-link" href="/ui/dashboard">Dashboard</a>
        <a class="nav-link active" href="/ui/oauth">OAuth</a>
        <a class="nav-link" href="/ui/users">Users</a>
        <a class="nav-link" href="/ui/usage">Usage</a>
        <a class="nav-link" href="/ui/budgets">Budgets</a>
        <a class="nav-link" href="/ui/settings">Settings</a>
    </div>
    <div class="nav-user">
        <span id="nav-username"></span>
        <button class="logout-btn" onclick="GAUD.logout()">Logout</button>
    </div>
</nav>
<div class="container">
    <div class="page-header">
        <h1>OAuth Management</h1>
        <p>Connect and manage LLM provider authentication</p>
    </div>

    <div id="oauth-status" class="alert hidden"></div>

    <!-- Copilot device code modal -->
    <div id="copilot-device-modal" class="card mb-2 hidden">
        <div class="card-header">Copilot Device Authorization</div>
        <div style="text-align:center;padding:1rem;">
            <p style="margin-bottom:1rem;">Visit the URL below and enter the code:</p>
            <p style="margin-bottom:0.5rem;">
                <a id="copilot-verify-url" href="" target="_blank" style="font-size:1.125rem;"></a>
            </p>
            <p style="margin-bottom:1rem;">
                <code id="copilot-user-code" class="mono" style="font-size:2rem;font-weight:700;letter-spacing:0.1em;color:var(--accent);"></code>
            </p>
            <p class="text-muted" style="font-size:0.8125rem;">Waiting for authorization... <span id="copilot-poll-status"></span></p>
        </div>
    </div>

    <div class="card-grid" id="oauth-providers">
        <div class="card"><p class="text-muted">Loading providers...</p></div>
    </div>
</div>
{% endblock %}
{% block scripts %}
<script>
    if (!GAUD.requireAuth()) throw new Error('Not authenticated');

    const PROVIDERS = {{ providers_json }};

    const PROVIDER_LABELS = {
        claude: 'Claude (Anthropic)',
        gemini: 'Gemini (Google)',
        copilot: 'Copilot (GitHub)',
        kiro: 'Kiro (AWS)',
        litellm: 'LiteLLM',
    };

    function renderProviders(statuses) {
        const container = document.getElementById('oauth-providers');
        if (!PROVIDERS.length) {
            container.innerHTML = '<div class="card"><div class="empty-state"><p>No providers configured. Check your llm-proxy.toml.</p></div></div>';
            return;
        }

        let html = '';
        for (const prov of PROVIDERS) {
            const status = statuses[prov] || {};
            const authenticated = status.authenticated || false;
            const expired = status.expired || false;
            const label = PROVIDER_LABELS[prov] || prov;

            let statusBadge;
            if (authenticated && expired) {
                statusBadge = '<span class="badge badge-warning">Expired</span>';
            } else if (authenticated) {
                statusBadge = '<span class="badge badge-success">Connected</span>';
            } else {
                statusBadge = '<span class="badge badge-muted">Not connected</span>';
            }

            let expiresInfo = '';
            if (status.expires_in_secs && status.expires_in_secs > 0) {
                const hrs = Math.floor(status.expires_in_secs / 3600);
                const mins = Math.floor((status.expires_in_secs % 3600) / 60);
                expiresInfo = '<p class="text-muted mt-1" style="font-size:0.75rem;">Expires in: ' + hrs + 'h ' + mins + 'm</p>';
            }

            let btn;
            if (prov === 'kiro') {
                btn = authenticated
                    ? '<span class="text-muted" style="font-size:0.8125rem;">Managed via config</span>'
                    : '<span class="text-muted" style="font-size:0.8125rem;">Configure in llm-proxy.toml</span>';
            } else if (prov === 'litellm') {
                btn = authenticated
                    ? '<span class="text-muted" style="font-size:0.8125rem;">Managed via config</span>'
                    : '<span class="text-muted" style="font-size:0.8125rem;">Configure in llm-proxy.toml</span>';
            } else if (authenticated) {
                btn = '<button class="btn btn-sm" onclick="startOAuth(\'' + prov + '\')">Reconnect</button>';
            } else {
                btn = '<button class="btn btn-sm btn-primary" onclick="startOAuth(\'' + prov + '\')">Connect</button>';
            }

            html += '<div class="card">' +
                '<div class="flex justify-between items-center mb-1">' +
                '<h3 style="font-size:1rem;font-weight:600;">' + label + '</h3>' +
                statusBadge +
                '</div>' +
                expiresInfo +
                '<div class="mt-2">' + btn + '</div>' +
                '</div>';
        }
        container.innerHTML = html;
    }

    async function loadStatuses() {
        const statuses = {};
        for (const prov of PROVIDERS) {
            try {
                const resp = await GAUD.apiFetch('/ui/api/oauth/status/' + prov);
                if (resp && resp.ok) {
                    statuses[prov] = await resp.json();
                }
            } catch (_) {}
        }
        renderProviders(statuses);
    }

    async function startOAuth(provider) {
        const statusEl = document.getElementById('oauth-status');
        statusEl.classList.add('hidden');

        if (provider === 'copilot') {
            await startCopilotDeviceFlow();
            return;
        }

        try {
            const resp = await GAUD.apiFetch('/ui/api/oauth/start/' + provider, { method: 'POST' });
            if (!resp) return;
            const data = await resp.json();
            if (data.auth_url) {
                window.open(data.auth_url, 'oauth_' + provider, 'width=600,height=700');
                statusEl.className = 'alert alert-info';
                statusEl.textContent = 'OAuth flow started for ' + provider + '. Complete authorization in the popup window.';
                statusEl.classList.remove('hidden');

                // Poll for completion
                const poll = setInterval(async () => {
                    try {
                        const sr = await GAUD.apiFetch('/ui/api/oauth/status/' + provider);
                        if (sr && sr.ok) {
                            const s = await sr.json();
                            if (s.authenticated) {
                                clearInterval(poll);
                                statusEl.className = 'alert alert-success';
                                statusEl.textContent = (PROVIDER_LABELS[provider] || provider) + ' connected successfully!';
                                loadStatuses();
                            }
                        }
                    } catch (_) {}
                }, 3000);
                setTimeout(() => clearInterval(poll), 300000);
            } else if (data.error) {
                statusEl.className = 'alert alert-danger';
                statusEl.textContent = data.error;
                statusEl.classList.remove('hidden');
            }
        } catch (err) {
            statusEl.className = 'alert alert-danger';
            statusEl.textContent = 'Error: ' + err.message;
            statusEl.classList.remove('hidden');
        }
    }

    async function startCopilotDeviceFlow() {
        const statusEl = document.getElementById('oauth-status');
        const modal = document.getElementById('copilot-device-modal');

        try {
            const resp = await GAUD.apiFetch('/ui/api/oauth/copilot/device', { method: 'POST' });
            if (!resp) return;
            const data = await resp.json();

            if (data.error) {
                statusEl.className = 'alert alert-danger';
                statusEl.textContent = data.error;
                statusEl.classList.remove('hidden');
                return;
            }

            // Show device code modal
            document.getElementById('copilot-verify-url').href = data.verification_uri;
            document.getElementById('copilot-verify-url').textContent = data.verification_uri;
            document.getElementById('copilot-user-code').textContent = data.user_code;
            document.getElementById('copilot-poll-status').textContent = '';
            modal.classList.remove('hidden');

            // Open verification URL
            window.open(data.verification_uri, 'copilot_auth', 'width=600,height=700');

            // Poll for completion
            const interval = Math.max(data.interval || 5, 5) * 1000;
            let attempts = 0;
            const maxAttempts = Math.ceil(data.expires_in / (interval / 1000));

            const pollTimer = setInterval(async () => {
                attempts++;
                document.getElementById('copilot-poll-status').textContent = '(attempt ' + attempts + ')';

                if (attempts >= maxAttempts) {
                    clearInterval(pollTimer);
                    modal.classList.add('hidden');
                    statusEl.className = 'alert alert-danger';
                    statusEl.textContent = 'Device code expired. Please try again.';
                    statusEl.classList.remove('hidden');
                    return;
                }

                try {
                    const pr = await GAUD.apiFetch('/ui/api/oauth/copilot/poll', {
                        method: 'POST',
                        body: JSON.stringify({ device_code: data.device_code }),
                    });
                    if (!pr) return;
                    const result = await pr.json();

                    if (result.status === 'complete') {
                        clearInterval(pollTimer);
                        modal.classList.add('hidden');
                        statusEl.className = 'alert alert-success';
                        statusEl.textContent = 'Copilot connected successfully!';
                        statusEl.classList.remove('hidden');
                        loadStatuses();
                    } else if (result.status === 'error') {
                        clearInterval(pollTimer);
                        modal.classList.add('hidden');
                        statusEl.className = 'alert alert-danger';
                        statusEl.textContent = 'Copilot error: ' + (result.error || 'Unknown error');
                        statusEl.classList.remove('hidden');
                    }
                    // 'pending' and 'slow_down' -- keep polling
                } catch (_) {}
            }, interval);

        } catch (err) {
            statusEl.className = 'alert alert-danger';
            statusEl.textContent = 'Error: ' + err.message;
            statusEl.classList.remove('hidden');
        }
    }

    loadStatuses();
</script>
{% endblock %}"#;

/// OAuth callback page template (shown after provider redirects back).
pub const OAUTH_CALLBACK: &str = r#"{% extends "layout" %}
{% block title %}OAuth Callback{% endblock %}
{% block body %}
<div style="display:flex;align-items:center;justify-content:center;min-height:100vh;padding:1rem;">
    <div class="card" style="width:100%;max-width:460px;text-align:center;">
        {% if success %}
        <div style="font-size:2.5rem;margin-bottom:0.75rem;">&#10003;</div>
        <h2 style="font-size:1.25rem;font-weight:600;">OAuth Completed</h2>
        <p class="text-secondary mt-1">
            Successfully connected <strong>{{ provider }}</strong>. You can close this window.
        </p>
        {% else %}
        <div style="font-size:2.5rem;margin-bottom:0.75rem;color:var(--danger);">&#10007;</div>
        <h2 style="font-size:1.25rem;font-weight:600;">OAuth Failed</h2>
        <p class="text-secondary mt-1">{{ error }}</p>
        <p class="text-muted mt-1" style="font-size:0.8125rem;">Please try again.</p>
        {% endif %}
        <p class="text-muted mt-2" style="font-size:0.75rem;">
            This window will close automatically in <span id="countdown">5</span> seconds.
        </p>
    </div>
</div>
{% endblock %}
{% block scripts %}
<script>
    let secs = 5;
    const cdEl = document.getElementById('countdown');
    const timer = setInterval(() => {
        secs--;
        cdEl.textContent = secs;
        if (secs <= 0) {
            clearInterval(timer);
            try { window.close(); } catch(_) {}
            window.location.href = '/ui/oauth';
        }
    }, 1000);
</script>
{% endblock %}"#;

/// User management page template.
pub const USERS: &str = r#"{% extends "layout" %}
{% block title %}Users{% endblock %}
{% block body %}
<nav class="navbar">
    <a class="navbar-brand" href="/ui/dashboard"><span>gaud</span></a>
    <div class="nav-links">
        <a class="nav-link" href="/ui/dashboard">Dashboard</a>
        <a class="nav-link" href="/ui/oauth">OAuth</a>
        <a class="nav-link active" href="/ui/users">Users</a>
        <a class="nav-link" href="/ui/usage">Usage</a>
        <a class="nav-link" href="/ui/budgets">Budgets</a>
        <a class="nav-link" href="/ui/settings">Settings</a>
    </div>
    <div class="nav-user">
        <span id="nav-username"></span>
        <button class="logout-btn" onclick="GAUD.logout()">Logout</button>
    </div>
</nav>
<div class="container">
    <div class="page-header flex justify-between items-center">
        <div>
            <h1>Users</h1>
            <p>Manage proxy users and API keys</p>
        </div>
        <button class="btn btn-primary" onclick="showCreateUser()">Create User</button>
    </div>

    <div id="user-alert" class="alert hidden"></div>

    <!-- Create user modal (inline) -->
    <div id="create-user-form" class="card mb-2 hidden">
        <div class="card-header">Create New User</div>
        <form onsubmit="createUser(event)">
            <div class="flex gap-2 flex-wrap">
                <div class="form-group" style="flex:1;min-width:200px;">
                    <label class="form-label" for="new-user-name">Username</label>
                    <input class="form-input" type="text" id="new-user-name" required>
                </div>
                <div class="form-group" style="flex:1;min-width:200px;">
                    <label class="form-label" for="new-user-role">Role</label>
                    <select class="form-input" id="new-user-role">
                        <option value="member">Member</option>
                        <option value="admin">Admin</option>
                    </select>
                </div>
            </div>
            <div class="flex gap-1">
                <button type="submit" class="btn btn-primary btn-sm">Create</button>
                <button type="button" class="btn btn-sm" onclick="hideCreateUser()">Cancel</button>
            </div>
        </form>
    </div>

    <!-- API key display (shown once after creation) -->
    <div id="api-key-display" class="alert alert-warning hidden">
        <strong>New API Key:</strong> <code id="new-api-key" class="mono"></code>
        <br><small class="text-muted">Save this key now. It will not be shown again.</small>
    </div>

    <div class="card">
        <div class="table-wrap">
            <table>
                <thead>
                    <tr>
                        <th>Name</th>
                        <th>Role</th>
                        <th>API Keys</th>
                        <th>Created</th>
                        <th>Actions</th>
                    </tr>
                </thead>
                <tbody id="users-table">
                    <tr><td colspan="5" class="text-muted">Loading...</td></tr>
                </tbody>
            </table>
        </div>
    </div>
</div>
{% endblock %}
{% block scripts %}
<script>
    if (!GAUD.requireAuth()) throw new Error('Not authenticated');

    function showCreateUser() { document.getElementById('create-user-form').classList.remove('hidden'); }
    function hideCreateUser() { document.getElementById('create-user-form').classList.add('hidden'); }

    async function loadUsers() {
        try {
            const resp = await GAUD.apiFetch('/admin/users');
            if (!resp || !resp.ok) return;
            const users = await resp.json();

            let rows = '';
            for (const u of users) {
                const roleBadge = u.role === 'admin'
                    ? '<span class="badge badge-info">admin</span>'
                    : '<span class="badge badge-muted">member</span>';
                rows += '<tr>' +
                    '<td><strong>' + u.name + '</strong></td>' +
                    '<td>' + roleBadge + '</td>' +
                    '<td><button class="btn btn-sm" onclick="createApiKey(\'' + u.id + '\', \'' + u.name + '\')">+ Key</button></td>' +
                    '<td class="text-secondary mono">' + (u.created_at || '--') + '</td>' +
                    '<td><button class="btn btn-sm btn-danger" onclick="deleteUser(\'' + u.id + '\', \'' + u.name + '\')">Delete</button></td>' +
                    '</tr>';
            }
            document.getElementById('users-table').innerHTML =
                rows || '<tr><td colspan="5" class="text-muted">No users found</td></tr>';
        } catch (err) {
            console.error('Failed to load users:', err);
        }
    }

    async function createUser(e) {
        e.preventDefault();
        const name = document.getElementById('new-user-name').value.trim();
        const role = document.getElementById('new-user-role').value;
        const alertEl = document.getElementById('user-alert');

        try {
            const resp = await GAUD.apiFetch('/admin/users', {
                method: 'POST',
                body: JSON.stringify({ name, role }),
            });
            if (!resp) return;
            const data = await resp.json();
            if (resp.ok) {
                if (data.api_key) {
                    document.getElementById('new-api-key').textContent = data.api_key;
                    document.getElementById('api-key-display').classList.remove('hidden');
                }
                hideCreateUser();
                document.getElementById('new-user-name').value = '';
                loadUsers();
            } else {
                alertEl.className = 'alert alert-danger';
                alertEl.textContent = data.error?.message || 'Failed to create user';
                alertEl.classList.remove('hidden');
            }
        } catch (err) {
            alertEl.className = 'alert alert-danger';
            alertEl.textContent = err.message;
            alertEl.classList.remove('hidden');
        }
    }

    async function createApiKey(userId, userName) {
        try {
            const resp = await GAUD.apiFetch('/admin/users/' + userId + '/keys', {
                method: 'POST',
                body: JSON.stringify({ label: 'web-ui' }),
            });
            if (!resp) return;
            const data = await resp.json();
            if (resp.ok && data.plaintext) {
                document.getElementById('new-api-key').textContent = data.plaintext;
                document.getElementById('api-key-display').classList.remove('hidden');
            }
        } catch (err) {
            console.error('Failed to create API key:', err);
        }
    }

    async function deleteUser(userId, userName) {
        if (!confirm('Delete user "' + userName + '"? This will revoke all their API keys.')) return;
        try {
            await GAUD.apiFetch('/admin/users/' + userId, { method: 'DELETE' });
            loadUsers();
        } catch (err) {
            console.error('Failed to delete user:', err);
        }
    }

    loadUsers();
</script>
{% endblock %}"#;

/// Usage logs page template.
pub const USAGE: &str = r#"{% extends "layout" %}
{% block title %}Usage{% endblock %}
{% block body %}
<nav class="navbar">
    <a class="navbar-brand" href="/ui/dashboard"><span>gaud</span></a>
    <div class="nav-links">
        <a class="nav-link" href="/ui/dashboard">Dashboard</a>
        <a class="nav-link" href="/ui/oauth">OAuth</a>
        <a class="nav-link" href="/ui/users">Users</a>
        <a class="nav-link active" href="/ui/usage">Usage</a>
        <a class="nav-link" href="/ui/budgets">Budgets</a>
        <a class="nav-link" href="/ui/settings">Settings</a>
    </div>
    <div class="nav-user">
        <span id="nav-username"></span>
        <button class="logout-btn" onclick="GAUD.logout()">Logout</button>
    </div>
</nav>
<div class="container">
    <div class="page-header">
        <h1>Usage Logs</h1>
        <p>Detailed request history and token usage</p>
    </div>

    <!-- Filters -->
    <div class="card mb-2">
        <div class="flex gap-2 flex-wrap items-center">
            <div class="form-group" style="margin:0;flex:1;min-width:150px;">
                <select class="form-input" id="filter-provider" onchange="loadUsage()">
                    <option value="">All Providers</option>
                    <option value="claude">Claude</option>
                    <option value="gemini">Gemini</option>
                    <option value="copilot">Copilot</option>
                </select>
            </div>
            <div class="form-group" style="margin:0;flex:1;min-width:150px;">
                <select class="form-input" id="filter-limit" onchange="loadUsage()">
                    <option value="25">Last 25</option>
                    <option value="50">Last 50</option>
                    <option value="100" selected>Last 100</option>
                    <option value="500">Last 500</option>
                </select>
            </div>
            <button class="btn btn-sm" onclick="loadUsage()">Refresh</button>
        </div>
    </div>

    <!-- Summary -->
    <div class="card-grid mb-2">
        <div class="card">
            <div class="stat">
                <div class="stat-value info" id="usage-total-requests">--</div>
                <div class="stat-label">Requests</div>
            </div>
        </div>
        <div class="card">
            <div class="stat">
                <div class="stat-value success" id="usage-total-tokens">--</div>
                <div class="stat-label">Total Tokens</div>
            </div>
        </div>
        <div class="card">
            <div class="stat">
                <div class="stat-value warning" id="usage-total-cost">--</div>
                <div class="stat-label">Total Cost</div>
            </div>
        </div>
        <div class="card">
            <div class="stat">
                <div class="stat-value" id="usage-avg-latency" style="color:var(--text-primary);">--</div>
                <div class="stat-label">Avg Latency</div>
            </div>
        </div>
    </div>

    <!-- Log table -->
    <div class="card">
        <div class="table-wrap">
            <table>
                <thead>
                    <tr>
                        <th>Timestamp</th>
                        <th>User</th>
                        <th>Provider</th>
                        <th>Model</th>
                        <th>In / Out</th>
                        <th>Cost</th>
                        <th>Latency</th>
                        <th>Status</th>
                    </tr>
                </thead>
                <tbody id="usage-table">
                    <tr><td colspan="8" class="text-muted">Loading...</td></tr>
                </tbody>
            </table>
        </div>
    </div>
</div>
{% endblock %}
{% block scripts %}
<script>
    if (!GAUD.requireAuth()) throw new Error('Not authenticated');

    async function loadUsage() {
        const provider = document.getElementById('filter-provider').value;
        const limit = document.getElementById('filter-limit').value;
        let url = '/admin/usage?limit=' + limit;
        if (provider) url += '&provider=' + provider;

        try {
            const resp = await GAUD.apiFetch(url);
            if (!resp || !resp.ok) return;
            const data = await resp.json();
            const entries = data.entries || data || [];

            let totalTokens = 0, totalCost = 0, totalLatency = 0;
            let rows = '';
            for (const e of entries) {
                const inTok = e.input_tokens || 0;
                const outTok = e.output_tokens || 0;
                totalTokens += inTok + outTok;
                totalCost += e.cost || 0;
                totalLatency += e.latency_ms || 0;

                const statusBadge = e.status === 'success'
                    ? '<span class="badge badge-success">OK</span>'
                    : '<span class="badge badge-danger">' + (e.status || 'err') + '</span>';

                rows += '<tr>' +
                    '<td class="mono" style="font-size:0.75rem;">' + (e.created_at || '--') + '</td>' +
                    '<td>' + (e.user_name || e.user_id || '--') + '</td>' +
                    '<td>' + (e.provider || '--') + '</td>' +
                    '<td class="mono" style="font-size:0.75rem;">' + (e.model || '--') + '</td>' +
                    '<td class="mono">' + GAUD.formatNumber(inTok) + ' / ' + GAUD.formatNumber(outTok) + '</td>' +
                    '<td class="mono">' + GAUD.formatCost(e.cost || 0) + '</td>' +
                    '<td class="mono">' + (e.latency_ms || 0) + 'ms</td>' +
                    '<td>' + statusBadge + '</td></tr>';
            }

            document.getElementById('usage-total-requests').textContent = entries.length;
            document.getElementById('usage-total-tokens').textContent = GAUD.formatNumber(totalTokens);
            document.getElementById('usage-total-cost').textContent = GAUD.formatCost(totalCost);
            const avgLat = entries.length > 0 ? Math.round(totalLatency / entries.length) : 0;
            document.getElementById('usage-avg-latency').textContent = avgLat + 'ms';

            document.getElementById('usage-table').innerHTML =
                rows || '<tr><td colspan="8" class="text-muted">No usage data</td></tr>';
        } catch (err) {
            console.error('Failed to load usage:', err);
        }
    }

    loadUsage();
</script>
{% endblock %}"#;

/// Budget management page template.
pub const BUDGETS: &str = r#"{% extends "layout" %}
{% block title %}Budgets{% endblock %}
{% block body %}
<nav class="navbar">
    <a class="navbar-brand" href="/ui/dashboard"><span>gaud</span></a>
    <div class="nav-links">
        <a class="nav-link" href="/ui/dashboard">Dashboard</a>
        <a class="nav-link" href="/ui/oauth">OAuth</a>
        <a class="nav-link" href="/ui/users">Users</a>
        <a class="nav-link" href="/ui/usage">Usage</a>
        <a class="nav-link active" href="/ui/budgets">Budgets</a>
        <a class="nav-link" href="/ui/settings">Settings</a>
    </div>
    <div class="nav-user">
        <span id="nav-username"></span>
        <button class="logout-btn" onclick="GAUD.logout()">Logout</button>
    </div>
</nav>
<div class="container">
    <div class="page-header">
        <h1>Budget Management</h1>
        <p>Configure spending limits per user</p>
    </div>

    <div id="budget-alert" class="alert hidden"></div>

    <div class="card">
        <div class="table-wrap">
            <table>
                <thead>
                    <tr>
                        <th>User</th>
                        <th>Monthly Limit</th>
                        <th>Monthly Used</th>
                        <th>Daily Limit</th>
                        <th>Daily Used</th>
                        <th>Usage</th>
                        <th>Actions</th>
                    </tr>
                </thead>
                <tbody id="budgets-table">
                    <tr><td colspan="7" class="text-muted">Loading...</td></tr>
                </tbody>
            </table>
        </div>
    </div>

    <!-- Edit budget modal (inline) -->
    <div id="edit-budget-form" class="card mt-2 hidden">
        <div class="card-header">Edit Budget</div>
        <form onsubmit="saveBudget(event)">
            <input type="hidden" id="edit-user-id">
            <p class="mb-1"><strong id="edit-user-name"></strong></p>
            <div class="flex gap-2 flex-wrap">
                <div class="form-group" style="flex:1;min-width:200px;">
                    <label class="form-label" for="edit-monthly-limit">Monthly Limit ($)</label>
                    <input class="form-input" type="number" step="0.01" id="edit-monthly-limit" placeholder="No limit">
                </div>
                <div class="form-group" style="flex:1;min-width:200px;">
                    <label class="form-label" for="edit-daily-limit">Daily Limit ($)</label>
                    <input class="form-input" type="number" step="0.01" id="edit-daily-limit" placeholder="No limit">
                </div>
            </div>
            <div class="flex gap-1">
                <button type="submit" class="btn btn-primary btn-sm">Save</button>
                <button type="button" class="btn btn-sm" onclick="hideEditBudget()">Cancel</button>
            </div>
        </form>
    </div>
</div>
{% endblock %}
{% block scripts %}
<script>
    if (!GAUD.requireAuth()) throw new Error('Not authenticated');

    function hideEditBudget() { document.getElementById('edit-budget-form').classList.add('hidden'); }

    function showEditBudget(userId, userName, monthlyLimit, dailyLimit) {
        document.getElementById('edit-user-id').value = userId;
        document.getElementById('edit-user-name').textContent = userName;
        document.getElementById('edit-monthly-limit').value = monthlyLimit || '';
        document.getElementById('edit-daily-limit').value = dailyLimit || '';
        document.getElementById('edit-budget-form').classList.remove('hidden');
    }

    async function loadBudgets() {
        try {
            const resp = await GAUD.apiFetch('/admin/budgets');
            if (!resp || !resp.ok) return;
            const budgets = await resp.json();

            let rows = '';
            for (const b of budgets) {
                const monthlyLimit = b.monthly_limit != null ? '$' + b.monthly_limit.toFixed(2) : 'None';
                const dailyLimit = b.daily_limit != null ? '$' + b.daily_limit.toFixed(2) : 'None';
                const monthlyUsed = '$' + (b.monthly_used || 0).toFixed(2);
                const dailyUsed = '$' + (b.daily_used || 0).toFixed(2);

                let pct = 0;
                let barClass = 'success';
                if (b.monthly_limit && b.monthly_limit > 0) {
                    pct = Math.min(100, ((b.monthly_used || 0) / b.monthly_limit) * 100);
                    if (pct >= 90) barClass = 'danger';
                    else if (pct >= 70) barClass = 'warning';
                }

                const progressBar = b.monthly_limit
                    ? '<div class="progress"><div class="progress-bar ' + barClass + '" style="width:' + pct.toFixed(0) + '%;"></div></div>'
                    : '<span class="text-muted" style="font-size:0.75rem;">No limit set</span>';

                rows += '<tr>' +
                    '<td><strong>' + (b.user_name || b.user_id) + '</strong></td>' +
                    '<td class="mono">' + monthlyLimit + '</td>' +
                    '<td class="mono">' + monthlyUsed + '</td>' +
                    '<td class="mono">' + dailyLimit + '</td>' +
                    '<td class="mono">' + dailyUsed + '</td>' +
                    '<td style="min-width:120px;">' + progressBar + '</td>' +
                    '<td><button class="btn btn-sm" onclick="showEditBudget(\'' +
                        b.user_id + "','" + (b.user_name || b.user_id) + "'," +
                        (b.monthly_limit || 'null') + ',' + (b.daily_limit || 'null') +
                    ')">Edit</button></td></tr>';
            }
            document.getElementById('budgets-table').innerHTML =
                rows || '<tr><td colspan="7" class="text-muted">No budgets configured. Budgets are created automatically when users make requests.</td></tr>';
        } catch (err) {
            console.error('Failed to load budgets:', err);
        }
    }

    async function saveBudget(e) {
        e.preventDefault();
        const userId = document.getElementById('edit-user-id').value;
        const monthlyLimit = document.getElementById('edit-monthly-limit').value;
        const dailyLimit = document.getElementById('edit-daily-limit').value;
        const alertEl = document.getElementById('budget-alert');

        try {
            const resp = await GAUD.apiFetch('/admin/budgets/' + userId, {
                method: 'PUT',
                body: JSON.stringify({
                    monthly_limit: monthlyLimit ? parseFloat(monthlyLimit) : null,
                    daily_limit: dailyLimit ? parseFloat(dailyLimit) : null,
                }),
            });
            if (!resp) return;
            if (resp.ok) {
                hideEditBudget();
                alertEl.className = 'alert alert-success';
                alertEl.textContent = 'Budget updated successfully.';
                alertEl.classList.remove('hidden');
                setTimeout(() => alertEl.classList.add('hidden'), 3000);
                loadBudgets();
            } else {
                const data = await resp.json();
                alertEl.className = 'alert alert-danger';
                alertEl.textContent = data.error?.message || 'Failed to save budget.';
                alertEl.classList.remove('hidden');
            }
        } catch (err) {
            alertEl.className = 'alert alert-danger';
            alertEl.textContent = err.message;
            alertEl.classList.remove('hidden');
        }
    }

    loadBudgets();
</script>
{% endblock %}"#;

/// Settings page template.
pub const SETTINGS: &str = r#"{% extends "layout" %}
{% block title %}Settings{% endblock %}
{% block body %}
<nav class="navbar">
    <a class="navbar-brand" href="/ui/dashboard"><span>gaud</span></a>
    <div class="nav-links">
        <a class="nav-link" href="/ui/dashboard">Dashboard</a>
        <a class="nav-link" href="/ui/oauth">OAuth</a>
        <a class="nav-link" href="/ui/users">Users</a>
        <a class="nav-link" href="/ui/usage">Usage</a>
        <a class="nav-link" href="/ui/budgets">Budgets</a>
        <a class="nav-link active" href="/ui/settings">Settings</a>
    </div>
    <div class="nav-user">
        <span id="nav-username"></span>
        <button class="logout-btn" onclick="GAUD.logout()">Logout</button>
    </div>
</nav>
<div class="container">
    <div class="page-header">
        <h1>Settings</h1>
        <p>View and edit server configuration. Changes require a restart to take effect.</p>
    </div>

    <div id="settings-alert" class="alert hidden"></div>
    <div id="restart-banner" class="alert alert-warning hidden" style="display:none;">
        <strong>Restart required.</strong> One or more settings have been saved. Restart the server for changes to take effect.
    </div>

    <div id="settings-container">
        <div class="card"><p class="text-muted">Loading settings...</p></div>
    </div>
</div>
{% endblock %}
{% block scripts %}
<script>
    if (!GAUD.requireAuth()) throw new Error('Not authenticated');

    const SECTION_ORDER = ['Server', 'Database', 'Authentication', 'Providers', 'LiteLLM', 'Budget', 'Logging'];
    let allSettings = [];
    let changedKeys = new Set();

    function showAlert(type, message) {
        const el = document.getElementById('settings-alert');
        el.className = 'alert alert-' + type;
        el.textContent = message;
        el.classList.remove('hidden');
        setTimeout(() => el.classList.add('hidden'), 5000);
    }

    function showRestartBanner() {
        const el = document.getElementById('restart-banner');
        el.style.display = '';
        el.classList.remove('hidden');
    }

    function maskValue(val) {
        if (val === null || val === undefined || val === '') return '';
        const s = String(val);
        if (s.length <= 4) return '****';
        return s.substring(0, 2) + '****' + s.substring(s.length - 2);
    }

    function renderInput(setting) {
        const disabled = setting.overridden ? ' disabled' : '';
        const dimClass = setting.overridden ? ' style="opacity:0.5;"' : '';
        const id = 'setting-' + setting.key.replace(/\./g, '-');
        let html = '';

        if (setting.input_type === 'bool') {
            const checked = setting.value === true ? ' checked' : '';
            html = '<label class="flex items-center gap-1"' + dimClass + '>' +
                '<input type="checkbox" id="' + id + '" data-key="' + setting.key + '"' +
                checked + disabled +
                ' onchange="markChanged(\'' + setting.key + '\')"' +
                ' style="width:18px;height:18px;accent-color:var(--accent);">' +
                '<span style="font-size:0.875rem;">' + (setting.value ? 'Enabled' : 'Disabled') + '</span>' +
                '</label>';
        } else if (setting.input_type === 'select') {
            html = '<select class="form-input" id="' + id + '" data-key="' + setting.key + '"' +
                disabled + dimClass +
                ' onchange="markChanged(\'' + setting.key + '\')">';
            if (setting.options) {
                for (const opt of setting.options) {
                    const selected = (String(setting.value) === opt) ? ' selected' : '';
                    html += '<option value="' + opt + '"' + selected + '>' + opt + '</option>';
                }
            }
            html += '</select>';
        } else if (setting.input_type === 'number') {
            const val = setting.sensitive ? '' : (setting.value !== null ? setting.value : '');
            html = '<input class="form-input mono" type="number" id="' + id + '"' +
                ' data-key="' + setting.key + '"' +
                ' value="' + val + '"' + disabled + dimClass +
                ' onchange="markChanged(\'' + setting.key + '\')">';
        } else {
            const val = setting.sensitive ? maskValue(setting.value) : (setting.value !== null ? setting.value : '');
            const inputType = setting.sensitive ? 'password' : 'text';
            html = '<input class="form-input mono" type="' + inputType + '" id="' + id + '"' +
                ' data-key="' + setting.key + '"' +
                ' value="' + val + '"' + disabled + dimClass +
                ' onchange="markChanged(\'' + setting.key + '\')">';
        }

        return html;
    }

    function renderOverrideBadge(setting) {
        if (!setting.overridden) return '';
        return '<span class="badge badge-warning" style="margin-left:0.5rem;" ' +
            'title="Unset this variable and restart to edit">' +
            'Set by ' + setting.env_var + '</span>';
    }

    function renderOverrideNote(setting) {
        if (!setting.overridden) return '';
        return '<p class="text-muted" style="font-size:0.75rem;margin-top:0.25rem;">' +
            'Unset <code>' + setting.env_var + '</code> and restart to edit this setting.</p>';
    }

    function renderSettings(settings) {
        allSettings = settings;
        const grouped = {};
        for (const s of settings) {
            if (!grouped[s.section]) grouped[s.section] = [];
            grouped[s.section].push(s);
        }

        const container = document.getElementById('settings-container');
        let html = '';

        for (const section of SECTION_ORDER) {
            const items = grouped[section];
            if (!items || items.length === 0) continue;

            html += '<div class="card mb-2">';
            html += '<div class="card-header flex justify-between items-center">' +
                '<span>' + section + '</span>' +
                '<button class="btn btn-sm btn-primary" onclick="saveSection(\'' + section + '\')" ' +
                'id="save-btn-' + section.replace(/\s/g, '-') + '">Save ' + section + '</button>' +
                '</div>';

            for (const s of items) {
                html += '<div class="form-group">';
                html += '<label class="form-label" for="setting-' + s.key.replace(/\./g, '-') + '">' +
                    s.label + renderOverrideBadge(s) + '</label>';
                html += '<div style="font-size:0.75rem;color:var(--text-muted);margin-bottom:0.25rem;">' +
                    '<code>' + s.key + '</code>' +
                    ' &middot; env: <code>' + s.env_var + '</code></div>';
                html += renderInput(s);
                html += renderOverrideNote(s);
                html += '</div>';
            }

            html += '</div>';
        }

        container.innerHTML = html || '<div class="card"><div class="empty-state"><p>No settings available.</p></div></div>';
    }

    function markChanged(key) {
        changedKeys.add(key);
    }

    function getInputValue(setting) {
        const id = 'setting-' + setting.key.replace(/\./g, '-');
        const el = document.getElementById(id);
        if (!el) return null;

        if (setting.input_type === 'bool') {
            return el.checked;
        } else if (setting.input_type === 'number') {
            const num = Number(el.value);
            return isNaN(num) ? el.value : num;
        } else {
            return el.value;
        }
    }

    async function saveSection(section) {
        const sectionSettings = allSettings.filter(s => s.section === section && !s.overridden);
        const changed = sectionSettings.filter(s => changedKeys.has(s.key));

        if (changed.length === 0) {
            showAlert('info', 'No changes in ' + section + ' section.');
            return;
        }

        let errorCount = 0;
        let successCount = 0;

        for (const s of changed) {
            const value = getInputValue(s);
            if (value === null) continue;

            try {
                const resp = await GAUD.apiFetch('/admin/settings', {
                    method: 'PUT',
                    body: JSON.stringify({ key: s.key, value: value }),
                });
                if (!resp) return;
                if (resp.ok) {
                    successCount++;
                    changedKeys.delete(s.key);
                } else {
                    const data = await resp.json();
                    const msg = (data.error && data.error.message) ? data.error.message : 'Failed to save ' + s.key;
                    showAlert('danger', msg);
                    errorCount++;
                }
            } catch (err) {
                showAlert('danger', 'Error saving ' + s.key + ': ' + err.message);
                errorCount++;
            }
        }

        if (successCount > 0 && errorCount === 0) {
            showAlert('success', successCount + ' setting(s) in ' + section + ' saved successfully.');
            showRestartBanner();
        } else if (successCount > 0 && errorCount > 0) {
            showAlert('warning', successCount + ' saved, ' + errorCount + ' failed in ' + section + '.');
            showRestartBanner();
        }
    }

    async function loadSettings() {
        try {
            const resp = await GAUD.apiFetch('/admin/settings');
            if (!resp || !resp.ok) {
                if (resp && resp.status === 403) {
                    showAlert('danger', 'Admin access required to view settings.');
                }
                return;
            }
            const settings = await resp.json();
            renderSettings(settings);
        } catch (err) {
            console.error('Failed to load settings:', err);
            showAlert('danger', 'Failed to load settings: ' + err.message);
        }
    }

    loadSettings();
</script>
{% endblock %}"#;
