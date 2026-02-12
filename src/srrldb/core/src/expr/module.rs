use anyhow::{Result, bail};
use reblessive::tree::Stk;
use srrldb_types::{SqlFormat, ToSql};

use crate::catalog;
use crate::catalog::{DatabaseId, NamespaceId};
use crate::ctx::FrozenContext;
use crate::dbs::Options;
use crate::doc::CursorDoc;
use crate::expr::{Kind, Value};
use crate::val::File;

#[derive(Clone, Debug, Eq, PartialEq, Hash, priority_lfu::DeepSizeOf)]
pub(crate) enum ModuleExecutable {
    Surrealism(SurrealismExecutable),
    Silo(SiloExecutable),
}

impl From<catalog::ModuleExecutable> for ModuleExecutable {
    fn from(executable: catalog::ModuleExecutable) -> Self {
        match executable {
            catalog::ModuleExecutable::Surrealism(surrealism) => {
                ModuleExecutable::Surrealism(surrealism.into())
            }
            catalog::ModuleExecutable::Silo(silo) => ModuleExecutable::Silo(silo.into()),
        }
    }
}

impl From<ModuleExecutable> for catalog::ModuleExecutable {
    fn from(executable: ModuleExecutable) -> Self {
        match executable {
            ModuleExecutable::Surrealism(surrealism) => {
                catalog::ModuleExecutable::Surrealism(surrealism.into())
            }
            ModuleExecutable::Silo(silo) => catalog::ModuleExecutable::Silo(silo.into()),
        }
    }
}

impl ModuleExecutable {
    pub(crate) async fn signature(
        &self,
        ctx: &FrozenContext,
        ns: &NamespaceId,
        db: &DatabaseId,
        sub: Option<&str>,
    ) -> Result<Signature> {
        match self {
            ModuleExecutable::Surrealism(surrealism) => {
                surrealism.signature(ctx, ns, db, sub).await
            }
            ModuleExecutable::Silo(silo) => silo.signature(ctx, sub).await,
        }
    }

    pub(crate) async fn run(
        &self,
        stk: &mut Stk,
        ctx: &FrozenContext,
        opt: &Options,
        doc: Option<&CursorDoc>,
        args: Vec<Value>,
        sub: Option<&str>,
    ) -> Result<Value> {
        match self {
            ModuleExecutable::Surrealism(surrealism) => {
                surrealism.run(stk, ctx, opt, doc, args, sub).await
            }
            ModuleExecutable::Silo(silo) => silo.run(stk, ctx, opt, doc, args, sub).await,
        }
    }
}

impl ToSql for ModuleExecutable {
    fn fmt_sql(&self, f: &mut String, sql_fmt: SqlFormat) {
        let module_executable: crate::sql::ModuleExecutable = self.clone().into();
        module_executable.fmt_sql(f, sql_fmt);
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Hash, priority_lfu::DeepSizeOf)]
pub(crate) struct Signature {
    pub(crate) args: Vec<Kind>,
    pub(crate) returns: Option<Kind>,
}

#[derive(Clone, Debug, Eq, PartialEq, Hash, priority_lfu::DeepSizeOf)]
pub(crate) struct SurrealismExecutable(pub File);

impl From<catalog::SurrealismExecutable> for SurrealismExecutable {
    fn from(executable: catalog::SurrealismExecutable) -> Self {
        Self(File::new(executable.bucket, executable.key))
    }
}

impl From<SurrealismExecutable> for catalog::SurrealismExecutable {
    fn from(executable: SurrealismExecutable) -> Self {
        Self {
            bucket: executable.0.bucket,
            key: executable.0.key,
        }
    }
}

impl ToSql for SurrealismExecutable {
    fn fmt_sql(&self, f: &mut String, sql_fmt: SqlFormat) {
        let surrealism_executable: crate::sql::SurrealismExecutable = self.clone().into();
        surrealism_executable.fmt_sql(f, sql_fmt);
    }
}

impl SurrealismExecutable {
    pub(crate) async fn signature(
        &self,
        _ctx: &FrozenContext,
        _ns: &NamespaceId,
        _db: &DatabaseId,
        _sub: Option<&str>,
    ) -> Result<Signature> {
        bail!("Surrealism module signatures are not enabled in this build")
    }

    pub(crate) async fn run(
        &self,
        _stk: &mut Stk,
        _ctx: &FrozenContext,
        _opt: &Options,
        _doc: Option<&CursorDoc>,
        _args: Vec<Value>,
        _sub: Option<&str>,
    ) -> Result<Value> {
        bail!("Surrealism module execution is not enabled in this build")
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Hash, priority_lfu::DeepSizeOf)]
pub(crate) struct SiloExecutable {
    pub organisation: String,
    pub package: String,
    pub major: u32,
    pub minor: u32,
    pub patch: u32,
}

impl From<catalog::SiloExecutable> for SiloExecutable {
    fn from(executable: catalog::SiloExecutable) -> Self {
        Self {
            organisation: executable.organisation,
            package: executable.package,
            major: executable.major,
            minor: executable.minor,
            patch: executable.patch,
        }
    }
}

impl From<SiloExecutable> for catalog::SiloExecutable {
    fn from(executable: SiloExecutable) -> Self {
        Self {
            organisation: executable.organisation,
            package: executable.package,
            major: executable.major,
            minor: executable.minor,
            patch: executable.patch,
        }
    }
}

impl ToSql for SiloExecutable {
    fn fmt_sql(&self, f: &mut String, sql_fmt: SqlFormat) {
        let silo_executable: crate::sql::SiloExecutable = self.clone().into();
        silo_executable.fmt_sql(f, sql_fmt);
    }
}

impl SiloExecutable {
    pub(crate) async fn signature(
        &self,
        _ctx: &FrozenContext,
        _sub: Option<&str>,
    ) -> Result<Signature> {
        bail!("Silo module signatures are not enabled in this build")
    }

    pub(crate) async fn run(
        &self,
        _stk: &mut Stk,
        _ctx: &FrozenContext,
        _opt: &Options,
        _doc: Option<&CursorDoc>,
        _args: Vec<Value>,
        _sub: Option<&str>,
    ) -> Result<Value> {
        bail!("Silo module execution is not enabled in this build")
    }
}
