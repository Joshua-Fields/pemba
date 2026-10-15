//! [`ReadStore`] for [`SqliteStore`] and its [`Snapshot`]s: the queries of `sql`, run on
//! read-only connections from the reader pool.
//!
//! Called on the store, each read runs as one autocommit statement on a pooled connection,
//! so under WAL it sees exactly the transactions committed before it started: never a
//! caller's open transaction, never part of a batch. Called on a snapshot, every read runs
//! inside the snapshot's one read transaction, whose first statement read head and GC
//! horizon, so WAL pins all of them to that state (`docs/05-storage.md` §2, "Revision
//! checks"). The engine reads only through snapshots.

use std::sync::Mutex;

use rusqlite::{Connection, OptionalExtension, Row, ToSql};
use warden_core::model::{
    ObjectId, ObjectRef, RelName, Relationship, RelationshipFilter, Revision, SubjectRef, TypeName,
};
use warden_core::store::{
    ReadStore, RelationCount, ReverseFilter, StoreError, StoredSchema, SubjectRelMatch,
    relationship_columns, subject_columns,
};

use crate::pool::Lease;
use crate::{SqliteStore, poisoned, sql, store_err};

fn corrupt(message: String) -> StoreError {
    StoreError::Corrupt { message }
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

fn rev_param(rev: Revision) -> Result<i64, StoreError> {
    // Above i64::MAX no row can exist; `check_readable` has already rejected it anyway.
    i64::try_from(rev.0).map_err(|_| StoreError::RevisionOutOfRange {
        requested: rev,
        horizon: Revision(0),
        head: Revision(i64::MAX as u64),
    })
}

/// A subject from its storage columns (`docs/05-storage.md` §4).
fn subject(ty: &str, id: &str, rel: &str) -> Result<SubjectRef, StoreError> {
    let object_type: TypeName = parse(ty, "subject type")?;
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

fn strings<const N: usize>(row: &Row<'_>) -> rusqlite::Result<[String; N]> {
    let mut out: [String; N] = std::array::from_fn(|_| String::new());
    for (i, slot) in out.iter_mut().enumerate() {
        *slot = row.get(i)?;
    }
    Ok(out)
}

/// Runs `sql` with `params` and collects each row's first `N` text columns.
fn text_rows<const N: usize>(
    conn: &Connection,
    sql: &str,
    params: &[&dyn ToSql],
) -> Result<Vec<[String; N]>, StoreError> {
    let mut stmt = conn.prepare_cached(sql).map_err(|e| store_err(&e))?;
    let rows = stmt
        .query_map(params, strings::<N>)
        .map_err(|e| store_err(&e))?;
    rows.collect::<Result<_, _>>().map_err(|e| store_err(&e))
}

/// Head and stored GC horizon in one statement on `conn`.
fn head_and_horizon(conn: &Connection) -> Result<(Revision, Revision), StoreError> {
    let (head, horizon): (i64, Option<String>) = conn
        .prepare_cached(sql::HEAD_AND_HORIZON)
        .and_then(|mut s| s.query_row([], |r| Ok((r.get(0)?, r.get(1)?))))
        .map_err(|e| store_err(&e))?;
    let head = revision(head)?;
    let horizon = horizon.ok_or_else(|| corrupt("warden_meta has no gc_horizon".into()))?;
    Ok((head, Revision(parse(&horizon, "gc_horizon")?)))
}

/// Where reads run, and the readable range they are checked against.
trait Source {
    /// `(head, horizon)` for the fail-closed backstop of `docs/05-storage.md` §2. Never a
    /// query.
    fn bounds(&self) -> (Revision, Revision);

    fn run<T>(&self, f: impl FnOnce(&Connection) -> Result<T, StoreError>)
    -> Result<T, StoreError>;

    fn check_readable(&self, rev: Revision) -> Result<(), StoreError> {
        let (head, horizon) = self.bounds();
        if rev < horizon || rev > head {
            return Err(StoreError::RevisionOutOfRange {
                requested: rev,
                horizon,
                head,
            });
        }
        Ok(())
    }
}

/// The store itself: a pooled connection per read, checked against the cached range,
/// which another process's GC does not move (hence [`Snapshot`] for requests).
impl Source for SqliteStore {
    fn bounds(&self) -> (Revision, Revision) {
        self.cache.get()
    }

    fn run<T>(
        &self,
        f: impl FnOnce(&Connection) -> Result<T, StoreError>,
    ) -> Result<T, StoreError> {
        self.readers.with(f)
    }
}

/// One read transaction on one leased reader connection: everything read through it,
/// head and horizon included, comes from the same WAL snapshot (`docs/05-storage.md` §2,
/// "Revision checks"). Dropping it rolls the transaction back and returns the connection
/// to the pool, on every path.
#[derive(Debug)]
pub(crate) struct Snapshot<'a> {
    /// A `Mutex` only because [`ReadStore`] is `Sync` and a connection is not; one request
    /// uses a snapshot from one thread, so it is never contended.
    lease: Mutex<Lease<'a>>,
    head: Revision,
    horizon: Revision,
}

impl Source for Snapshot<'_> {
    fn bounds(&self) -> (Revision, Revision) {
        (self.head, self.horizon)
    }

    fn run<T>(
        &self,
        f: impl FnOnce(&Connection) -> Result<T, StoreError>,
    ) -> Result<T, StoreError> {
        let lease = self.lease.lock().map_err(poisoned)?;
        f(lease.conn()?)
    }
}

