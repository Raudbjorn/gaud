use priority_lfu::DeepSizeOf;
use std::ops::Deref;

use srrldb_types::write_sql;

use crate::fmt::Fmt;
use crate::sql::idiom::Idiom;

#[derive(Clone, Debug, Default, PartialEq, Eq, DeepSizeOf)]
pub struct Splits(pub Vec<Split>);

impl srrldb_types::ToSql for Splits {
    fn fmt_sql(&self, f: &mut String, fmt: srrldb_types::SqlFormat) {
        write_sql!(f, fmt, "SPLIT ON {}", Fmt::comma_separated(&self.0))
    }
}

impl From<Splits> for crate::expr::Splits {
    fn from(v: Splits) -> Self {
        Self(v.0.into_iter().map(Into::into).collect())
    }
}

impl From<crate::expr::Splits> for Splits {
    fn from(v: crate::expr::Splits) -> Self {
        Self(v.0.into_iter().map(Into::into).collect())
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[cfg_attr(feature = "arbitrary", derive(arbitrary::Arbitrary))]
#[derive(DeepSizeOf)]
pub(crate) struct Split(pub(crate) Idiom);

impl Deref for Split {
    type Target = Idiom;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl srrldb_types::ToSql for Split {
    fn fmt_sql(&self, f: &mut String, fmt: srrldb_types::SqlFormat) {
        self.0.fmt_sql(f, fmt);
    }
}

impl From<Split> for crate::expr::Split {
    fn from(v: Split) -> Self {
        Self(v.0.into())
    }
}

impl From<crate::expr::Split> for Split {
    fn from(v: crate::expr::Split) -> Self {
        Self(v.0.into())
    }
}
