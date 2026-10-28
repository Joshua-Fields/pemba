//! [`ReadStore`] for [`PostgresStore`] and its [`Snapshot`]s (`docs/10-postgres.md`
//! §1.6.2): the statements of [`sql`], run on the store's pooled connections.
//!
//! Called on the store, each read is one autocommit statement on a pooled connection, so
//! it sees exactly the transactions committed before it started. Its range check uses
//! the head and horizon last observed, which may lag; it always uses the batch-map join,
//! which is right at any revision up to the real head. Called on a snapshot, every read
//! runs inside one `REPEATABLE READ READ ONLY` transaction whose first statement read head
//! and horizon, so all of them see one state; reads at the snapshot's own head use the
//! cheaper `deleted_batch IS NULL` form. The engine reads only through snapshots.

use std::sync::Mutex;

use pemba_core::model::{
    ObjectRef, RelName, Relationship, RelationshipFilter, Revision, SubjectRef,
};
use pemba_core::store::{
    ReadStore, RelationCount, ReverseFilter, StoreError, StoredSchema, relationship_columns,
};
use postgres::Row;
use postgres::types::ToSql;

use crate::dialect::PostgresDialect;
use crate::pool::Lease;
use crate::sql::{self, Form, Forward, P, Query};
use crate::statements::Conn;
use crate::{PostgresStore, store_err};

fn corrupt(message: String) -> StoreError {
    StoreError::Corrupt { message }
}

fn db(e: &postgres::Error) -> StoreError {
    store_err(e)
}

fn parse<T: std::str::FromStr>(s: &str, what: &str) -> Result<T, StoreError> {
    s.parse()
        .map_err(|_| corrupt(format!("stored {what} {s:?} is not valid")))
}

fn revision(n: i64) -> Result<Revision, StoreError> {
    u64::try_from(n)
        .map(Revision)
        .map_err(|_| corrupt(format!("negative revision {n}")))
}

fn text(row: &Row, i: usize) -> Result<&str, StoreError> {
    row.try_get(i).map_err(|e| db(&e))
}

fn object(row: &Row, i: usize, what: &str) -> Result<ObjectRef, StoreError> {
    Ok(ObjectRef {
        object_type: parse(text(row, i)?, &format!("{what} type"))?,
        object_id: parse(text(row, i + 1)?, &format!("{what} id"))?,
    })
}

/// A subject from its storage columns at `row[i..i + 3]` (`docs/05-storage.md` §4).
fn subject(row: &Row, i: usize) -> Result<SubjectRef, StoreError> {
    let (ty, id, rel) = (text(row, i)?, text(row, i + 1)?, text(row, i + 2)?);
    let object_type = parse(ty, "subject type")?;
    if id == "*" {
        if !rel.is_empty() {
            return Err(corrupt(format!(
                "wildcard subject {ty}:* has subject relation {rel:?}"
            )));
        }
        return Ok(SubjectRef::Wildcard { object_type });
    }
    let object = ObjectRef {
        object_type,
        object_id: parse(id, "subject id")?,
    };
    Ok(if rel.is_empty() {
        SubjectRef::Object(object)
    } else {
        SubjectRef::Set {
            object,
            relation: parse(rel, "subject relation")?,
        }
    })
}

fn run(client: &mut Conn, q: &Query<'_>) -> Result<Vec<Row>, StoreError> {
    let params: Vec<&(dyn ToSql + Sync)> = q
        .params
        .iter()
        .map(|p| match p {
            P::Text(s) => s as &(dyn ToSql + Sync),
            P::Int(n) => n as &(dyn ToSql + Sync),
        })
        .collect();
    client.query(q.sql.as_str(), &params).map_err(|e| db(&e))
}

/// Head and stored GC horizon from one statement.
fn head_and_horizon(
    client: &mut Conn,
    d: &PostgresDialect,
) -> Result<(Revision, Revision), StoreError> {
    let row = client
        .query_one(sql::head_and_horizon(d).as_str(), &[])
        .map_err(|e| db(&e))?;
    let head: Option<i64> = row.try_get(0).map_err(|e| db(&e))?;
    // The map always holds at least batch 0 → revision 0, and GC keeps its newest row.
    let head = revision(head.ok_or_else(|| corrupt("pemba_revisions is empty".into()))?)?;
    let horizon: Option<&str> = row.try_get(1).map_err(|e| db(&e))?;
    let horizon = horizon.ok_or_else(|| corrupt("pemba_meta has no gc_horizon".into()))?;
    Ok((head, Revision(parse(horizon, "gc_horizon")?)))
}

