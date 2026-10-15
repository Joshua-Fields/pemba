//! Validated name and id newtypes (`docs/02-data-model.md` §1).

use std::borrow::Borrow;
use std::fmt;
use std::str::FromStr;
use std::sync::Arc;

use warden_schema::ident::{IdentError, validate_identifier, validate_type_name};

use super::error::{Component, Fail, ParseError, ParseErrorReason};

const OBJECT_ID_MAX: usize = 1024;

macro_rules! name_type {
    ($(#[$meta:meta])* $name:ident, $check:path) => {
        $(#[$meta])*
        #[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $name(Arc<str>);

        impl $name {
            /// The validated string.
            pub fn as_str(&self) -> &str {
                &self.0
            }

            pub(crate) fn parse(s: &str) -> Result<Self, Fail> {
                $check(s)?;
                Ok(Self(Arc::from(s)))
            }
        }

        impl FromStr for $name {
            type Err = ParseError;

            fn from_str(s: &str) -> Result<Self, ParseError> {
                Self::parse(s).map_err(|f| f.into_error(s))
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }

        impl AsRef<str> for $name {
            fn as_ref(&self) -> &str {
                &self.0
            }
        }

        // Sound because `Arc<str>` hashes and compares exactly like `str`.
        impl Borrow<str> for $name {
            fn borrow(&self) -> &str {
                &self.0
            }
        }
    };
}

name_type!(
    /// An object type name such as `document` or `docs/document`: one or more
    /// `/`-separated identifier segments, at most 128 bytes in total.
    TypeName,
    check_type_name
);

name_type!(
    /// A relation or permission name: a single identifier (`docs/02-data-model.md` §1.1).
    RelName,
    check_rel_name
);

name_type!(
    /// An object id: 1–1024 characters from `[A-Za-z0-9_\-=+/|.]`, never `*`.
    /// Compared byte-wise, so case-sensitive.
    ObjectId,
    check_object_id
);

fn ident_fail(component: Component) -> impl FnOnce(IdentError) -> Fail {
    move |e| {
        Fail::new(
            e.offset,
            ParseErrorReason::Identifier {
                component,
                kind: e.kind,
            },
        )
    }
}

fn check_rel_name(s: &str) -> Result<(), Fail> {
    validate_identifier(s).map_err(ident_fail(Component::RelName))
}

fn check_type_name(s: &str) -> Result<(), Fail> {
    validate_type_name(s).map_err(ident_fail(Component::TypeName))
}

fn is_object_id_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '=' | '+' | '/' | '|' | '.')
}

fn check_object_id(s: &str) -> Result<(), Fail> {
    if s.is_empty() {
        return Err(Fail::new(0, ParseErrorReason::Empty(Component::ObjectId)));
    }
    if s == "*" {
        return Err(Fail::new(0, ParseErrorReason::Wildcard));
    }
    if let Some((i, found)) = s.char_indices().find(|&(_, c)| !is_object_id_char(c)) {
        return Err(Fail::new(
            i,
            ParseErrorReason::InvalidChar {
                component: Component::ObjectId,
                found,
            },
        ));
    }
    if s.len() > OBJECT_ID_MAX {
        return Err(Fail::new(
            OBJECT_ID_MAX,
            ParseErrorReason::TooLong {
                component: Component::ObjectId,
                max: OBJECT_ID_MAX,
            },
        ));
    }
    Ok(())
}
