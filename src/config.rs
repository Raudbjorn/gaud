use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::str::FromStr;

// ---------------------------------------------------------------------------
// Environment override tracking
// ---------------------------------------------------------------------------

/// Tracks which configuration settings are overridden by environment variables.
///
/// When a setting is overridden by an env var, the web UI should display that
/// setting's input as disabled with a visual indicator showing the env var name.
#[derive(Debug, Clone, Default)]
pub struct EnvOverrides {
    overrides: HashMap<String, String>,
}

impl EnvOverrides {
    /// Check whether a setting key (e.g. "server.host") is overridden by an env var.
    pub fn is_overridden(&self, key: &str) -> bool {
        self.overrides.contains_key(key)
    }

    /// Get the env var name that overrides the given setting key.
    pub fn env_var_for(&self, key: &str) -> Option<&str> {
        self.overrides.get(key).map(String::as_str)
    }

    /// Get all overrides as a map of setting key -> env var name.
    pub fn all(&self) -> &HashMap<String, String> {
        &self.overrides
    }

    fn record(&mut self, key: &str, env_var: &str) {
        self.overrides.insert(key.to_string(), env_var.to_string());
    }
}

// ---------------------------------------------------------------------------
// Settings report entry (for web UI)
// ---------------------------------------------------------------------------

/// A single setting entry for the web UI settings page.
#[derive(Debug, Clone, Serialize)]
pub struct SettingEntry {
    /// Dotted key path (e.g. "server.host").
    pub key: String,
    /// Human-readable section name (e.g. "Server").
    pub section: String,
    /// Human-readable label (e.g. "Bind Address").
    pub label: String,
    /// Current effective value.
    pub value: serde_json::Value,
    /// The env var that can override this setting.
    pub env_var: String,
    /// Whether the setting is currently overridden by the env var.
    pub overridden: bool,
    /// HTML input type: "text", "number", "bool", "select".
    pub input_type: String,
    /// For select inputs, the available options.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub options: Option<Vec<String>>,
    /// Whether the value is sensitive (should be masked in UI).
    #[serde(default)]
    pub sensitive: bool,
}

// ---------------------------------------------------------------------------
// Main configuration
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Config {
    #[serde(default)]
    pub server: ServerConfig,
    #[serde(default)]
    pub database: DatabaseConfig,
    #[serde(default)]
    pub auth: AuthConfig,
    #[serde(default)]
    pub providers: ProvidersConfig,
    #[serde(default)]
    pub budget: BudgetConfig,
    #[serde(default)]
    pub logging: LoggingConfig,
    /// Env var overrides are not serialized to TOML.
    #[serde(skip)]
    pub env_overrides: EnvOverrides,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ServerConfig {
    #[serde(default = "default_host")]
    pub host: String,
    #[serde(default = "default_port")]
    pub port: u16,
    #[serde(default)]
    pub cors_origins: Vec<String>,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            host: default_host(),
            port: default_port(),
            cors_origins: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct DatabaseConfig {
    #[serde(default = "default_db_path")]
    pub path: PathBuf,
}

impl Default for DatabaseConfig {
    fn default() -> Self {
        Self {
            path: default_db_path(),
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct AuthConfig {
    /// Master switch: when false, all API routes are accessible without auth.
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_admin_name")]
    pub default_admin_name: String,
    #[serde(default)]
    pub bootstrap_key: Option<String>,
    /// TLS client certificate authentication.
    #[serde(default)]
    pub tls_client_cert: TlsClientCertConfig,
}

impl Default for AuthConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            default_admin_name: default_admin_name(),
            bootstrap_key: None,
            tls_client_cert: TlsClientCertConfig::default(),
        }
    }
}

/// TLS client certificate authentication configuration.
///
/// When a TLS-terminating reverse proxy (nginx, envoy, etc.) is in front of
/// gaud, it can pass the client certificate common name via a header. This
/// section configures gaud to trust that header for authentication.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct TlsClientCertConfig {
    /// Enable TLS client cert header-based auth.
    #[serde(default)]
    pub enabled: bool,
    /// Path to the CA certificate used to verify client certs (informational
    /// only; the actual verification is done by the reverse proxy).
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ca_cert_path: Option<String>,
    /// When true, requests without a valid client cert header are rejected.
    /// When false, client cert auth is optional and falls back to API key auth.
    #[serde(default)]
    pub require_cert: bool,
    /// Header name containing the client certificate CN. Defaults to
    /// "X-Client-Cert-CN" if not specified.
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub header_name: Option<String>,
}

impl Default for TlsClientCertConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            ca_cert_path: None,
            require_cert: false,
            header_name: None,
        }
    }
}