/// Where reads run, the readable range they are checked against, and the form they use.
trait Source {
    fn dialect(&self) -> &PostgresDialect;

    /// `(head, horizon)` for the range check. Never a query.
    fn bounds(&self) -> (Revision, Revision);

    /// The visibility form for a read at `rev` (already range-checked).
    fn form(&self, rev: Revision) -> Result<Form, StoreError>;

    fn with<T>(&self, f: impl FnOnce(&mut Conn) -> Result<T, StoreError>) -> Result<T, StoreError>;

    fn check_readable(&self, rev: Revision) -> Result<Form, StoreError> {
        let (head, horizon) = self.bounds();
        if rev < horizon || rev > head {
            return Err(StoreError::RevisionOutOfRange {
                requested: rev,
                horizon,
                head,
            });
        }
        self.form(rev)
    }
}

/// The join form, with the revision as a parameter.
fn at(rev: Revision) -> Result<Form, StoreError> {
    // Above i64::MAX no row can exist; the range check has already rejected it anyway.
    i64::try_from(rev.0)
        .map(Form::At)
        .map_err(|_| StoreError::RevisionOutOfRange {
            requested: rev,
            horizon: Revision(0),
            head: Revision(i64::MAX.unsigned_abs()),
        })
}

impl Source for PostgresStore {
    fn dialect(&self) -> &PostgresDialect {
        &self.dialect
    }

    fn bounds(&self) -> (Revision, Revision) {
        self.cache.get()
    }

    fn form(&self, rev: Revision) -> Result<Form, StoreError> {
        // The cached head may lag the real one, so `deleted_batch IS NULL` could show
        // later commits: always join.
        at(rev)
    }

    fn with<T>(&self, f: impl FnOnce(&mut Conn) -> Result<T, StoreError>) -> Result<T, StoreError> {
        let mut lease = self.pool.lease()?;
        f(lease.conn()?)
    }
}

/// One `REPEATABLE READ READ ONLY` transaction on one leased connection: everything read
/// through it, head and horizon included, comes from the same Postgres snapshot. Dropping
/// it rolls the transaction back and returns the connection, or closes the connection if
/// the rollback fails.
pub(crate) struct Snapshot<'a> {
    /// A `Mutex` only because [`ReadStore`] is `Sync`; one request uses a snapshot from
    /// one thread, so it is never contended. `None` only during `drop`.
    lease: Mutex<Option<Lease<'a>>>,
    dialect: &'a PostgresDialect,
    head: Revision,
    horizon: Revision,
}

impl Drop for Snapshot<'_> {
    fn drop(&mut self) {
        let lease = match self.lease.get_mut() {
            Ok(l) => l.take(),
            Err(poisoned) => poisoned.into_inner().take(),
        };
        if let Some(mut lease) = lease {
            let ended = lease
                .client()
                .is_ok_and(|c| c.batch_execute("ROLLBACK").is_ok());
            if !ended {
                lease.discard();
            }
        }
    }
}

impl Source for Snapshot<'_> {
    fn dialect(&self) -> &PostgresDialect {
        self.dialect
    }

    fn bounds(&self) -> (Revision, Revision) {
        (self.head, self.horizon)
    }

    fn form(&self, rev: Revision) -> Result<Form, StoreError> {
        if rev == self.head {
            Ok(Form::Head)
        } else {
            at(rev)
        }
    }

    fn with<T>(&self, f: impl FnOnce(&mut Conn) -> Result<T, StoreError>) -> Result<T, StoreError> {
        let mut guard = self.lease.lock().map_err(|_| StoreError::Poisoned)?;
        let lease = guard.as_mut().ok_or(StoreError::Poisoned)?;
        f(lease.conn()?)
    }
}

impl PostgresStore {
    /// Reads head and horizon on a pooled connection and refreshes the cache.
    fn observe(&self) -> Result<(Revision, Revision), StoreError> {
        let bounds = self.with(|c| head_and_horizon(c, &self.dialect))?;
        self.cache.observe(bounds.0, bounds.1);
        Ok(bounds)
    }

