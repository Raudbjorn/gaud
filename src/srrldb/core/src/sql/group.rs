use priority_lfu::DeepSizeOf;
use crate::sql::idiom::Idiom;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[derive(DeepSizeOf)]
pub struct Groups(pub Vec<Group>);

impl srrldb_types::ToSql for Groups {
	fn fmt_sql(&self, f: &mut String, fmt: srrldb_types::SqlFormat) {
		if self.0.is_empty() {
			f.push_str("GROUP ALL");
		} else {
			f.push_str("GROUP BY ");
			for (i, item) in self.0.iter().enumerate() {
				if i > 0 {
					fmt.write_separator(f);
				}
				item.fmt_sql(f, fmt);
			}
		}
	}
}

impl From<Groups> for crate::expr::Groups {
	fn from(v: Groups) -> Self {
		Self(v.0.into_iter().map(Into::into).collect())
	}
}

impl From<crate::expr::Groups> for Groups {
	fn from(v: crate::expr::Groups) -> Self {
		Self(v.0.into_iter().map(Into::into).collect())
	}
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[cfg_attr(feature = "arbitrary", derive(arbitrary::Arbitrary))]
#[derive(DeepSizeOf)]
pub(crate) struct Group(
	#[cfg_attr(feature = "arbitrary", arbitrary(with = crate::sql::arbitrary::basic_idiom))]
	pub(crate) Idiom,
);

impl srrldb_types::ToSql for Group {
	fn fmt_sql(&self, f: &mut String, fmt: srrldb_types::SqlFormat) {
		self.0.fmt_sql(f, fmt);
	}
}

impl From<Group> for crate::expr::Group {
	fn from(v: Group) -> Self {
		Self(v.0.into())
	}
}

impl From<crate::expr::Group> for Group {
	fn from(v: crate::expr::Group) -> Self {
		Self(v.0.into())
	}
}