impl TlsClientCertConfig {
    /// The effective header name (defaults to X-Client-Cert-CN).
    pub fn effective_header(&self) -> &str {
        self.header_name
            .as_deref()
            .unwrap_or("X-Client-Cert-CN")
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct ProvidersConfig {
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub claude: Option<ClaudeProviderConfig>,
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gemini: Option<GeminiProviderConfig>,
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub copilot: Option<CopilotProviderConfig>,
    #[serde(default)]
    pub routing_strategy: RoutingStrategy,
    #[serde(default = "default_token_storage_dir")]
    pub token_storage_dir: PathBuf,
    #[serde(default = "default_storage_backend")]
    pub storage_backend: StorageBackend,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ClaudeProviderConfig {
    pub client_id: String,
    #[serde(default = "default_claude_auth_url")]
    pub auth_url: String,
    #[serde(default = "default_claude_token_url")]
    pub token_url: String,
    #[serde(default = "default_claude_callback_port")]
    pub callback_port: u16,
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_model: Option<String>,
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct GeminiProviderConfig {
    pub client_id: String,
    pub client_secret: String,
    #[serde(default = "default_gemini_auth_url")]
    pub auth_url: String,
    #[serde(default = "default_gemini_token_url")]
    pub token_url: String,
    #[serde(default = "default_gemini_callback_port")]
    pub callback_port: u16,
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_model: Option<String>,
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct CopilotProviderConfig {
    #[serde(default = "default_copilot_client_id")]
    pub client_id: String,
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_model: Option<String>,
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
}

#[derive(Debug, Clone, Deserialize, Serialize, Default, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum RoutingStrategy {
    #[default]
    Priority,
    RoundRobin,
    LeastUsed,
    Random,
}

impl std::fmt::Display for RoutingStrategy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Priority => write!(f, "priority"),
            Self::RoundRobin => write!(f, "round_robin"),
            Self::LeastUsed => write!(f, "least_used"),
            Self::Random => write!(f, "random"),
        }
    }
}

impl FromStr for RoutingStrategy {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().replace('-', "_").as_str() {
            "priority" => Ok(Self::Priority),
            "round_robin" | "roundrobin" => Ok(Self::RoundRobin),
            "least_used" | "leastused" => Ok(Self::LeastUsed),
            "random" => Ok(Self::Random),
            _ => Err(format!("Unknown routing strategy: {s}")),
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, Default, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum StorageBackend {
    #[default]
    File,
    Keyring,
    Memory,
}

impl std::fmt::Display for StorageBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::File => write!(f, "file"),
            Self::Keyring => write!(f, "keyring"),
            Self::Memory => write!(f, "memory"),
        }
    }
}

