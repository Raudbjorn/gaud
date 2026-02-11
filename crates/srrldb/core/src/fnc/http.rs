use anyhow::Result;

use super::args::Optional;
use crate::ctx::FrozenContext;
use crate::err::Error;
use crate::val::Value;

pub async fn head(_: &FrozenContext, (_, _): (Value, Optional<Value>)) -> Result<Value> {
	anyhow::bail!(Error::HttpDisabled)
}

pub async fn get(_: &FrozenContext, (_, _): (Value, Optional<Value>)) -> Result<Value> {
	anyhow::bail!(Error::HttpDisabled)
}

pub async fn put(
	_: &FrozenContext,
	(_, _, _): (Value, Optional<Value>, Optional<Value>),
) -> Result<Value> {
	anyhow::bail!(Error::HttpDisabled)
}

pub async fn post(
	_: &FrozenContext,
	(_, _, _): (Value, Optional<Value>, Optional<Value>),
) -> Result<Value> {
	anyhow::bail!(Error::HttpDisabled)
}

pub async fn patch(
	_: &FrozenContext,
	(_, _, _): (Value, Optional<Value>, Optional<Value>),
) -> Result<Value> {
	anyhow::bail!(Error::HttpDisabled)
}

pub async fn delete(_: &FrozenContext, (_, _): (Value, Optional<Value>)) -> Result<Value> {
	anyhow::bail!(Error::HttpDisabled)
}