impl SqliteStore {
    /// Reads head and horizon in one statement and refreshes both caches.
    fn observe(&self) -> Result<(Revision, Revision), StoreError> {
        let (head, horizon) = self.readers.with(head_and_horizon)?;
        self.cache.observe(head, horizon);
        Ok((head, horizon))
    }

    /// Starts a read transaction whose first statement reads head and horizon. In WAL
    /// mode a deferred transaction takes its snapshot at its first read, so that query
    /// and every later one see the same commits; a GC committed afterwards, by any
    /// process, stays invisible to it (SQLite does not checkpoint over a page an open
    /// reader still needs).
    fn begin_snapshot(&self) -> Result<Snapshot<'_>, StoreError> {
        // On any error below, dropping the lease rolls back and returns the connection.
        let lease = self.readers.lease()?;
        let conn = lease.conn()?;
        conn.execute_batch("BEGIN DEFERRED")
            .map_err(|e| store_err(&e))?;
        let (head, horizon) = head_and_horizon(conn)?;
        self.cache.observe(head, horizon);
        Ok(Snapshot {
            lease: Mutex::new(lease),
            head,
            horizon,
        })
    }
}

fn forward<const N: usize>(
    src: &impl Source,
    query: &str,
    rev: Revision,
    resource: &ObjectRef,
    relation: &RelName,
) -> Result<Vec<[String; N]>, StoreError> {
    src.check_readable(rev)?;
    let r = rev_param(rev)?;
    src.run(|conn| {
        text_rows::<N>(
            conn,
            query,
            &[
                &resource.object_type.as_str(),
                &resource.object_id.as_str(),
                &relation.as_str(),
                &r,
            ],
        )
    })
}

fn exists(src: &impl Source, rev: Revision, rel: &Relationship) -> Result<bool, StoreError> {
    src.check_readable(rev)?;
    let r = rev_param(rev)?;
    let [rt, rid, relation, st, sid, srel] = relationship_columns(rel);
    src.run(|conn| {
        conn.prepare_cached(sql::EXISTS)
            .and_then(|mut s| {
                s.query_row(
                    rusqlite::params![rt, rid, relation, st, sid, srel, r],
                    |_| Ok(()),
                )
                .optional()
            })
            .map(|found| found.is_some())
            .map_err(|e| store_err(&e))
    })
}

