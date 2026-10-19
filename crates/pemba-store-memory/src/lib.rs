//! In-memory relationship store for warden (`docs/05-storage.md` §3).
//!
//! Not persistent; per-process. Implements the write algorithm natively under one write
//! lock, so a batch is applied atomically with respect to every reader. Rows are never
//! updated in place except to set their delete revision, which is what makes reads at old
//! revisions see old state (MVCC-by-columns, ADR-005).

#![forbid(unsafe_code)]

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::{RwLock, RwLockReadGuard, RwLockWriteGuard};

use warden_core::Error;
use warden_core::model::{
    ObjectRef, Precondition, RelName, Relationship, RelationshipFilter, Revision, SubjectRef,
    TypeName, WriteOp, WriteRequest,
};
use warden_core::store::{
    DeleteResult, ReadStore, RelationCount, ReverseFilter, SchemaCheck, StoreError, StoredSchema,
    SubjectRelMatch, WritableStore, cmp_relationships, subject_columns,
};

/// A relationship store held in process memory. Cheap to create; ideal for tests.
#[derive(Debug, Default)]
pub struct MemoryStore {
    inner: RwLock<Inner>,
}

#[derive(Debug)]
struct Row {
    rel: Relationship,
    created: Revision,
    deleted: Option<Revision>,
}

impl Row {
    fn visible_at(&self, rev: Revision) -> bool {
        self.created <= rev && self.deleted.is_none_or(|d| d > rev)
    }
}

/// Group key of [`RelationCount`], in its field (and so `Ord`) order.
type CountKey = (TypeName, RelName, TypeName, Option<RelName>, bool);

#[derive(Debug, Default)]
struct Inner {
    rows: Vec<Row>,
    /// `(resource, relation)` → rows, for forward reads.
    fwd: HashMap<(ObjectRef, RelName), Vec<usize>>,
    /// `(subject type, subject id)` → rows, for reverse reads. Keyed without the subject
    /// relation (unlike `docs/05` §3) so `SubjectRelMatch::Any` is one lookup.
    rev: HashMap<(TypeName, String), Vec<usize>>,
    /// The live copy of each tuple; plays the role of SQLite's partial unique index.
    live: HashMap<Relationship, usize>,
    head: Revision,
    horizon: Revision,
    schema_versions: Vec<StoredSchema>,
}

fn subject_key(s: &SubjectRef) -> (TypeName, String) {
    let ty = match s {
        SubjectRef::Object(o) | SubjectRef::Set { object: o, .. } => &o.object_type,
        SubjectRef::Wildcard { object_type } => object_type,
    };
    (ty.clone(), subject_columns(s).1.to_owned())
}

fn rel_matches(m: &SubjectRelMatch, s: &SubjectRef) -> bool {
    match (m, s) {
        (SubjectRelMatch::Any, _) => true,
        (SubjectRelMatch::None, _) => !matches!(s, SubjectRef::Set { .. }),
        (SubjectRelMatch::Exact(want), SubjectRef::Set { relation, .. }) => want == relation,
        (SubjectRelMatch::Exact(_), _) => false,
    }
}

impl Inner {
    /// Visible tuples at `rev`, grouped as `RelationCount`.
    fn counts(&self, rev: Revision) -> Vec<RelationCount> {
        let mut groups: BTreeMap<CountKey, u64> = BTreeMap::new();
        for row in self.rows.iter().filter(|row| row.visible_at(rev)) {
            let r = &row.rel;
            let (subject_type, subject_relation, wildcard) = match &r.subject {
                SubjectRef::Object(o) => (&o.object_type, None, false),
                SubjectRef::Set { object, relation } => {
                    (&object.object_type, Some(relation.clone()), false)
                }
                SubjectRef::Wildcard { object_type } => (object_type, None, true),
            };
            let key = (
                r.resource.object_type.clone(),
                r.relation.clone(),
                subject_type.clone(),
                subject_relation,
                wildcard,
            );
            *groups.entry(key).or_default() += 1;
        }
        groups
            .into_iter()
            .map(
                |((resource_type, relation, subject_type, subject_relation, wildcard), count)| {
                    RelationCount {
                        resource_type,
                        relation,
                        subject_type,
                        subject_relation,
                        wildcard,
                        count,
                    }
                },
            )
            .collect()
    }

    fn next_revision(&self) -> Result<Revision, StoreError> {
        self.head
            .0
            .checked_add(1)
            .map(Revision)
            .ok_or(StoreError::RevisionsExhausted)
    }

    fn check_readable(&self, rev: Revision) -> Result<(), StoreError> {
        if rev < self.horizon || rev > self.head {
            return Err(StoreError::RevisionOutOfRange {
                requested: rev,
                horizon: self.horizon,
                head: self.head,
            });
        }
        Ok(())
    }

    fn index(&mut self, idx: usize) {
        let rel = &self.rows[idx].rel;
        self.fwd
            .entry((rel.resource.clone(), rel.relation.clone()))
            .or_default()
            .push(idx);
        self.rev
            .entry(subject_key(&rel.subject))
            .or_default()
            .push(idx);
        if self.rows[idx].deleted.is_none() {
            self.live.insert(rel.clone(), idx);
        }
    }

