use crate::types::PublicDuration;
use priority_lfu::DeepSizeOf;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, DeepSizeOf)]
pub struct ChangeFeed {
    pub expiry: PublicDuration,
    pub store_diff: bool,
}

impl srrldb_types::ToSql for ChangeFeed {
    fn fmt_sql(&self, f: &mut String, sql_fmt: srrldb_types::SqlFormat) {
        use srrldb_types::write_sql;
        write_sql!(f, sql_fmt, "CHANGEFEED {}", self.expiry);
        if self.store_diff {
            f.push_str(" INCLUDE ORIGINAL");
        }
    }
}

impl From<ChangeFeed> for crate::expr::ChangeFeed {
    fn from(v: ChangeFeed) -> Self {
        crate::expr::ChangeFeed {
            expiry: v.expiry.into(),
            store_diff: v.store_diff,
        }
    }
}

impl From<crate::expr::ChangeFeed> for ChangeFeed {
    fn from(v: crate::expr::ChangeFeed) -> Self {
        ChangeFeed {
            expiry: v.expiry.into(),
            store_diff: v.store_diff,
        }
    }
}
