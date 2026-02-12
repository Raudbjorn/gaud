use rusqlite::params;
use serde::Serialize;
use uuid::Uuid;

use crate::auth::keys::{self, GeneratedKey};
use crate::db::Database;
use crate::error::AppError;

/// Stored user record.
#[derive(Debug, Clone, Serialize)]
pub struct User {
    pub id: String,
    pub name: String,
    pub role: String,
    pub created_at: String,
}

/// Stored API key metadata (never includes the hash).
#[derive(Debug, Clone, Serialize)]
pub struct ApiKeyInfo {
    pub id: String,
    pub user_id: String,
    pub key_prefix: String,
    pub label: String,
    pub created_at: String,
    pub last_used: Option<String>,
}

/// Result of creating a new API key: metadata plus the one-time plaintext.
#[derive(Debug)]
pub struct CreatedApiKey {
    pub info: ApiKeyInfo,
    pub plaintext: String,
}

/// Result of bootstrapping the first admin user.
#[derive(Debug)]
pub struct BootstrapResult {
    pub user: User,
    pub api_key_plaintext: String,
}

// ---------------------------------------------------------------------------
// User CRUD
// ---------------------------------------------------------------------------

/// Create a new user with the given name and role.
pub fn create_user(db: &Database, name: &str, role: &str) -> Result<User, AppError> {
    if role != "admin" && role != "member" {
        return Err(AppError::BadRequest(format!(
            "Invalid role '{role}': must be 'admin' or 'member'"
        )));
    }

    let id = Uuid::new_v4().to_string();
    let user = db.with_conn(|conn| {
        conn.execute(
            "INSERT INTO users (id, name, role) VALUES (?1, ?2, ?3)",
            params![id, name, role],
        )?;

        let mut stmt =
            conn.prepare("SELECT id, name, role, created_at FROM users WHERE id = ?1")?;
        stmt.query_row(params![id], |row| {
            Ok(User {
                id: row.get(0)?,
                name: row.get(1)?,
                role: row.get(2)?,
                created_at: row.get(3)?,
            })
        })
    })?;

    tracing::info!(user_id = %user.id, name = %user.name, role = %user.role, "User created");
    Ok(user)
}

