use rusqlite::Connection;
use std::path::Path;
use std::sync::{Arc, Mutex};

/// Thread-safe database handle wrapping a SQLite connection.
#[derive(Clone)]
pub struct Database {
    conn: Arc<Mutex<Connection>>,
}

impl Database {
    /// Open or create the database at the given path with WAL mode.
    pub fn open(path: &Path) -> anyhow::Result<Self> {
        let conn = Connection::open(path)?;

        // Enable WAL mode for concurrent reads
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        conn.pragma_update(None, "busy_timeout", 5000)?;

        let db = Self {
            conn: Arc::new(Mutex::new(conn)),
        };
        db.run_migrations()?;
        Ok(db)
    }

    /// Open an in-memory database (for testing).
    pub fn open_in_memory() -> anyhow::Result<Self> {
        let conn = Connection::open_in_memory()?;
        conn.pragma_update(None, "foreign_keys", "ON")?;

        let db = Self {
            conn: Arc::new(Mutex::new(conn)),
        };
        db.run_migrations()?;
        Ok(db)
    }

    /// Execute a closure with access to the database connection.
    pub fn with_conn<F, T>(&self, f: F) -> Result<T, rusqlite::Error>
    where
        F: FnOnce(&Connection) -> Result<T, rusqlite::Error>,
    {
        let conn = self.conn.lock().expect("database mutex poisoned");
        f(&conn)
    }

    fn run_migrations(&self) -> anyhow::Result<()> {
        self.with_conn(|conn| {
            conn.execute_batch(SCHEMA)?;
            Ok(())
        })?;
        Ok(())
    }
}

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS users (
    id          TEXT PRIMARY KEY,
    name        TEXT NOT NULL UNIQUE,
    role        TEXT NOT NULL DEFAULT 'member' CHECK (role IN ('admin', 'member')),
    created_at  TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE IF NOT EXISTS api_keys (
    id          TEXT PRIMARY KEY,
    user_id     TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    key_hash    TEXT NOT NULL UNIQUE,
    key_prefix  TEXT NOT NULL,
    label       TEXT NOT NULL DEFAULT '',
    created_at  TEXT NOT NULL DEFAULT (datetime('now')),
    last_used   TEXT
);
CREATE INDEX IF NOT EXISTS idx_api_keys_hash ON api_keys(key_hash);
CREATE INDEX IF NOT EXISTS idx_api_keys_user ON api_keys(user_id);

CREATE TABLE IF NOT EXISTS budgets (
    user_id         TEXT PRIMARY KEY REFERENCES users(id) ON DELETE CASCADE,
    monthly_limit   REAL,
    daily_limit     REAL,
    monthly_used    REAL NOT NULL DEFAULT 0.0,
    daily_used      REAL NOT NULL DEFAULT 0.0,
    period_start    TEXT NOT NULL DEFAULT (datetime('now', 'start of month')),
    day_start       TEXT NOT NULL DEFAULT (date('now'))
);

CREATE TABLE IF NOT EXISTS usage_log (
    id              TEXT PRIMARY KEY,
    user_id         TEXT NOT NULL REFERENCES users(id),
    request_id      TEXT NOT NULL,
    provider        TEXT NOT NULL,
    model           TEXT NOT NULL,
    input_tokens    INTEGER NOT NULL DEFAULT 0,
    output_tokens   INTEGER NOT NULL DEFAULT 0,
    cost            REAL NOT NULL DEFAULT 0.0,
    latency_ms      INTEGER NOT NULL DEFAULT 0,
    status          TEXT NOT NULL DEFAULT 'success',
    created_at      TEXT NOT NULL DEFAULT (datetime('now'))
);
CREATE INDEX IF NOT EXISTS idx_usage_log_user ON usage_log(user_id);
CREATE INDEX IF NOT EXISTS idx_usage_log_provider ON usage_log(provider);
CREATE INDEX IF NOT EXISTS idx_usage_log_created ON usage_log(created_at);

CREATE TABLE IF NOT EXISTS oauth_state (
    state_token     TEXT PRIMARY KEY,
    provider        TEXT NOT NULL,
    code_verifier   TEXT NOT NULL,
    created_at      TEXT NOT NULL DEFAULT (datetime('now')),
    expires_at      TEXT NOT NULL
);
"#;