impl FromStr for StorageBackend {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "file" => Ok(Self::File),
            "keyring" => Ok(Self::Keyring),
            "memory" => Ok(Self::Memory),
            _ => Err(format!("Unknown storage backend: {s}")),
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct BudgetConfig {
    #[serde(default = "default_budget_check_enabled")]
    pub enabled: bool,
    #[serde(default = "default_warning_threshold")]
    pub warning_threshold_percent: u8,
}

impl Default for BudgetConfig {
    fn default() -> Self {
        Self {
            enabled: default_budget_check_enabled(),
            warning_threshold_percent: default_warning_threshold(),
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct LoggingConfig {
    #[serde(default = "default_log_level")]
    pub level: String,
    #[serde(default)]
    pub json: bool,
    #[serde(default)]
    pub log_content: bool,
}

impl Default for LoggingConfig {
    fn default() -> Self {
        Self {
            level: default_log_level(),
            json: false,
            log_content: false,
        }
    }
}

// ---------------------------------------------------------------------------
// Default value functions
// ---------------------------------------------------------------------------

const fn default_port() -> u16 {
    8400
}
fn default_host() -> String {
    "127.0.0.1".to_string()
}
fn default_db_path() -> PathBuf {
    PathBuf::from("gaud.db")
}
fn default_admin_name() -> String {
    "admin".to_string()
}
const fn default_true() -> bool {
    true
}
fn default_token_storage_dir() -> PathBuf {
    dirs_default_token_storage()
}
fn dirs_default_token_storage() -> PathBuf {
    dirs::data_local_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("gaud")
        .join("tokens")
}
fn default_storage_backend() -> StorageBackend {
    StorageBackend::File
}
fn default_claude_auth_url() -> String {
    "https://console.anthropic.com/oauth/authorize".to_string()
}
fn default_claude_token_url() -> String {
    "https://console.anthropic.com/v1/oauth/token".to_string()
}
const fn default_claude_callback_port() -> u16 {
    19284
}
fn default_gemini_auth_url() -> String {
    "https://accounts.google.com/o/oauth2/v2/auth".to_string()
}
fn default_gemini_token_url() -> String {
    "https://oauth2.googleapis.com/token".to_string()
}
const fn default_gemini_callback_port() -> u16 {
    19285
}
fn default_copilot_client_id() -> String {
    "Iv1.b507a08c87ecfe98".to_string()
}
const fn default_budget_check_enabled() -> bool {
    true
}
const fn default_warning_threshold() -> u8 {
    80
}
fn default_log_level() -> String {
    "info".to_string()
}

// ---------------------------------------------------------------------------
// Config loading, env overrides, and settings report
// ---------------------------------------------------------------------------

impl Config {
    /// Load configuration from a TOML file, then apply environment variable
    /// overrides. Any setting prefixed with `GAUD_` takes precedence over the
    /// file value and is tracked in `env_overrides`.
    pub fn load(path: &Path) -> anyhow::Result<Self> {
        let mut config = if path.exists() {
            let content = std::fs::read_to_string(path)?;
            let config: Config = toml::from_str(&content)?;
            config
        } else {
            tracing::warn!("Config file not found at {}, using defaults", path.display());
            Self::default()
        };
        config.apply_env_overrides();
        Ok(config)
    }

    pub fn listen_addr(&self) -> String {
        format!("{}:{}", self.server.host, self.server.port)
    }

    /// Save the current (file-level) configuration to a TOML file.
    /// This serializes the config without env overrides applied.
    pub fn save(&self, path: &Path) -> anyhow::Result<()> {
        let content = toml::to_string_pretty(self)
            .map_err(|e| anyhow::anyhow!("Failed to serialize config: {e}"))?;
        std::fs::write(path, content)?;
        Ok(())
    }

    /// Apply environment variable overrides to the configuration.
    ///
    /// Every supported setting has a corresponding `GAUD_*` env var. When set,
    /// the env var value replaces the file/default value and the setting key is
    /// recorded in `env_overrides` so the web UI can display it as locked.
    fn apply_env_overrides(&mut self) {
        let mut ov = EnvOverrides::default();

        // -- Helpers (macros for concise per-field overrides) --

        macro_rules! env_str {
            ($key:expr, $env:expr, $field:expr) => {
                if let Ok(val) = std::env::var($env) {
                    $field = val;
                    ov.record($key, $env);
                }
            };
        }
        macro_rules! env_bool {
            ($key:expr, $env:expr, $field:expr) => {
                if let Ok(val) = std::env::var($env) {
                    $field = matches!(val.to_lowercase().as_str(), "1" | "true" | "yes" | "on");
                    ov.record($key, $env);
                }
            };
        }
        macro_rules! env_parse {
            ($key:expr, $env:expr, $field:expr) => {
                if let Ok(val) = std::env::var($env) {
                    if let Ok(parsed) = val.parse() {
                        $field = parsed;
                        ov.record($key, $env);
                    }
                }
            };
        }
        macro_rules! env_path {
            ($key:expr, $env:expr, $field:expr) => {
                if let Ok(val) = std::env::var($env) {
                    $field = PathBuf::from(val);
                    ov.record($key, $env);
                }
            };
        }
        macro_rules! env_opt_str {
            ($key:expr, $env:expr, $field:expr) => {
                if let Ok(val) = std::env::var($env) {
                    $field = if val.is_empty() { None } else { Some(val) };
                    ov.record($key, $env);
                }
            };
        }

        // -- Server --
        env_str!("server.host", "GAUD_SERVER_HOST", self.server.host);
        env_parse!("server.port", "GAUD_SERVER_PORT", self.server.port);
        if let Ok(val) = std::env::var("GAUD_SERVER_CORS_ORIGINS") {
            self.server.cors_origins = val
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();
            ov.record("server.cors_origins", "GAUD_SERVER_CORS_ORIGINS");
        }

        // -- Database --
        env_path!("database.path", "GAUD_DATABASE_PATH", self.database.path);

        // -- Auth --
        env_bool!("auth.enabled", "GAUD_AUTH_ENABLED", self.auth.enabled);
        env_str!(
            "auth.default_admin_name",
            "GAUD_AUTH_ADMIN_NAME",
            self.auth.default_admin_name
        );
        env_opt_str!(
            "auth.bootstrap_key",
            "GAUD_AUTH_BOOTSTRAP_KEY",
            self.auth.bootstrap_key
        );
        env_bool!(
            "auth.tls_client_cert.enabled",
            "GAUD_AUTH_TLS_ENABLED",
            self.auth.tls_client_cert.enabled
        );
        env_opt_str!(
            "auth.tls_client_cert.ca_cert_path",
            "GAUD_AUTH_TLS_CA_CERT",
            self.auth.tls_client_cert.ca_cert_path
        );
        env_bool!(
            "auth.tls_client_cert.require_cert",
            "GAUD_AUTH_TLS_REQUIRE",
            self.auth.tls_client_cert.require_cert
        );
        env_opt_str!(
            "auth.tls_client_cert.header_name",
            "GAUD_AUTH_TLS_HEADER",
            self.auth.tls_client_cert.header_name
        );

        // -- Providers --
        if let Ok(val) = std::env::var("GAUD_PROVIDERS_ROUTING") {
            if let Ok(strategy) = val.parse() {
                self.providers.routing_strategy = strategy;
                ov.record("providers.routing_strategy", "GAUD_PROVIDERS_ROUTING");
            }
        }
        env_path!(
            "providers.token_storage_dir",
            "GAUD_PROVIDERS_TOKEN_DIR",
            self.providers.token_storage_dir
        );
        if let Ok(val) = std::env::var("GAUD_PROVIDERS_STORAGE_BACKEND") {
            if let Ok(backend) = val.parse() {
                self.providers.storage_backend = backend;
                ov.record(
                    "providers.storage_backend",
                    "GAUD_PROVIDERS_STORAGE_BACKEND",
                );
            }
        }

        // -- Budget --
        env_bool!("budget.enabled", "GAUD_BUDGET_ENABLED", self.budget.enabled);
        env_parse!(
            "budget.warning_threshold_percent",
            "GAUD_BUDGET_WARNING_THRESHOLD",
            self.budget.warning_threshold_percent
        );

        // -- Logging --
        env_str!("logging.level", "GAUD_LOG_LEVEL", self.logging.level);
        env_bool!("logging.json", "GAUD_LOG_JSON", self.logging.json);
        env_bool!(
            "logging.log_content",
            "GAUD_LOG_CONTENT",
            self.logging.log_content
        );

        self.env_overrides = ov;
    }

    /// Produce a list of setting entries for the web UI settings page.
    ///
    /// Each entry includes the current effective value, the corresponding env
    /// var name, whether it is currently overridden, and metadata for the input
    /// control to render.
    pub fn settings_report(&self) -> Vec<SettingEntry> {
        let ov = &self.env_overrides;

        let se = |key: &str,
                  section: &str,
                  label: &str,
                  value: serde_json::Value,
                  env_var: &str,
                  input_type: &str|
         -> SettingEntry {
            SettingEntry {
                key: key.to_string(),
                section: section.to_string(),
                label: label.to_string(),
                value,
                env_var: env_var.to_string(),
                overridden: ov.is_overridden(key),
                input_type: input_type.to_string(),
                options: None,
                sensitive: false,
            }
        };

        let mut entries = vec![
            // -- Server --
            se("server.host", "Server", "Bind Address", serde_json::json!(self.server.host), "GAUD_SERVER_HOST", "text"),
            se("server.port", "Server", "Port", serde_json::json!(self.server.port), "GAUD_SERVER_PORT", "number"),
            se("server.cors_origins", "Server", "CORS Origins", serde_json::json!(self.server.cors_origins.join(", ")), "GAUD_SERVER_CORS_ORIGINS", "text"),
            // -- Database --
            se("database.path", "Database", "Database Path", serde_json::json!(self.database.path.display().to_string()), "GAUD_DATABASE_PATH", "text"),
            // -- Auth --
            se("auth.enabled", "Authentication", "Auth Enabled", serde_json::json!(self.auth.enabled), "GAUD_AUTH_ENABLED", "bool"),
            se("auth.default_admin_name", "Authentication", "Default Admin Name", serde_json::json!(self.auth.default_admin_name), "GAUD_AUTH_ADMIN_NAME", "text"),
            se("auth.tls_client_cert.enabled", "Authentication", "TLS Client Cert Auth", serde_json::json!(self.auth.tls_client_cert.enabled), "GAUD_AUTH_TLS_ENABLED", "bool"),
            se("auth.tls_client_cert.require_cert", "Authentication", "Require Client Cert", serde_json::json!(self.auth.tls_client_cert.require_cert), "GAUD_AUTH_TLS_REQUIRE", "bool"),
            se("auth.tls_client_cert.ca_cert_path", "Authentication", "CA Cert Path", serde_json::json!(self.auth.tls_client_cert.ca_cert_path.as_deref().unwrap_or("")), "GAUD_AUTH_TLS_CA_CERT", "text"),
            se("auth.tls_client_cert.header_name", "Authentication", "Client Cert Header", serde_json::json!(self.auth.tls_client_cert.effective_header()), "GAUD_AUTH_TLS_HEADER", "text"),
            // -- Providers --
            {
                let mut e = se("providers.routing_strategy", "Providers", "Routing Strategy", serde_json::json!(self.providers.routing_strategy.to_string()), "GAUD_PROVIDERS_ROUTING", "select");
                e.options = Some(vec![
                    "priority".to_string(),
                    "round_robin".to_string(),
                    "least_used".to_string(),
                    "random".to_string(),
                ]);
                e
            },
            se("providers.token_storage_dir", "Providers", "Token Storage Directory", serde_json::json!(self.providers.token_storage_dir.display().to_string()), "GAUD_PROVIDERS_TOKEN_DIR", "text"),
            {
                let mut e = se("providers.storage_backend", "Providers", "Storage Backend", serde_json::json!(self.providers.storage_backend.to_string()), "GAUD_PROVIDERS_STORAGE_BACKEND", "select");
                e.options = Some(vec![
                    "file".to_string(),
                    "keyring".to_string(),
                    "memory".to_string(),
                ]);
                e
            },
            // -- Budget --
            se("budget.enabled", "Budget", "Budget Tracking Enabled", serde_json::json!(self.budget.enabled), "GAUD_BUDGET_ENABLED", "bool"),
            se("budget.warning_threshold_percent", "Budget", "Warning Threshold (%)", serde_json::json!(self.budget.warning_threshold_percent), "GAUD_BUDGET_WARNING_THRESHOLD", "number"),
            // -- Logging --
            {
                let mut e = se("logging.level", "Logging", "Log Level", serde_json::json!(self.logging.level), "GAUD_LOG_LEVEL", "select");
                e.options = Some(vec![
                    "trace".to_string(),
                    "debug".to_string(),
                    "info".to_string(),
                    "warn".to_string(),
                    "error".to_string(),
                ]);
                e
            },
            se("logging.json", "Logging", "JSON Log Format", serde_json::json!(self.logging.json), "GAUD_LOG_JSON", "bool"),
            se("logging.log_content", "Logging", "Log Request Content", serde_json::json!(self.logging.log_content), "GAUD_LOG_CONTENT", "bool"),
        ];

        // Mark bootstrap_key as sensitive.
        let mut bk = se(
            "auth.bootstrap_key",
            "Authentication",
            "Bootstrap Key",
            serde_json::json!(self.auth.bootstrap_key.as_deref().map(|_| "********").unwrap_or("")),
            "GAUD_AUTH_BOOTSTRAP_KEY",
            "text",
        );
        bk.sensitive = true;
        entries.insert(6, bk);

        entries
    }

    /// Update a single setting from a key-value pair (for the settings API).
    ///
    /// Returns `Err` if the key is unknown or the value cannot be parsed.
    pub fn update_setting(&mut self, key: &str, value: &serde_json::Value) -> Result<(), String> {
        match key {
            "server.host" => {
                self.server.host = value.as_str().ok_or("Expected string")?.to_string();
            }
            "server.port" => {
                self.server.port = value
                    .as_u64()
                    .ok_or("Expected number")?
                    .try_into()
                    .map_err(|_| "Port out of range")?;
            }
            "server.cors_origins" => {
                let s = value.as_str().ok_or("Expected string")?;
                self.server.cors_origins = s
                    .split(',')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect();
            }
            "database.path" => {
                self.database.path =
                    PathBuf::from(value.as_str().ok_or("Expected string")?);
            }
            "auth.enabled" => {
                self.auth.enabled = value.as_bool().ok_or("Expected boolean")?;
            }
            "auth.default_admin_name" => {
                self.auth.default_admin_name =
                    value.as_str().ok_or("Expected string")?.to_string();
            }
            "auth.tls_client_cert.enabled" => {
                self.auth.tls_client_cert.enabled =
                    value.as_bool().ok_or("Expected boolean")?;
            }
            "auth.tls_client_cert.require_cert" => {
                self.auth.tls_client_cert.require_cert =
                    value.as_bool().ok_or("Expected boolean")?;
            }
            "auth.tls_client_cert.ca_cert_path" => {
                let s = value.as_str().ok_or("Expected string")?;
                self.auth.tls_client_cert.ca_cert_path = if s.is_empty() {
                    None
                } else {
                    Some(s.to_string())
                };
            }
            "auth.tls_client_cert.header_name" => {
                let s = value.as_str().ok_or("Expected string")?;
                self.auth.tls_client_cert.header_name = if s.is_empty() {
                    None
                } else {
                    Some(s.to_string())
                };
            }
            "providers.routing_strategy" => {
                let s = value.as_str().ok_or("Expected string")?;
                self.providers.routing_strategy =
                    s.parse().map_err(|e: String| e)?;
            }
            "providers.token_storage_dir" => {
                self.providers.token_storage_dir =
                    PathBuf::from(value.as_str().ok_or("Expected string")?);
            }
            "providers.storage_backend" => {
                let s = value.as_str().ok_or("Expected string")?;
                self.providers.storage_backend =
                    s.parse().map_err(|e: String| e)?;
            }
            "budget.enabled" => {
                self.budget.enabled = value.as_bool().ok_or("Expected boolean")?;
            }
            "budget.warning_threshold_percent" => {
                self.budget.warning_threshold_percent = value
                    .as_u64()
                    .ok_or("Expected number")?
                    .try_into()
                    .map_err(|_| "Value out of range")?;
            }
            "logging.level" => {
                self.logging.level =
                    value.as_str().ok_or("Expected string")?.to_string();
            }
            "logging.json" => {
                self.logging.json = value.as_bool().ok_or("Expected boolean")?;
            }
            "logging.log_content" => {
                self.logging.log_content = value.as_bool().ok_or("Expected boolean")?;
            }
            _ => return Err(format!("Unknown setting key: {key}")),
        }
        Ok(())
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            server: ServerConfig::default(),
            database: DatabaseConfig::default(),
            auth: AuthConfig::default(),
            providers: ProvidersConfig::default(),
            budget: BudgetConfig::default(),
            logging: LoggingConfig::default(),
            env_overrides: EnvOverrides::default(),
        }
    }
}

// Helper for default token storage directory
mod dirs {
    use std::path::PathBuf;

    pub fn data_local_dir() -> Option<PathBuf> {
        std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .or_else(|| {
                std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local").join("share"))
            })
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = Config::default();
        assert_eq!(config.server.host, "127.0.0.1");
        assert_eq!(config.server.port, 8400);
        assert!(config.auth.enabled);
        assert!(!config.auth.tls_client_cert.enabled);
        assert!(!config.auth.tls_client_cert.require_cert);
        assert!(config.budget.enabled);
        assert_eq!(config.budget.warning_threshold_percent, 80);
        assert_eq!(config.logging.level, "info");
        assert!(!config.logging.json);
    }

    #[test]
    fn test_tls_client_cert_effective_header_default() {
        let tls = TlsClientCertConfig::default();
        assert_eq!(tls.effective_header(), "X-Client-Cert-CN");
    }

    #[test]
    fn test_tls_client_cert_effective_header_custom() {
        let tls = TlsClientCertConfig {
            header_name: Some("X-SSL-Client-CN".to_string()),
            ..Default::default()
        };
        assert_eq!(tls.effective_header(), "X-SSL-Client-CN");
    }

    #[test]
    fn test_routing_strategy_from_str() {
        assert_eq!("priority".parse::<RoutingStrategy>().unwrap(), RoutingStrategy::Priority);
        assert_eq!("round_robin".parse::<RoutingStrategy>().unwrap(), RoutingStrategy::RoundRobin);
        assert_eq!("round-robin".parse::<RoutingStrategy>().unwrap(), RoutingStrategy::RoundRobin);
        assert_eq!("least_used".parse::<RoutingStrategy>().unwrap(), RoutingStrategy::LeastUsed);
        assert_eq!("random".parse::<RoutingStrategy>().unwrap(), RoutingStrategy::Random);
        assert!("unknown".parse::<RoutingStrategy>().is_err());
    }

    #[test]
    fn test_routing_strategy_display() {
        assert_eq!(RoutingStrategy::Priority.to_string(), "priority");
        assert_eq!(RoutingStrategy::RoundRobin.to_string(), "round_robin");
        assert_eq!(RoutingStrategy::LeastUsed.to_string(), "least_used");
        assert_eq!(RoutingStrategy::Random.to_string(), "random");
    }

    #[test]
    fn test_storage_backend_from_str() {
        assert_eq!("file".parse::<StorageBackend>().unwrap(), StorageBackend::File);
        assert_eq!("keyring".parse::<StorageBackend>().unwrap(), StorageBackend::Keyring);
        assert_eq!("memory".parse::<StorageBackend>().unwrap(), StorageBackend::Memory);
        assert!("unknown".parse::<StorageBackend>().is_err());
    }

    #[test]
    fn test_storage_backend_display() {
        assert_eq!(StorageBackend::File.to_string(), "file");
        assert_eq!(StorageBackend::Keyring.to_string(), "keyring");
        assert_eq!(StorageBackend::Memory.to_string(), "memory");
    }

    #[test]
    fn test_env_overrides_tracking() {
        let mut ov = EnvOverrides::default();
        assert!(!ov.is_overridden("server.host"));
        assert!(ov.env_var_for("server.host").is_none());

        ov.record("server.host", "GAUD_SERVER_HOST");
        assert!(ov.is_overridden("server.host"));
        assert_eq!(ov.env_var_for("server.host"), Some("GAUD_SERVER_HOST"));
        assert!(!ov.is_overridden("server.port"));
        assert_eq!(ov.all().len(), 1);
    }

    #[test]
    fn test_env_override_applies() {
        // Set an env var, load config, verify it's applied and tracked.
        // SAFETY: Tests are run sequentially for env-mutating tests.
        unsafe {
            std::env::set_var("GAUD_SERVER_PORT", "9999");
            std::env::set_var("GAUD_AUTH_ENABLED", "false");
            std::env::set_var("GAUD_LOG_LEVEL", "debug");
        }

        let mut config = Config::default();
        config.apply_env_overrides();

        assert_eq!(config.server.port, 9999);
        assert!(!config.auth.enabled);
        assert_eq!(config.logging.level, "debug");

        assert!(config.env_overrides.is_overridden("server.port"));
        assert!(config.env_overrides.is_overridden("auth.enabled"));
        assert!(config.env_overrides.is_overridden("logging.level"));
        assert!(!config.env_overrides.is_overridden("server.host"));

        // Clean up env.
        unsafe {
            std::env::remove_var("GAUD_SERVER_PORT");
            std::env::remove_var("GAUD_AUTH_ENABLED");
            std::env::remove_var("GAUD_LOG_LEVEL");
        }
    }

    #[test]
    fn test_env_bool_variants() {
        for (val, expected) in [
            ("1", true),
            ("true", true),
            ("yes", true),
            ("on", true),
            ("0", false),
            ("false", false),
            ("no", false),
            ("off", false),
        ] {
            // SAFETY: Tests are run sequentially for env-mutating tests.
            unsafe { std::env::set_var("GAUD_LOG_JSON", val); }
            let mut config = Config::default();
            config.apply_env_overrides();
            assert_eq!(config.logging.json, expected, "GAUD_LOG_JSON={val}");
        }
        unsafe { std::env::remove_var("GAUD_LOG_JSON"); }
    }

    #[test]
    fn test_env_cors_origins_split() {
        // SAFETY: Tests are run sequentially for env-mutating tests.
        unsafe { std::env::set_var("GAUD_SERVER_CORS_ORIGINS", "http://a.com, http://b.com, http://c.com"); }
        let mut config = Config::default();
        config.apply_env_overrides();
        assert_eq!(config.server.cors_origins, vec!["http://a.com", "http://b.com", "http://c.com"]);
        unsafe { std::env::remove_var("GAUD_SERVER_CORS_ORIGINS"); }
    }

    #[test]
    fn test_settings_report_completeness() {
        let config = Config::default();
        let report = config.settings_report();

        // Verify all major sections are present.
        let sections: Vec<&str> = report.iter().map(|e| e.section.as_str()).collect();
        assert!(sections.contains(&"Server"));
        assert!(sections.contains(&"Database"));
        assert!(sections.contains(&"Authentication"));
        assert!(sections.contains(&"Providers"));
        assert!(sections.contains(&"Budget"));
        assert!(sections.contains(&"Logging"));

        // Verify env var names are set.
        for entry in &report {
            assert!(!entry.env_var.is_empty(), "entry {} missing env_var", entry.key);
        }

        // Verify bootstrap_key is sensitive.
        let bk = report.iter().find(|e| e.key == "auth.bootstrap_key").unwrap();
        assert!(bk.sensitive);
    }

    #[test]
    fn test_settings_report_env_override_flag() {
        // SAFETY: Tests are run sequentially for env-mutating tests.
        unsafe { std::env::set_var("GAUD_SERVER_HOST", "0.0.0.0"); }
        let mut config = Config::default();
        config.apply_env_overrides();
        let report = config.settings_report();

        let host = report.iter().find(|e| e.key == "server.host").unwrap();
        assert!(host.overridden);
        assert_eq!(host.value.as_str().unwrap(), "0.0.0.0");

        let port = report.iter().find(|e| e.key == "server.port").unwrap();
        assert!(!port.overridden);

        unsafe { std::env::remove_var("GAUD_SERVER_HOST"); }
    }

    #[test]
    fn test_update_setting_valid() {
        let mut config = Config::default();
        config.update_setting("server.host", &serde_json::json!("0.0.0.0")).unwrap();
        assert_eq!(config.server.host, "0.0.0.0");

        config.update_setting("server.port", &serde_json::json!(9090)).unwrap();
        assert_eq!(config.server.port, 9090);

        config.update_setting("auth.enabled", &serde_json::json!(false)).unwrap();
        assert!(!config.auth.enabled);

        config.update_setting("logging.level", &serde_json::json!("debug")).unwrap();
        assert_eq!(config.logging.level, "debug");

        config
            .update_setting("providers.routing_strategy", &serde_json::json!("round_robin"))
            .unwrap();
        assert_eq!(config.providers.routing_strategy, RoutingStrategy::RoundRobin);
    }

    #[test]
    fn test_update_setting_unknown_key() {
        let mut config = Config::default();
        let result = config.update_setting("nonexistent.key", &serde_json::json!("x"));
        assert!(result.is_err());
    }

    #[test]
    fn test_update_setting_wrong_type() {
        let mut config = Config::default();
        let result = config.update_setting("server.port", &serde_json::json!("not_a_number"));
        assert!(result.is_err());
    }

    #[test]
    fn test_config_serialization_roundtrip() {
        let config = Config::default();
        let toml_str = toml::to_string_pretty(&config).unwrap();
        let parsed: Config = toml::from_str(&toml_str).unwrap();
        assert_eq!(parsed.server.host, config.server.host);
        assert_eq!(parsed.server.port, config.server.port);
        assert_eq!(parsed.auth.enabled, config.auth.enabled);
    }

    #[test]
    fn test_listen_addr() {
        let config = Config::default();
        assert_eq!(config.listen_addr(), "127.0.0.1:8400");
    }

    #[test]
    fn test_config_load_missing_file() {
        let path = Path::new("/tmp/nonexistent_gaud_config_test.toml");
        let config = Config::load(path).unwrap();
        assert_eq!(config.server.port, 8400);
    }

    #[test]
    fn test_config_load_from_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.toml");
        std::fs::write(
            &path,
            r#"
[server]
host = "0.0.0.0"
port = 9000

[auth]
enabled = false

[logging]
level = "debug"
json = true
"#,
        )
        .unwrap();

        let config = Config::load(&path).unwrap();
        assert_eq!(config.server.host, "0.0.0.0");
        assert_eq!(config.server.port, 9000);
        assert!(!config.auth.enabled);
        assert_eq!(config.logging.level, "debug");
        assert!(config.logging.json);
    }

    #[test]
    fn test_config_save_and_reload() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("save_test.toml");

        let mut config = Config::default();
        config.server.host = "10.0.0.1".to_string();
        config.server.port = 7777;
        config.auth.enabled = false;
        config.save(&path).unwrap();

        let reloaded = Config::load(&path).unwrap();
        assert_eq!(reloaded.server.host, "10.0.0.1");
        assert_eq!(reloaded.server.port, 7777);
        assert!(!reloaded.auth.enabled);
    }
}
