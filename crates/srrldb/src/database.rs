//! Simplified embedded SurrealDB handle.
//!
//! Wraps `surrealdb_core`'s [`Datastore`] behind a single opaque [`Database`]
//! struct so callers don't need to carry generic engine parameters.

use surrealdb_core::dbs::Session;
use surrealdb_core::kvs::Datastore;
use crate::types::{SurrealValue, Value, Variables};
use std::collections::BTreeMap;
use std::sync::Arc;

/// Opaque handle to an embedded SurrealDB datastore.
pub struct Database {
    ds: Arc<Datastore>,
    session: Session,
}

/// Thin wrapper around SurrealDB query results so callers can chain
/// `.take()` / `.take_vec()` like the upstream client.
pub struct QueryResponse {
    results: Vec<surrealdb_core::dbs::QueryResult>,
}

impl Database {
    // -------------------------------------------------------------------
    // Constructors
    // -------------------------------------------------------------------

    /// Open a persistent RocksDB-backed datastore at `path`.
    #[cfg(feature = "kv-rocksdb")]
    pub async fn new_rocksdb(path: &str) -> Result<Self, Box<dyn std::error::Error>> {
        let ds = Datastore::new(&format!("rocksdb:{path}")).await?;
        Ok(Self {
            ds: Arc::new(ds),
            session: Session::owner(),
        })
    }

    /// Open an ephemeral in-memory datastore.
    #[cfg(feature = "kv-mem")]
    pub async fn new_mem() -> Result<Self, Box<dyn std::error::Error>> {
        let ds = Datastore::new("memory").await?;
        Ok(Self {
            ds: Arc::new(ds),
            session: Session::owner(),
        })
    }

    // -------------------------------------------------------------------
    // Namespace / database selection
    // -------------------------------------------------------------------

    /// Select both namespace and database in one call.
    pub async fn use_ns_db(
        &self,
        ns: &str,
        db: &str,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let sql = format!("USE NS `{ns}` DB `{db}`");
        let _ = self.ds.execute(&sql, &self.session, None).await?;
        Ok(())
    }

    // -------------------------------------------------------------------
    // Query builder
    // -------------------------------------------------------------------

    /// Start building a query. Returns a [`QueryBuilder`] that supports
    /// chained `.bind()` calls just like the upstream `Surreal` client.
    pub fn query<'a>(&'a self, sql: &'a str) -> QueryBuilder<'a> {
        QueryBuilder {
            db: self,
            sql,
            vars: BTreeMap::new(),
        }
    }
}

// -----------------------------------------------------------------------
// QueryBuilder (mini builder for parameter binding)
// -----------------------------------------------------------------------

/// Accumulates bound variables before executing a SurrealQL query.
pub struct QueryBuilder<'a> {
    db: &'a Database,
    sql: &'a str,
    vars: BTreeMap<String, Value>,
}

impl<'a> QueryBuilder<'a> {
    /// Bind a named parameter. The value must implement the `SurrealValue`
    /// trait (which all core types — String, u64, f32, Vec, Option, etc — do).
    pub fn bind(mut self, (key, val): (&str, impl SurrealValue)) -> Self {
        self.vars.insert(key.to_string(), val.into_value());
        self
    }

    /// Execute the query and return a [`QueryResponse`] wrapper.
    pub async fn execute(self) -> Result<QueryResponse, Box<dyn std::error::Error>> {
        let vars = if self.vars.is_empty() {
            None
        } else {
            Some(Variables::from(self.vars))
        };
        let results = self.db.ds.execute(self.sql, &self.db.session, vars).await?;
        Ok(QueryResponse { results })
    }
}

// Allow `builder.await` as a shorthand for `builder.execute().await`.
impl<'a> std::future::IntoFuture for QueryBuilder<'a> {
    type Output = Result<QueryResponse, Box<dyn std::error::Error>>;
    type IntoFuture = std::pin::Pin<
        Box<dyn std::future::Future<Output = Self::Output> + Send + 'a>,
    >;
    fn into_future(self) -> Self::IntoFuture {
        Box::pin(self.execute())
    }
}

// -----------------------------------------------------------------------
// QueryResponse
// -----------------------------------------------------------------------

impl QueryResponse {
    /// Take the result at position `idx`, deserialising into `T` via
    /// the `SurrealValue` trait (JSON round-trip).
    pub fn take<T: serde::de::DeserializeOwned>(
        &mut self,
        idx: usize,
    ) -> Result<T, Box<dyn std::error::Error>> {
        let qr = self
            .results
            .get(idx)
            .ok_or("response index out of bounds")?;
        let val = qr.result.as_ref().map_err(|e| e.to_string())?;
        let json = serde_json::to_string(val)?;
        let out: T = serde_json::from_str(&json)?;
        Ok(out)
    }

    /// Like [`take`](Self::take) but always returns a `Vec<T>`.
    pub fn take_vec<T: serde::de::DeserializeOwned>(
        &mut self,
        idx: usize,
    ) -> Result<Vec<T>, Box<dyn std::error::Error>> {
        self.take::<Vec<T>>(idx)
    }
}
