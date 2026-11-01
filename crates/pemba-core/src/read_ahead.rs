//! Fetching store reads ahead of the evaluator on a store where each read is a round trip
//! (`docs/T10.1-READ-AHEAD.md`, OQ-065).
//!
//! The evaluator stays depth-first, lazy and in charge of everything it decides. Hints queue
//! the reads it may make next, with no I/O. When it asks for a read nobody has answered, one
//! statement carries that read and the front of the queue, and the answers are kept for the
//! rest of the request. What is kept is tuples at one snapshot and revision, the same a
//! single read would return, never a node's value, so memo, cycle handling, limits and errors
//! work as if each read had been made alone. A wrong hint costs a statement or wasted rows,
//! and cannot change an answer.

use std::collections::{HashMap, HashSet, VecDeque};

use crate::model::{ObjectRef, RelName, Relationship, Revision};
use crate::store::{BatchAnswer, BatchRead, ReadStore, StoreError};

/// Children of one arrow or subject-set read whose reads are hinted at once.
pub(crate) const MAX_FRONTIER: usize = 256;
/// Candidates of a `lookup_resources` page hinted at once. At the 7 reads of a typical
/// permission's expression, these fit one statement at Postgres' batch limit.
pub(crate) const CANDIDATE_WINDOW: usize = 128;
/// Speculative reads in a request's first statement; each later one may carry twice as many,
/// up to the store's limit. A wide arrow decided by its first object stays cheap.
const FIRST_RAMP: usize = 32;
/// Each read asked for allows this many more speculative ones, so what speculation fetches
/// stays linear in the reads `max_check_reads` bounds.
const SPECULATION_PER_READ: u64 = 16;
/// Answers kept before they are dropped. Dropping costs only a read again.
const MAX_ANSWERS: usize = 16_384;

/// Answers fetched ahead for one evaluator. All of them come from its one snapshot and
/// revision.
#[derive(Debug)]
pub(crate) struct ReadAhead {
    /// Reads one statement may carry.
    limit: usize,
    /// Speculative reads the next statement may carry.
    ramp: usize,
    /// Reads the evaluator has asked for.
    demanded: u64,
    /// Speculative reads sent so far.
    speculated: u64,
    /// Hinted reads, soonest first.
    pending: VecDeque<BatchRead>,
    /// What `pending` holds or a statement has carried, so a hint is sent once.
    known: HashSet<BatchRead>,
    answers: HashMap<BatchRead, BatchAnswer>,
}

impl ReadAhead {
    /// `None` if a store's limit leaves no room for anything but the read asked for.
    pub(crate) fn new(limit: usize) -> Option<Self> {
        (limit >= 2).then(|| Self {
            limit,
            ramp: FIRST_RAMP.min(limit - 1),
            demanded: 0,
            speculated: 0,
            pending: VecDeque::new(),
            known: HashSet::new(),
            answers: HashMap::new(),
        })
    }

    /// Queues `reads` ahead of older hints: evaluation is depth-first, so what was hinted
    /// last is reached first.
    pub(crate) fn hint(&mut self, reads: Vec<BatchRead>) {
        for read in reads.into_iter().rev() {
            if self.known.insert(read.clone()) {
                self.pending.push_front(read);
            }
        }
    }

    /// Lets the next statement carry as many reads as the store allows.
    pub(crate) fn open_ramp(&mut self) {
        self.ramp = self.limit - 1;
    }