    /// Starts the snapshot's transaction and reads head and horizon as its first
    /// statement, which fixes the snapshot every later read sees.
    fn begin_snapshot(&self) -> Result<Snapshot<'_>, StoreError> {
        let mut lease = self.pool.lease()?;
        let client = lease.client()?;
        if let Err(e) = client.batch_execute("BEGIN ISOLATION LEVEL REPEATABLE READ READ ONLY") {
            let err = db(&e);
            lease.discard();
            return Err(err);
        }
        // From here on, dropping the snapshot ends the transaction on every path.
        let mut snapshot = Snapshot {
            lease: Mutex::new(Some(lease)),
            dialect: &self.dialect,
            head: Revision(0),
            horizon: Revision(0),
        };
        let (head, horizon) = snapshot.with(|c| head_and_horizon(c, &self.dialect))?;
        snapshot.head = head;
        snapshot.horizon = horizon;
        self.cache.observe(head, horizon);
        Ok(snapshot)
    }
}

fn exists(src: &impl Source, rev: Revision, rel: &Relationship) -> Result<bool, StoreError> {
    let form = src.check_readable(rev)?;
    let q = sql::exists(src.dialect(), form, relationship_columns(rel));
    Ok(!src.with(|c| run(c, &q))?.is_empty())
}

fn forward(
    src: &impl Source,
    kind: Forward,
    rev: Revision,
    resource: &ObjectRef,
    relation: &RelName,
) -> Result<Vec<Row>, StoreError> {
    let form = src.check_readable(rev)?;
    let q = sql::forward(
        src.dialect(),
        form,
        kind,
        [
            resource.object_type.as_str(),
            resource.object_id.as_str(),
            relation.as_str(),
        ],
    );
    src.with(|c| run(c, &q))
}

fn read_subject_sets(
    src: &impl Source,
    rev: Revision,
    resource: &ObjectRef,
    relation: &RelName,
) -> Result<Vec<(ObjectRef, RelName)>, StoreError> {
    forward(src, Forward::SubjectSets, rev, resource, relation)?
        .iter()
        .map(|row| {
            Ok((
                object(row, 0, "subject")?,
                parse(text(row, 2)?, "subject relation")?,
            ))
        })
        .collect()
}

fn read_related_objects(
    src: &impl Source,
    rev: Revision,
    resource: &ObjectRef,
    relation: &RelName,
) -> Result<Vec<ObjectRef>, StoreError> {
    forward(src, Forward::RelatedObjects, rev, resource, relation)?
        .iter()
        .map(|row| object(row, 0, "subject"))
        .collect()
}

fn read_tuples(
    src: &impl Source,
    rev: Revision,
    resource: &ObjectRef,
    relation: &RelName,
) -> Result<Vec<SubjectRef>, StoreError> {
    forward(src, Forward::Tuples, rev, resource, relation)?
        .iter()
        .map(|row| subject(row, 0))
        .collect()
}

fn read_reverse(
    src: &impl Source,
    rev: Revision,
    f: &ReverseFilter,
) -> Result<Vec<(ObjectRef, RelName)>, StoreError> {
    let form = src.check_readable(rev)?;
    let q = sql::read_reverse(src.dialect(), form, f);
    src.with(|c| run(c, &q))?
        .iter()
        .map(|row| {
            Ok((
                object(row, 0, "resource")?,
                parse(text(row, 2)?, "relation")?,
            ))
        })
        .collect()
}

fn read_relationships(
    src: &impl Source,
    rev: Revision,
    f: &RelationshipFilter,
    limit: u32,
    after: Option<&Relationship>,
) -> Result<Vec<Relationship>, StoreError> {
    let form = src.check_readable(rev)?;
    let q = sql::read_relationships(
        src.dialect(),
        form,
        f,
        limit,
        after.map(relationship_columns),
    );
    src.with(|c| run(c, &q))?
        .iter()
        .map(|row| {
            Ok(Relationship {
                resource: object(row, 0, "resource")?,
                relation: parse(text(row, 2)?, "relation")?,
                subject: subject(row, 3)?,
            })
        })
        .collect()
}

fn count_by_relation(src: &impl Source, rev: Revision) -> Result<Vec<RelationCount>, StoreError> {
    let form = src.check_readable(rev)?;
    let q = sql::count_by_relation(src.dialect(), form);
    src.with(|c| run(c, &q))?
        .iter()
        .map(|row| {
            let srel = text(row, 3)?;
            let count: i64 = row.try_get(5).map_err(|e| db(&e))?;
            Ok(RelationCount {
                resource_type: parse(text(row, 0)?, "resource type")?,
                relation: parse(text(row, 1)?, "relation")?,
                subject_type: parse(text(row, 2)?, "subject type")?,
                subject_relation: if srel.is_empty() {
                    None
                } else {
                    Some(parse(srel, "subject relation")?)
                },
                wildcard: row.try_get(4).map_err(|e| db(&e))?,
                count: u64::try_from(count)
                    .map_err(|_| corrupt(format!("negative count {count}")))?,
            })
        })
        .collect()
}

