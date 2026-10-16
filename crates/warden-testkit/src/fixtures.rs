//! The fixture runner with the reference evaluator as its oracle (`docs/07-testing.md` §3).
//!
//! The format and the runner live in the publishable `warden-fixtures` crate, which
//! `warden test` uses against the engine alone (OQ-024). Here, every `assertTrue` /
//! `assertFalse` and `expectedLookups` entry is also answered by [`crate::reference`] over
//! the tuples the runner believes are live. The oracle's answer to a subjects lookup is a
//! wildcard when it allows its fresh id, which stands in for every object never written.
//! It declines non-stratifiable schemas, so the runner then checks the engine only.

use std::path::Path;

use warden_core::model::{ObjectRef, RelName, Relationship, SubjectRef};
use warden_core::{LookupResourcesRequest, LookupSubjectsRequest};
use warden_schema::ir::CompiledSchema;

pub use warden_fixtures::{
    Failure, FixtureError, Oracle, OracleError, OracleView, Report, StoreKind, SubjectsAnswer,
};

use crate::reference::{FRESH_ID, Reference, ReferenceError};

/// [`Reference`] as the fixture runner's [`Oracle`].
#[derive(Clone, Copy, Debug, Default)]
pub struct ReferenceOracle;

impl Oracle for ReferenceOracle {
    fn name(&self) -> &'static str {
        "reference evaluator"
    }

    fn view(
        &self,
        schema: &CompiledSchema,
        live: &[Relationship],
        objects: &[ObjectRef],
    ) -> Result<Box<dyn OracleView>, OracleError> {
        match Reference::with_objects(schema, live, objects) {
            Ok(r) => Ok(Box::new(r)),
            Err(e @ ReferenceError::NotStratifiable { .. }) => {
                Err(OracleError::Declined(e.to_string()))
            }
            Err(e) => Err(OracleError::Failed(e.to_string())),
        }
    }
}

impl OracleView for Reference {
    fn check(
        &self,
        resource: &ObjectRef,
        permission: &RelName,
        subject: &SubjectRef,
    ) -> Result<bool, String> {
        Reference::check(self, resource, permission, subject).map_err(|e| e.to_string())
    }

    /// `{ o : check(o, m, s) }` over the universe, without the stand-in fresh object
    /// (which the engine cannot name), ids ascending.
    fn lookup_resources(&self, req: &LookupResourcesRequest) -> Result<Vec<String>, String> {
        let mut ids = Vec::new();
        for o in self.universe() {
            if o.object_type == req.resource_type
                && o.object_id.as_str() != FRESH_ID
                && Reference::check(self, &o, &req.permission, &req.subject)
                    .map_err(|e| e.to_string())?
            {
                ids.push(o.object_id.to_string());
            }
        }
        ids.sort();
        Ok(ids)
    }

    /// The objects of the subject type it allows; a wildcard excluding the ones it denies
    /// if it allows its fresh id (an object never written).
    fn lookup_subjects(&self, req: &LookupSubjectsRequest) -> Result<SubjectsAnswer, String> {
        let (mut allowed, mut denied, mut fresh) = (Vec::new(), Vec::new(), false);
        for o in self.universe() {
            if o.object_type != req.subject_type {
                continue;
            }
            let yes = Reference::check(
                self,
                &req.resource,
                &req.permission,
                &SubjectRef::Object(o.clone()),
            )
            .map_err(|e| e.to_string())?;
            if o.object_id.as_str() == FRESH_ID {
                fresh = yes;
            } else if yes {
                allowed.push(o.object_id.to_string());
            } else {
                denied.push(o.object_id.to_string());
            }
        }
        Ok(if fresh {
            SubjectsAnswer {
                ids: Vec::new(),
                wildcard: true,
                excluded: denied,
            }
        } else {
            SubjectsAnswer {
                ids: allowed,
                wildcard: false,
                excluded: Vec::new(),
            }
        }
        .sorted())
    }
}

/// Runs the fixture file at `path` on `store`, cross-checked by [`ReferenceOracle`].
///
/// # Errors
///
/// As [`warden_fixtures::run`].
pub fn run(path: &Path, store: StoreKind) -> Result<Report, FixtureError> {
    warden_fixtures::run_with_oracle(path, store, &ReferenceOracle)
}

/// Runs fixture text, cross-checked by [`ReferenceOracle`]; a `schemaFile` is resolved
/// against `base_dir`.
///
/// # Errors
///
/// As [`warden_fixtures::run`].
pub fn run_str(source: &str, base_dir: &Path, store: StoreKind) -> Result<Report, FixtureError> {
    warden_fixtures::run_str_with_oracle(source, base_dir, store, &ReferenceOracle)
}
