//! Postgres statement text (`docs/10-postgres.md` §3.3): `$n` placeholders, revisions
//! assigned at commit, and every table qualified with the store's quoted schema, so the
//! `search_path` of whatever connection runs the statement cannot redirect it.

use pemba_core::Error;
use pemba_core::store::{PlaceholderStyle, RevisionStrategy, SqlDialect};

/// Postgres's identifier length limit (`NAMEDATALEN - 1`), in bytes; longer names are
/// truncated by the server, which would silently name a different schema.
const MAX_IDENTIFIER_BYTES: usize = 63;

/// SQL text for one Postgres store: the store's schema, quoted, on every table name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PostgresDialect {
    schema: String,
    quoted: String,
}

impl PostgresDialect {
    /// The dialect for the store in `schema` (unquoted, as Postgres stores the name).
    ///
    /// # Errors
    ///
    /// [`Error::InvalidRequest`] if the name is not one this store accepts
    /// ([`check_schema_name`]).
    pub fn new(schema: &str) -> Result<Self, Error> {
        check_schema_name(schema)?;
        Ok(Self {
            schema: schema.to_owned(),
            quoted: quote_ident(schema),
        })
    }

    /// The schema name, unquoted.
    pub fn schema(&self) -> &str {
        &self.schema
    }

    /// The schema as a quoted identifier, e.g. `"app"`.
    pub fn quoted_schema(&self) -> &str {
        &self.quoted
    }
}

impl SqlDialect for PostgresDialect {
    fn placeholder_style(&self) -> PlaceholderStyle {
        PlaceholderStyle::Dollar
    }

    fn revision_strategy(&self) -> RevisionStrategy {
        // Concurrent writers commit in any order; only the commit trigger knows the order.
        RevisionStrategy::AssignAtCommit
    }

    fn table(&self, name: &str) -> String {
        format!("{}.{name}", self.quoted)
    }

    fn row_id(&self) -> &'static str {
        "ctid"
    }

    /// `docs/10-postgres.md` §1.6.1: an advisory lock keyed, like the revision lock, by
    /// [`PEMBA_LOCK_CLASS`](crate::PEMBA_LOCK_CLASS) and a table's oid, here
    /// `pemba_schema`'s, so it differs from the revision lock and between stores (OQ-049).
    fn schema_lock(&self, exclusive: bool) -> Option<String> {
        let function = if exclusive {
            "pg_advisory_xact_lock"
        } else {
            "pg_advisory_xact_lock_shared"
        };
        Some(format!(
            "SELECT pg_catalog.{function}({}, '{}'::pg_catalog.regclass::pg_catalog.oid::pg_catalog.int4)",
            crate::PEMBA_LOCK_CLASS,
            literal_body(&self.table("pemba_schema")),
        ))
    }
}

/// Refuses schema names the store cannot use safely.
///
/// Postgres accepts almost any name once quoted, but the migration bakes the name into a
/// PL/pgSQL body (`docs/10-postgres.md` §3.3): a `$` could end its dollar quote early and a
/// `\` would be read as an escape by a session with `standard_conforming_strings` off. So
/// these are refused along with names the server would truncate or cannot store.
///
/// # Errors
///
/// [`Error::InvalidRequest`] naming what is wrong.
pub fn check_schema_name(schema: &str) -> Result<(), Error> {
    let problem = if schema.is_empty() {
        Some("is empty")
    } else if schema.len() > MAX_IDENTIFIER_BYTES {
        Some("is longer than 63 bytes, which Postgres would truncate")
    } else if schema.contains('\0') {
        Some("contains NUL")
    } else if schema.contains('$') || schema.contains('\\') {
        Some("contains `$` or `\\`, which the store does not accept in schema names")
    } else {
        None
    };
    match problem {
        None => Ok(()),
        Some(p) => Err(Error::InvalidRequest {
            message: format!("schema name {schema:?} {p}"),
        }),
    }
}

/// `name` as a quoted SQL identifier.
pub(crate) fn quote_ident(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

/// `text` with `'` doubled, to sit between the quotes of a SQL string literal.
pub(crate) fn literal_body(text: &str) -> String {
    text.replace('\'', "''")
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn tables_are_qualified_with_the_quoted_schema() {
        let d = PostgresDialect::new("My \"odd\" schema").unwrap();
        assert_eq!(
            d.table("pemba_meta"),
            "\"My \"\"odd\"\" schema\".pemba_meta"
        );
        assert_eq!(d.placeholder(3), "$3");
        assert_eq!(d.revision_strategy(), RevisionStrategy::AssignAtCommit);
        assert_eq!(d.row_id(), "ctid");
        assert_eq!(
            d.schema_lock(false).unwrap(),
            "SELECT pg_catalog.pg_advisory_xact_lock_shared(1885695330, \
             '\"My \"\"odd\"\" schema\".pemba_schema'::pg_catalog.regclass::pg_catalog.oid::pg_catalog.int4)"
        );
        assert!(
            d.schema_lock(true)
                .unwrap()
                .starts_with("SELECT pg_catalog.pg_advisory_xact_lock(")
        );
    }

    #[test]
    fn unusable_schema_names_are_refused() {
        for bad in ["", "a$b", "a\\b", "a\0b", &"x".repeat(64)] {
            assert!(
                matches!(PostgresDialect::new(bad), Err(Error::InvalidRequest { .. })),
                "{bad:?}"
            );
        }
        assert!(PostgresDialect::new(&"x".repeat(63)).is_ok());
        assert!(PostgresDialect::new("O'Brien \"schema\"").is_ok());
    }

    #[test]
    fn literal_body_doubles_quotes() {
        assert_eq!(literal_body("\"O'B\".t"), "\"O''B\".t");
    }
}