    fn read(
        &mut self,
        store: &dyn ReadStore,
        rev: Revision,
        read: BatchRead,
    ) -> Result<BatchAnswer, StoreError> {
        self.demanded = self.demanded.saturating_add(1);
        if let Some(answer) = self.answers.get(&read) {
            return Ok(answer.clone());
        }
        // This read goes either way, so it is not sent twice.
        self.known.insert(read.clone());
        self.pending.retain(|r| *r != read);

        let allowed = u64::try_from(self.limit)
            .unwrap_or(u64::MAX)
            .saturating_add(self.demanded.saturating_mul(SPECULATION_PER_READ))
            .saturating_sub(self.speculated);
        let room = self
            .ramp
            .min(usize::try_from(allowed).unwrap_or(usize::MAX));
        let mut batch = Vec::with_capacity(room.min(self.pending.len()) + 1);
        batch.push(read);
        while batch.len() <= room {
            match self.pending.pop_front() {
                Some(r) => batch.push(r),
                None => break,
            }
        }
        self.speculated = self
            .speculated
            .saturating_add(u64::try_from(batch.len() - 1).unwrap_or(u64::MAX));
        self.ramp = self.ramp.saturating_mul(2).min(self.limit - 1);

        let mut answers = store.read_batch(rev, &batch)?;
        if answers.len() != batch.len() {
            return Err(broken("the wrong number of answers"));
        }
        let first = answers.first_mut().and_then(Option::take);
        let first = first.ok_or_else(|| broken("no answer to the read asked for"))?;
        let mut reads = batch.into_iter();
        let asked = reads.next().ok_or_else(|| broken("an empty batch"))?;
        for (r, answer) in reads.zip(answers.into_iter().skip(1)) {
            // Unanswered: left out, so it is read singly if evaluation asks for it.
            if let Some(answer) = answer {
                self.answers.insert(r, answer);
            }
        }
        self.answers.insert(asked, first.clone());
        if self.answers.len() > MAX_ANSWERS {
            self.answers.clear();
            self.known = self.pending.iter().cloned().collect();
        }
        Ok(first)
    }
}

/// A store broke `read_batch`'s contract. An error, so evaluation fails closed.
fn broken(what: &str) -> StoreError {
    StoreError::Backend {
        message: format!("internal: read_batch returned {what}"),
    }
}

/// [`ReadStore::exists`], through `ahead` on a store that batches.
pub(crate) fn exists(
    ahead: Option<&mut ReadAhead>,
    store: &dyn ReadStore,
    rev: Revision,
    rel: &Relationship,
) -> Result<bool, StoreError> {
    let Some(ahead) = ahead else {
        return store.exists(rev, rel);
    };
    match ahead.read(store, rev, BatchRead::Exists(rel.clone()))? {
        BatchAnswer::Exists(found) => Ok(found),
        _ => Err(broken("the wrong kind of answer")),
    }
}

/// [`ReadStore::read_subject_sets`], through `ahead` on a store that batches.
pub(crate) fn subject_sets(
    ahead: Option<&mut ReadAhead>,
    store: &dyn ReadStore,
    rev: Revision,
    resource: &ObjectRef,
    relation: &RelName,
) -> Result<Vec<(ObjectRef, RelName)>, StoreError> {
    let Some(ahead) = ahead else {
        return store.read_subject_sets(rev, resource, relation);
    };
    let read = BatchRead::SubjectSets {
        resource: resource.clone(),
        relation: relation.clone(),
    };
    match ahead.read(store, rev, read)? {
        BatchAnswer::SubjectSets(sets) => Ok(sets),
        _ => Err(broken("the wrong kind of answer")),
    }
}

/// [`ReadStore::read_related_objects`], through `ahead` on a store that batches.
pub(crate) fn related_objects(
    ahead: Option<&mut ReadAhead>,
    store: &dyn ReadStore,
    rev: Revision,
    resource: &ObjectRef,
    relation: &RelName,
) -> Result<Vec<ObjectRef>, StoreError> {
    let Some(ahead) = ahead else {
        return store.read_related_objects(rev, resource, relation);
    };
    let read = BatchRead::RelatedObjects {
        resource: resource.clone(),
        relation: relation.clone(),
    };
    match ahead.read(store, rev, read)? {
        BatchAnswer::RelatedObjects(objects) => Ok(objects),
        _ => Err(broken("the wrong kind of answer")),
    }
}