    fn insert(&mut self, rel: Relationship, created: Revision) {
        self.rows.push(Row {
            rel,
            created,
            deleted: None,
        });
        self.index(self.rows.len() - 1);
    }

    fn reindex(&mut self) {
        self.fwd.clear();
        self.rev.clear();
        self.live.clear();
        for idx in 0..self.rows.len() {
            self.index(idx);
        }
    }

    /// Rows on `resource#relation` visible at `rev`.
    fn forward(
        &self,
        rev: Revision,
        resource: &ObjectRef,
        relation: &RelName,
    ) -> impl Iterator<Item = &Relationship> {
        self.fwd
            .get(&(resource.clone(), relation.clone()))
            .into_iter()
            .flatten()
            .map(|&i| &self.rows[i])
            .filter(move |row| row.visible_at(rev))
            .map(|row| &row.rel)
    }
}

impl MemoryStore {
    /// An empty store at revision 0.
    pub fn new() -> Self {
        Self::default()
    }

    fn read_at(&self, rev: Revision) -> Result<RwLockReadGuard<'_, Inner>, StoreError> {
        let inner = self.read()?;
        inner.check_readable(rev)?;
        Ok(inner)
    }

    fn read(&self) -> Result<RwLockReadGuard<'_, Inner>, StoreError> {
        self.inner.read().map_err(|_| StoreError::Poisoned)
    }

    fn lock(&self) -> Result<RwLockWriteGuard<'_, Inner>, StoreError> {
        self.inner.write().map_err(|_| StoreError::Poisoned)
    }
}

impl ReadStore for MemoryStore {
    fn head_revision(&self) -> Result<Revision, StoreError> {
        Ok(self.read()?.head)
    }

    fn gc_horizon(&self) -> Result<Revision, StoreError> {
        Ok(self.read()?.horizon)
    }

    fn exists(&self, rev: Revision, rel: &Relationship) -> Result<bool, StoreError> {
        let inner = self.read_at(rev)?;
        Ok(inner
            .forward(rev, &rel.resource, &rel.relation)
            .any(|r| r.subject == rel.subject))
    }

    fn read_subject_sets(
        &self,
        rev: Revision,
        resource: &ObjectRef,
        relation: &RelName,
    ) -> Result<Vec<(ObjectRef, RelName)>, StoreError> {
        let inner = self.read_at(rev)?;
        let mut out: Vec<_> = inner
            .forward(rev, resource, relation)
            .filter_map(|r| match &r.subject {
                SubjectRef::Set { object, relation } => Some((object.clone(), relation.clone())),
                _ => None,
            })
            .collect();
        // The derived `Ord` on names is byte-wise `str` order, i.e. column order.
        out.sort();
        Ok(out)
    }

    fn read_related_objects(
        &self,
        rev: Revision,
        resource: &ObjectRef,
        relation: &RelName,
    ) -> Result<Vec<ObjectRef>, StoreError> {
        let inner = self.read_at(rev)?;
        let mut out: Vec<_> = inner
            .forward(rev, resource, relation)
            .filter_map(|r| match &r.subject {
                SubjectRef::Object(o) | SubjectRef::Set { object: o, .. } => Some(o.clone()),
                SubjectRef::Wildcard { .. } => None,
            })
            .collect();
        out.sort();
        out.dedup();
        Ok(out)
    }

    fn read_reverse(
        &self,
        rev: Revision,
        f: &ReverseFilter,
    ) -> Result<Vec<(ObjectRef, RelName)>, StoreError> {
        let inner = self.read_at(rev)?;
        let key = (f.subject_type.clone(), f.subject_id.clone());
        let mut out: Vec<_> = inner
            .rev
            .get(&key)
            .into_iter()
            .flatten()
            .map(|&i| &inner.rows[i])
            .filter(|row| row.visible_at(rev))
            .map(|row| &row.rel)
            .filter(|r| {
                rel_matches(&f.subject_relation, &r.subject)
                    && f.resource_type
                        .as_ref()
                        .is_none_or(|t| *t == r.resource.object_type)
                    && f.relation.as_ref().is_none_or(|n| *n == r.relation)
            })
            .map(|r| (r.resource.clone(), r.relation.clone()))
            .collect();
        out.sort();
        out.dedup();
        Ok(out)
    }

    fn read_relationships(
        &self,
        rev: Revision,
        f: &RelationshipFilter,
        limit: u32,
        after: Option<&Relationship>,
    ) -> Result<Vec<Relationship>, StoreError> {
        let inner = self.read_at(rev)?;
        // A full scan: this store is for tests and small embedded use, and tooling reads
        // are not on the evaluator's hot path.
        let mut out: Vec<Relationship> = inner
            .rows
            .iter()
            .filter(|row| row.visible_at(rev) && f.matches(&row.rel))
            .filter(|row| after.is_none_or(|a| cmp_relationships(&row.rel, a).is_gt()))
            .map(|row| row.rel.clone())
            .collect();
        out.sort_by(cmp_relationships);
        out.truncate(usize::try_from(limit).unwrap_or(usize::MAX));
        Ok(out)
    }

