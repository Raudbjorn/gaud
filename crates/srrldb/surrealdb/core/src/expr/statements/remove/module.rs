use anyhow::Result;

use crate::catalog::providers::DatabaseProvider;
use crate::catalog::{ModuleExecutable, ModuleName};
use crate::ctx::FrozenContext;
use crate::dbs::Options;
use crate::err::Error;
use crate::expr::{Base, Value};
use crate::iam::{Action, ResourceKind};

#[derive(Clone, Debug, Eq, PartialEq, Hash, priority_lfu::DeepSizeOf)]
pub(crate) struct RemoveModuleStatement {
	pub name: ModuleName,
	pub if_exists: bool,
}

impl RemoveModuleStatement {
	/// Process this type returning a computed simple Value
	pub(crate) async fn compute(&self, ctx: &FrozenContext, opt: &Options) -> Result<Value> {
		// Allowed to run?
		opt.is_allowed(Action::Edit, ResourceKind::Module, &Base::Db)?;
		// Get the transaction
		let txn = ctx.tx();
		// Get the definition
		let (ns, db) = ctx.expect_ns_db_ids(opt).await?;
		let storage_name = self.name.get_storage_name();
		let md = match txn.get_db_module(ns, db, &storage_name).await {
			Ok(x) => x,
			Err(e) => {
				if self.if_exists && matches!(e.downcast_ref(), Some(Error::MdNotFound { .. })) {
					return Ok(Value::None);
				} else {
					return Err(e);
				}
			}
		};
		// Delete the definition
		let key = crate::key::database::md::new(ns, db, &storage_name);
		txn.del(&key).await?;
		// Clear the cache
		txn.clear_cache();

		// Ok all good
		Ok(Value::None)
	}
}
