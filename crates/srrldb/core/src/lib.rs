// Temporary allow deprecated until the 3.0
#![allow(deprecated)]
// This triggers because we have regex's in our Value type which have a unsafecell inside.
#![allow(clippy::mutable_key_type)]
// Increased to support #[instrument] on complex async functions. Those are compiled out in release
// builds.
#![recursion_limit = "256"]

//! # Surrealdb Core
//!
//! This crate is the internal core library of SurrealDB.
//! It contains most of the database functionality on top of which the surreal
//! binary is implemented.
//!
//! <section class="warning">
//! <h3>Unstable!</h3>
//! This crate is <b>SurrealDB internal API</b>. It does not adhere to semver
//! and it's API is free to change and break code even between patch versions.
//! If you are looking for a stable interface to the Surrealdb library please have a look at <a href="https://crates.io/crates/surrealdb">the rust SDK</a>
//! </section>

#![doc(html_favicon_url = "https://surrealdb.s3.amazonaws.com/favicon.png")]
#![doc(html_logo_url = "https://surrealdb.s3.amazonaws.com/icon.png")]
// TODO: Remove
// This is added to keep the move anyhow PR somewhat smaller. This should be removed in a follow-up
// PR.
#![allow(clippy::large_enum_variant)]

#[macro_use]
extern crate tracing;

pub mod types {
	pub use surrealdb_types::*;
	pub type PublicValue = surrealdb_types::Value;
	pub type PublicAction = surrealdb_types::Action;
	pub type PublicArray = surrealdb_types::Array;
	pub type PublicBytes = surrealdb_types::Bytes;
	pub type PublicDatetime = surrealdb_types::Datetime;
	pub type PublicDuration = surrealdb_types::Duration;
	pub type PublicFile = surrealdb_types::File;
	pub type PublicGeometry = surrealdb_types::Geometry;
	pub type PublicGeometryKind = surrealdb_types::GeometryKind;
	pub type PublicKind = surrealdb_types::Kind;
	pub type PublicKindLiteral = surrealdb_types::KindLiteral;
	pub type PublicNotification = surrealdb_types::Notification;
	pub type PublicNumber = surrealdb_types::Number;
	pub type PublicObject = surrealdb_types::Object;
	pub type PublicRecordId = surrealdb_types::RecordId;
	pub type PublicRecordIdKey = surrealdb_types::RecordIdKey;
	pub type PublicRecordIdKeyRange = surrealdb_types::RecordIdKeyRange;
	pub type PublicUuid = surrealdb_types::Uuid;
	pub type PublicVariables = surrealdb_types::Variables;
	pub type PublicRange = surrealdb_types::Range;
	pub type PublicRegex = surrealdb_types::Regex;
	pub type PublicSet = surrealdb_types::Set;
	pub type PublicTable = surrealdb_types::Table;
}

/// A unit struct used by the community edition of SurrealDB.
pub struct CommunityComposer();

#[macro_use]
mod mac;

#[doc(hidden)]
pub mod buc;
mod cf;
#[doc(hidden)]
pub mod doc;
mod exe;
mod fmt;
mod fnc;
mod key;
#[doc(hidden)]
pub mod str;
mod sys;

pub mod api;
pub mod catalog;
pub mod cnf;
pub mod ctx;
pub mod dbs;
pub mod env;
pub mod err;
pub mod expr;
pub mod iam;
pub mod idx;
pub mod kvs;
pub mod mem;
pub mod obs;
mod options;
pub mod rpc;
pub mod sql;
pub mod syn;
pub mod val;