    fn read_tuples(
        &self,
        rev: Revision,
        resource: &ObjectRef,
        relation: &RelName,
    ) -> Result<Vec<SubjectRef>, StoreError> {
        let inner = self.read_at(rev)?;
        let mut out: Vec<SubjectRef> = inner
            .forward(rev, resource, relation)
            .map(|r| r.subject.clone())
            .collect();
        out.sort_by(|a, b| subject_columns(a).cmp(&subject_columns(b)));
        Ok(out)
    }

    fn count_by_relation(&self, rev: Revision) -> Result<Vec<RelationCount>, StoreError> {
        Ok(self.read_at(rev)?.counts(rev))
    }

    fn current_schema(&self) -> Result<Option<StoredSchema>, StoreError> {
        Ok(self.read()?.schema_versions.last().cloned())
    }
}

impl WritableStore for MemoryStore {
    fn write(&self, req: &WriteRequest, schema_version: Option<u64>) -> Result<Revision, Error> {
        let mut inner = self.lock()?;

        // Every check runs before the first mutation, so a failed batch leaves no trace
        // and consumes no revision.
        let mut seen = HashSet::with_capacity(req.ops.len());
        for op in &req.ops {
            if !seen.insert(op.relationship()) {
                return Err(Error::DuplicateInBatch(op.relationship().to_string()));
            }
        }
        if let Some(expected) = schema_version {
            let found = inner.schema_versions.len() as u64;
            if found != expected {
                return Err(Error::Store(StoreError::SchemaChanged { expected, found }));
            }
        }
        for (index, pre) in req.preconditions.iter().enumerate() {
            let (filter, must_exist) = match pre {
                Precondition::MustExist(f) => (f, true),
                Precondition::MustNotExist(f) => (f, false),
            };
            if inner.live.keys().any(|r| filter.matches(r)) != must_exist {
                return Err(Error::PreconditionFailed { index });
            }
        }
        for op in &req.ops {
            if let WriteOp::Create(r) = op
                && inner.live.contains_key(r)
            {
                return Err(Error::AlreadyExists(r.to_string()));
            }
        }
        let rev = inner.next_revision()?;

        for op in &req.ops {
            match op {
                WriteOp::Create(r) | WriteOp::Touch(r) => {
                    if !inner.live.contains_key(r) {
                        inner.insert(r.clone(), rev);
                    }
                }
                WriteOp::Delete(r) => {
                    if let Some(idx) = inner.live.remove(r) {
                        inner.rows[idx].deleted = Some(rev);
                    }
                }
            }
        }
        inner.head = rev;
        Ok(rev)
    }

    fn write_schema(
        &self,
        source: &str,
        hash: &str,
        check: SchemaCheck<'_>,
    ) -> Result<StoredSchema, Error> {
        let mut inner = self.lock()?;
        let head = inner.head;
        check(&inner.counts(head))?;
        let rev = inner.next_revision()?;
        let stored = StoredSchema {
            version: inner.schema_versions.len() as u64 + 1,
            source: source.to_owned(),
            hash: hash.to_owned(),
            created_rev: rev,
        };
        inner.schema_versions.push(stored.clone());
        inner.head = rev;
        Ok(stored)
    }

    fn delete_matching(
        &self,
        filter: &RelationshipFilter,
        limit: usize,
    ) -> Result<DeleteResult, Error> {
        let mut inner = self.lock()?;
        let rev = inner.next_revision()?;
        let mut matching: Vec<Relationship> = inner
            .live
            .keys()
            .filter(|r| filter.matches(r))
            .cloned()
            .collect();
        matching.sort_by(cmp_relationships);
        let more = matching.len() > limit;
        matching.truncate(limit);
        for r in &matching {
            if let Some(idx) = inner.live.remove(r) {
                inner.rows[idx].deleted = Some(rev);
            }
        }
        inner.head = rev;
        Ok(DeleteResult {
            deleted: matching.len() as u64,
            more,
            revision: rev,
        })
    }

    fn raise_gc_horizon(&self, keep_from: Revision) -> Result<Revision, Error> {
        let mut inner = self.lock()?;
        if keep_from > inner.head {
            return Err(Error::InvalidRequest {
                message: format!(
                    "gc keep_from {keep_from} is above the head revision {}",
                    inner.head
                ),
            });
        }
        inner.horizon = inner.horizon.max(keep_from);
        Ok(inner.horizon)
    }

    fn gc_batch(&self, max_rows: usize) -> Result<u64, Error> {
        let mut inner = self.lock()?;
        let horizon = inner.horizon;
        let mut removed = 0;
        inner.rows.retain(|row| {
            let collect = removed < max_rows && row.deleted.is_some_and(|d| d <= horizon);
            removed += usize::from(collect);
            !collect
        });
        if removed > 0 {
            // Row indices shifted; rebuilding is simpler than patching and GC is rare.
            inner.reindex();
        }
        Ok(removed as u64)
    }
}