fn read_subject_sets(
    src: &impl Source,
    rev: Revision,
    resource: &ObjectRef,
    relation: &RelName,
) -> Result<Vec<(ObjectRef, RelName)>, StoreError> {
    forward::<3>(src, sql::READ_SUBJECT_SETS, rev, resource, relation)?
        .into_iter()
        .map(|[ty, id, rel]| {
            Ok((
                ObjectRef {
                    object_type: parse(&ty, "subject type")?,
                    object_id: parse(&id, "subject id")?,
                },
                parse(&rel, "subject relation")?,
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
    forward::<2>(src, sql::READ_RELATED_OBJECTS, rev, resource, relation)?
        .into_iter()
        .map(|[ty, id]| {
            Ok(ObjectRef {
                object_type: parse(&ty, "subject type")?,
                object_id: parse(&id, "subject id")?,
            })
        })
        .collect()
}

fn read_reverse(
    src: &impl Source,
    rev: Revision,
    f: &ReverseFilter,
) -> Result<Vec<(ObjectRef, RelName)>, StoreError> {
    src.check_readable(rev)?;
    let r = rev_param(rev)?;
    let query = sql::read_reverse(f);
    let subject_type = f.subject_type.as_str();
    let mut params: Vec<&dyn ToSql> = vec![&subject_type, &f.subject_id, &r];
    let exact;
    if let SubjectRelMatch::Exact(rel) = &f.subject_relation {
        exact = rel.as_str();
        params.push(&exact);
    }
    let resource_type = f.resource_type.as_ref().map(TypeName::as_str);
    if let Some(t) = &resource_type {
        params.push(t);
    }
    let relation = f.relation.as_ref().map(RelName::as_str);
    if let Some(n) = &relation {
        params.push(n);
    }
    src.run(|conn| text_rows::<3>(conn, &query, &params))?
        .into_iter()
        .map(|[ty, id, rel]| {
            Ok((
                ObjectRef {
                    object_type: parse(&ty, "resource type")?,
                    object_id: parse(&id, "resource id")?,
                },
                parse(&rel, "relation")?,
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
    src.check_readable(rev)?;
    let r = rev_param(rev)?;
    let limit = i64::from(limit);
    let query = sql::read_relationships(f, after.is_some());
    let optional = [
        f.resource_id.as_ref().map(ObjectId::as_str),
        f.relation.as_ref().map(RelName::as_str),
        f.subject_type.as_ref().map(TypeName::as_str),
        f.subject_id.as_deref(),
        f.subject_relation
            .as_ref()
            .map(|rel| rel.as_ref().map_or("", RelName::as_str)),
    ];
    let cursor = after.map(relationship_columns);
    let resource_type = f.resource_type.as_str();
    let mut params: Vec<&dyn ToSql> = vec![&resource_type, &r, &limit];
    params.extend(optional.iter().flatten().map(|v| v as &dyn ToSql));
    if let Some(c) = &cursor {
        params.extend(c.iter().map(|v| v as &dyn ToSql));
    }
    src.run(|conn| text_rows::<6>(conn, &query, &params))?
        .into_iter()
        .map(|[rt, rid, relation, st, sid, srel]| {
            Ok(Relationship {
                resource: ObjectRef {
                    object_type: parse(&rt, "resource type")?,
                    object_id: parse(&rid, "resource id")?,
                },
                relation: parse(&relation, "relation")?,
                subject: subject(&st, &sid, &srel)?,
            })
        })
        .collect()
}

fn read_tuples(
    src: &impl Source,
    rev: Revision,
    resource: &ObjectRef,
    relation: &RelName,
) -> Result<Vec<SubjectRef>, StoreError> {
    let out = forward::<3>(src, sql::READ_TUPLES, rev, resource, relation)?
        .into_iter()
        .map(|[ty, id, rel]| subject(&ty, &id, &rel))
        .collect::<Result<Vec<_>, _>>()?;
    debug_assert!(
        out.windows(2)
            .all(|w| subject_columns(&w[0]) <= subject_columns(&w[1]))
    );
    Ok(out)
}

fn count_by_relation(src: &impl Source, rev: Revision) -> Result<Vec<RelationCount>, StoreError> {
    src.check_readable(rev)?;
    let r = rev_param(rev)?;
    let rows: Vec<(String, String, String, String, bool, i64)> = src.run(|conn| {
        let mut stmt = conn
            .prepare_cached(sql::COUNT_BY_RELATION)
            .map_err(|e| store_err(&e))?;
        let rows = stmt
            .query_map([r], |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                ))
            })
            .map_err(|e| store_err(&e))?;
        rows.collect::<Result<_, _>>().map_err(|e| store_err(&e))
    })?;
    rows.into_iter()
        .map(|(rt, relation, st, srel, wildcard, count)| {
            Ok(RelationCount {
                resource_type: parse(&rt, "resource type")?,
                relation: parse(&relation, "relation")?,
                subject_type: parse(&st, "subject type")?,
                subject_relation: if srel.is_empty() {
                    None
                } else {
                    Some(parse(&srel, "subject relation")?)
                },
                wildcard,
                count: u64::try_from(count)
                    .map_err(|_| corrupt(format!("negative count {count}")))?,
            })
        })
        .collect()
}

fn current_schema(src: &impl Source) -> Result<Option<StoredSchema>, StoreError> {
    let row: Option<(i64, String, String, i64)> = src.run(|conn| {
        conn.prepare_cached(sql::CURRENT_SCHEMA)
            .and_then(|mut s| {
                s.query_row([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
                    .optional()
            })
            .map_err(|e| store_err(&e))
    })?;
    row.map(|(version, source, hash, created_rev)| {
        Ok(StoredSchema {
            version: u64::try_from(version)
                .map_err(|_| corrupt(format!("negative schema version {version}")))?,
            source,
            hash,
            created_rev: revision(created_rev)?,
        })
    })
    .transpose()
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
    };
}

impl ReadStore for SqliteStore {
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
/// lifetime, so the engine's revision check and every read agree.
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