fn current_schema(src: &impl Source) -> Result<Option<StoredSchema>, StoreError> {
    let q = sql::current_schema(src.dialect());
    let rows = src.with(|c| c.query(q.as_str(), &[]).map_err(|e| db(&e)))?;
    rows.first()
        .map(|row| {
            let version: i64 = row.try_get(0).map_err(|e| db(&e))?;
            let rev: Option<i64> = row.try_get(3).map_err(|e| db(&e))?;
            Ok(StoredSchema {
                version: u64::try_from(version)
                    .map_err(|_| corrupt(format!("negative schema version {version}")))?,
                source: text(row, 1)?.to_owned(),
                hash: text(row, 2)?.to_owned(),
                created_rev: revision(rev.ok_or_else(|| {
                    corrupt(format!("schema version {version}'s batch has no revision"))
                })?)?,
            })
        })
        .transpose()
}

/// The latest schema version; through a snapshot, as of the snapshot.
fn schema_version(src: &impl Source) -> Result<u64, StoreError> {
    let q = sql::schema_version(src.dialect());
    let version: i64 = src.with(|c| {
        c.query_one(q.as_str(), &[])
            .and_then(|row| row.try_get(0))
            .map_err(|e| db(&e))
    })?;
    u64::try_from(version).map_err(|_| corrupt(format!("negative schema version {version}")))
}

/// The reads shared by the store and its snapshots; only head, horizon and `snapshot`
/// differ.
macro_rules! shared_reads {
    () => {
        fn exists(&self, rev: Revision, rel: &Relationship) -> Result<bool, StoreError> {
            exists(self, rev, rel)
        }

        fn read_subject_sets(
            &self,
            rev: Revision,
            resource: &ObjectRef,
            relation: &RelName,
        ) -> Result<Vec<(ObjectRef, RelName)>, StoreError> {
            read_subject_sets(self, rev, resource, relation)
        }

        fn read_related_objects(
            &self,
            rev: Revision,
            resource: &ObjectRef,
            relation: &RelName,
        ) -> Result<Vec<ObjectRef>, StoreError> {
            read_related_objects(self, rev, resource, relation)
        }

        fn read_reverse(
            &self,
            rev: Revision,
            f: &ReverseFilter,
        ) -> Result<Vec<(ObjectRef, RelName)>, StoreError> {
            read_reverse(self, rev, f)
        }

        fn read_relationships(
            &self,
            rev: Revision,
            f: &RelationshipFilter,
            limit: u32,
            after: Option<&Relationship>,
        ) -> Result<Vec<Relationship>, StoreError> {
            read_relationships(self, rev, f, limit, after)
        }

        fn read_tuples(
            &self,
            rev: Revision,
            resource: &ObjectRef,
            relation: &RelName,
        ) -> Result<Vec<SubjectRef>, StoreError> {
            read_tuples(self, rev, resource, relation)
        }

        fn count_by_relation(&self, rev: Revision) -> Result<Vec<RelationCount>, StoreError> {
            count_by_relation(self, rev)
        }

        fn current_schema(&self) -> Result<Option<StoredSchema>, StoreError> {
            current_schema(self)
        }

        fn schema_version(&self) -> Result<u64, StoreError> {
            schema_version(self)
        }
    };
}

impl ReadStore for PostgresStore {
    fn head_revision(&self) -> Result<Revision, StoreError> {
        Ok(self.observe()?.0)
    }

    fn gc_horizon(&self) -> Result<Revision, StoreError> {
        Ok(self.observe()?.1)
    }

    shared_reads!();

    fn snapshot(&self) -> Result<Box<dyn ReadStore + '_>, StoreError> {
        Ok(Box::new(self.begin_snapshot()?))
    }
}

/// Head and horizon are the ones the snapshot's first statement read: fixed for its
/// lifetime, so the engine's revision check, the choice of form and every read agree.
impl ReadStore for Snapshot<'_> {
    fn head_revision(&self) -> Result<Revision, StoreError> {
        Ok(self.head)
    }

    fn gc_horizon(&self) -> Result<Revision, StoreError> {
        Ok(self.horizon)
    }

    shared_reads!();

    /// Already a snapshot; a nested one sees the same state.
    fn snapshot(&self) -> Result<Box<dyn ReadStore + '_>, StoreError> {
        Ok(Box::new(self))
    }
}
