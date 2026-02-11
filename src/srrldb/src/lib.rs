//! `srrldb` — thin wrapper around SurrealDB for the gaud project.
//!
//! Re-exports [`srrldb_types`] as [`types`] and provides a simplified
//! [`Database`] handle that hides the generic engine parameter.

#[doc(inline)]
pub use srrldb_types as types;

mod database;
pub use database::Database;
