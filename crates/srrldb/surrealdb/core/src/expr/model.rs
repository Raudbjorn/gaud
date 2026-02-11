use priority_lfu::DeepSizeOf;
use reblessive::tree::Stk;
use surrealdb_types::{SqlFormat, ToSql};

use crate::ctx::FrozenContext;
use crate::dbs::Options;
use crate::doc::CursorDoc;
use crate::err::Error;
use crate::expr::{ControlFlow, FlowResult};
use crate::val::Value;

pub fn get_model_path(ns: &str, db: &str, name: &str, version: &str, hash: &str) -> String {
	format!("ml/{ns}/{db}/{name}-{version}-{hash}.surml")
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Hash, DeepSizeOf)]
pub(crate) struct Model {
	pub name: String,
	pub version: String,
}

impl ToSql for Model {
	fn fmt_sql(&self, f: &mut String, sql_fmt: SqlFormat) {
		let stmt: crate::sql::model::Model = self.clone().into();
		stmt.fmt_sql(f, sql_fmt);
	}
}

impl Model {
	pub(crate) async fn compute(
		&self,
		_stk: &mut Stk,
		_ctx: &FrozenContext,
		_opt: &Options,
		_doc: Option<&CursorDoc>,
		_args: Vec<Value>,
	) -> FlowResult<Value> {
		Err(ControlFlow::from(anyhow::Error::new(Error::InvalidModel {
			message: String::from("Machine learning computation is not enabled."),
		})))
	}
}