/// List all users.
pub fn list_users(db: &Database) -> Result<Vec<User>, AppError> {
    let users = db.with_conn(|conn| {
        let mut stmt =
            conn.prepare("SELECT id, name, role, created_at FROM users ORDER BY created_at")?;
        let rows = stmt.query_map([], |row| {
            Ok(User {
                id: row.get(0)?,
                name: row.get(1)?,
                role: row.get(2)?,
                created_at: row.get(3)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>()
    })?;
    Ok(users)
}

/// Get a single user by name.
pub fn get_user_by_name(db: &Database, name: &str) -> Result<User, AppError> {
    db.with_conn(|conn| {
        conn.query_row(
            "SELECT id, name, role, created_at FROM users WHERE name = ?1",
            [name],
            |row| {
                Ok(User {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    role: row.get(2)?,
                    created_at: row.get(3)?,
                })
            },
        )
    })
    .map_err(|_| AppError::NotFound(format!("User not found: {name}")))
}

/// Get a single user by ID.
pub fn get_user(db: &Database, user_id: &str) -> Result<User, AppError> {
    let user = db
        .with_conn(|conn| {
            let mut stmt =
                conn.prepare("SELECT id, name, role, created_at FROM users WHERE id = ?1")?;
            stmt.query_row(params![user_id], |row| {
                Ok(User {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    role: row.get(2)?,
                    created_at: row.get(3)?,
                })
            })
        })
        .map_err(|e| match e {
            rusqlite::Error::QueryReturnedNoRows => {
                AppError::NotFound(format!("User '{user_id}' not found"))
            }
            other => AppError::Database(other.to_string()),
        })?;
    Ok(user)
}

/// Delete a user and all associated API keys (cascade).
pub fn delete_user(db: &Database, user_id: &str) -> Result<(), AppError> {
    let deleted =
        db.with_conn(|conn| conn.execute("DELETE FROM users WHERE id = ?1", params![user_id]))?;

    if deleted == 0 {
        return Err(AppError::NotFound(format!("User '{user_id}' not found")));
    }

    tracing::info!(user_id = %user_id, "User deleted");
    Ok(())
}

// ---------------------------------------------------------------------------
// API key CRUD
// ---------------------------------------------------------------------------

/// Create a new API key for a user.
pub fn create_api_key(
    db: &Database,
    user_id: &str,
    label: &str,
) -> Result<CreatedApiKey, AppError> {
    // Verify the user exists first.
    get_user(db, user_id)?;

    let generated = keys::generate_api_key()
        .map_err(|e| AppError::Internal(format!("Failed to generate API key: {e}")))?;

    let GeneratedKey {
        plaintext,
        hash,
        prefix,
    } = generated;

    let key_id = Uuid::new_v4().to_string();

    let info = db.with_conn(|conn| {
        conn.execute(
            "INSERT INTO api_keys (id, user_id, key_hash, key_prefix, label) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![key_id, user_id, hash, prefix, label],
        )?;

        let mut stmt = conn.prepare(
            "SELECT id, user_id, key_prefix, label, created_at, last_used FROM api_keys WHERE id = ?1",
        )?;
        stmt.query_row(params![key_id], |row| {
            Ok(ApiKeyInfo {
                id: row.get(0)?,
                user_id: row.get(1)?,
                key_prefix: row.get(2)?,
                label: row.get(3)?,
                created_at: row.get(4)?,
                last_used: row.get(5)?,
            })
        })
    })?;

    tracing::info!(
        key_id = %info.id,
        user_id = %user_id,
        prefix = %info.key_prefix,
        "API key created"
    );

    Ok(CreatedApiKey { info, plaintext })
}

/// List all API keys for a user (metadata only, no hashes).
pub fn list_api_keys(db: &Database, user_id: &str) -> Result<Vec<ApiKeyInfo>, AppError> {
    let keys = db.with_conn(|conn| {
        let mut stmt = conn.prepare(
            "SELECT id, user_id, key_prefix, label, created_at, last_used \
             FROM api_keys WHERE user_id = ?1 ORDER BY created_at",
        )?;
        let rows = stmt.query_map(params![user_id], |row| {
            Ok(ApiKeyInfo {
                id: row.get(0)?,
                user_id: row.get(1)?,
                key_prefix: row.get(2)?,
                label: row.get(3)?,
                created_at: row.get(4)?,
                last_used: row.get(5)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>()
    })?;
    Ok(keys)
}

/// Revoke (delete) an API key by its ID.
pub fn revoke_api_key(db: &Database, key_id: &str) -> Result<(), AppError> {
    let deleted =
        db.with_conn(|conn| conn.execute("DELETE FROM api_keys WHERE id = ?1", params![key_id]))?;

    if deleted == 0 {
        return Err(AppError::NotFound(format!("API key '{key_id}' not found")));
    }

    tracing::info!(key_id = %key_id, "API key revoked");
    Ok(())
}

// ---------------------------------------------------------------------------
// Auth validation (used by middleware)
// ---------------------------------------------------------------------------

/// Validate a plaintext API key against the database.
///
/// Iterates all stored key hashes and verifies with argon2. On success,
/// updates `last_used` and returns the associated `AuthUser`.
pub fn validate_api_key(db: &Database, plaintext: &str) -> Result<crate::auth::AuthUser, AppError> {
    let rows = db.with_conn(|conn| {
        let mut stmt = conn.prepare(
            "SELECT ak.id, ak.key_hash, u.id, u.name, u.role \
             FROM api_keys ak \
             JOIN users u ON ak.user_id = u.id",
        )?;
        let mapped = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
            ))
        })?;
        mapped.collect::<Result<Vec<_>, _>>()
    })?;

    for (key_id, key_hash, user_id, name, role) in &rows {
        let ok = keys::verify_key(plaintext, key_hash)
            .map_err(|e| AppError::Internal(format!("Key verification error: {e}")))?;

        if ok {
            // Update last_used timestamp asynchronously (best-effort).
            let _ = db.with_conn(|conn| {
                conn.execute(
                    "UPDATE api_keys SET last_used = datetime('now') WHERE id = ?1",
                    params![key_id],
                )
            });

            return Ok(crate::auth::AuthUser {
                user_id: user_id.clone(),
                name: name.clone(),
                role: role.clone(),
            });
        }
    }

    Err(AppError::Unauthorized("Invalid API key".to_string()))
}

// ---------------------------------------------------------------------------
// Bootstrap
// ---------------------------------------------------------------------------

/// If no users exist, create a default admin user and generate an API key.
///
/// The plaintext key is printed to stdout so the operator can use it.
/// Returns `None` if users already exist.
pub fn bootstrap_admin(
    db: &Database,
    admin_name: &str,
) -> Result<Option<BootstrapResult>, AppError> {
    let user_count: i64 =
        db.with_conn(|conn| conn.query_row("SELECT COUNT(*) FROM users", [], |row| row.get(0)))?;

    if user_count > 0 {
        return Ok(None);
    }

    tracing::info!("No users found -- bootstrapping default admin");

    let user = create_user(db, admin_name, "admin")?;
    let key = create_api_key(db, &user.id, "bootstrap")?;

    println!();
    println!("=========================================================");
    println!("  GAUD first-run bootstrap");
    println!("---------------------------------------------------------");
    println!("  Admin user : {}", user.name);
    println!("  API key    : {}", key.plaintext);
    println!("---------------------------------------------------------");
    println!("  Save this key now -- it will not be shown again.");
    println!("=========================================================");
    println!();

    Ok(Some(BootstrapResult {
        user,
        api_key_plaintext: key.plaintext,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_db() -> Database {
        Database::open_in_memory().unwrap()
    }

    #[test]
    fn test_create_and_get_user() {
        let db = test_db();
        let user = create_user(&db, "alice", "member").unwrap();
        assert_eq!(user.name, "alice");
        assert_eq!(user.role, "member");

        let fetched = get_user(&db, &user.id).unwrap();
        assert_eq!(fetched.id, user.id);
    }

    #[test]
    fn test_get_user_by_name() {
        let db = test_db();
        let user = create_user(&db, "alice", "member").unwrap();
        let fetched = get_user_by_name(&db, "alice").unwrap();
        assert_eq!(fetched.id, user.id);
        assert_eq!(fetched.name, "alice");
        assert_eq!(fetched.role, "member");
    }

    #[test]
    fn test_get_user_by_name_not_found() {
        let db = test_db();
        let result = get_user_by_name(&db, "nonexistent");
        assert!(result.is_err());
    }

    #[test]
    fn test_create_user_invalid_role() {
        let db = test_db();
        let result = create_user(&db, "bob", "superuser");
        assert!(result.is_err());
    }

    #[test]
    fn test_create_user_duplicate_name() {
        let db = test_db();
        create_user(&db, "alice", "member").unwrap();
        let result = create_user(&db, "alice", "admin");
        assert!(result.is_err());
    }

    #[test]
    fn test_list_users() {
        let db = test_db();
        create_user(&db, "alice", "admin").unwrap();
        create_user(&db, "bob", "member").unwrap();

        let users = list_users(&db).unwrap();
        assert_eq!(users.len(), 2);
    }

    #[test]
    fn test_delete_user() {
        let db = test_db();
        let user = create_user(&db, "alice", "member").unwrap();
        delete_user(&db, &user.id).unwrap();

        let result = get_user(&db, &user.id);
        assert!(result.is_err());
    }

    #[test]
    fn test_delete_user_not_found() {
        let db = test_db();
        let result = delete_user(&db, "nonexistent");
        assert!(result.is_err());
    }

    #[test]
    fn test_create_and_list_api_keys() {
        let db = test_db();
        let user = create_user(&db, "alice", "admin").unwrap();
        let created = create_api_key(&db, &user.id, "test key").unwrap();

        assert!(created.plaintext.starts_with("sk-prx-"));
        assert_eq!(created.info.label, "test key");

        let keys = list_api_keys(&db, &user.id).unwrap();
        assert_eq!(keys.len(), 1);
        assert_eq!(keys[0].id, created.info.id);
    }

    #[test]
    fn test_create_api_key_user_not_found() {
        let db = test_db();
        let result = create_api_key(&db, "nonexistent", "test");
        assert!(result.is_err());
    }

    #[test]
    fn test_revoke_api_key() {
        let db = test_db();
        let user = create_user(&db, "alice", "admin").unwrap();
        let created = create_api_key(&db, &user.id, "revoke me").unwrap();
        revoke_api_key(&db, &created.info.id).unwrap();

        let keys = list_api_keys(&db, &user.id).unwrap();
        assert!(keys.is_empty());
    }

    #[test]
    fn test_revoke_api_key_not_found() {
        let db = test_db();
        let result = revoke_api_key(&db, "nonexistent");
        assert!(result.is_err());
    }

    #[test]
    fn test_validate_api_key_success() {
        let db = test_db();
        let user = create_user(&db, "alice", "admin").unwrap();
        let created = create_api_key(&db, &user.id, "auth test").unwrap();

        let auth_user = validate_api_key(&db, &created.plaintext).unwrap();
        assert_eq!(auth_user.user_id, user.id);
        assert_eq!(auth_user.name, "alice");
        assert_eq!(auth_user.role, "admin");
    }

    #[test]
    fn test_validate_api_key_invalid() {
        let db = test_db();
        create_user(&db, "alice", "admin").unwrap();

        let result = validate_api_key(&db, "sk-prx-invalid00000000000000000000");
        assert!(result.is_err());
    }

    #[test]
    fn test_validate_updates_last_used() {
        let db = test_db();
        let user = create_user(&db, "alice", "admin").unwrap();
        let created = create_api_key(&db, &user.id, "timestamp test").unwrap();

        // Before validation, last_used should be None.
        let keys = list_api_keys(&db, &user.id).unwrap();
        assert!(keys[0].last_used.is_none());

        // After validation, last_used should be set.
        validate_api_key(&db, &created.plaintext).unwrap();
        let keys = list_api_keys(&db, &user.id).unwrap();
        assert!(keys[0].last_used.is_some());
    }

    #[test]
    fn test_bootstrap_admin_creates_user_and_key() {
        let db = test_db();
        let result = bootstrap_admin(&db, "admin").unwrap();
        assert!(result.is_some());

        let boot = result.unwrap();
        assert_eq!(boot.user.name, "admin");
        assert_eq!(boot.user.role, "admin");
        assert!(boot.api_key_plaintext.starts_with("sk-prx-"));

        // Validate the generated key works.
        let auth_user = validate_api_key(&db, &boot.api_key_plaintext).unwrap();
        assert_eq!(auth_user.name, "admin");
    }

    #[test]
    fn test_bootstrap_admin_skips_if_users_exist() {
        let db = test_db();
        create_user(&db, "existing", "member").unwrap();

        let result = bootstrap_admin(&db, "admin").unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_delete_user_cascades_api_keys() {
        let db = test_db();
        let user = create_user(&db, "alice", "admin").unwrap();
        create_api_key(&db, &user.id, "key1").unwrap();
        create_api_key(&db, &user.id, "key2").unwrap();

        delete_user(&db, &user.id).unwrap();

        let keys = list_api_keys(&db, &user.id).unwrap();
        assert!(keys.is_empty());
    }
}
