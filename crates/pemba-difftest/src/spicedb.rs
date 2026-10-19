//! A blocking client for the parts of SpiceDB's HTTP API the harness needs
//! (<https://authzed.com/docs/spicedb/api/http-api>). Every read is fully consistent.

use std::collections::BTreeSet;
use std::time::Duration;

use serde_json::{Value, json};
use pemba_core::model::{ObjectRef, RelName, Relationship, SubjectRef, TypeName};

use crate::{Answer, Question};

/// Most updates SpiceDB accepts in one write by default is 1 000.
const WRITE_CHUNK: usize = 500;
/// Items per `CheckBulkPermissions` request.
const CHECK_CHUNK: usize = 25;

/// A SpiceDB server's HTTP gateway.
#[derive(Debug)]
pub struct SpiceDb {
    base: String,
    key: String,
    agent: ureq::Agent,
}

impl SpiceDb {
    /// `PEMBA_SPICEDB_URL` and `PEMBA_SPICEDB_KEY`, if both are set.
    pub fn from_env() -> Option<Self> {
        let base = std::env::var("PEMBA_SPICEDB_URL").ok()?;
        let key = std::env::var("PEMBA_SPICEDB_KEY").ok()?;
        Some(Self::new(&base, &key))
    }

    /// A client for the gateway at `base`; `key` prefixes every token.
    pub fn new(base: &str, key: &str) -> Self {
        let agent = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .timeout_global(Some(Duration::from_secs(120)))
            .build()
            .into();
        Self {
            base: base.trim_end_matches('/').to_owned(),
            key: key.to_owned(),
            agent,
        }
    }

    /// The bearer token for case `name`: under `spicedb serve-testing` each token is a
    /// separate, empty datastore.
    pub fn token(&self, name: &str) -> String {
        format!("{}-{name}", self.key)
    }

    /// The response body; an HTTP error status is an `Err` with the body.
    fn post(&self, token: &str, path: &str, body: &Value) -> Result<String, String> {
        let url = format!("{}{path}", self.base);
        let mut resp = self
            .agent
            .post(&url)
            .header("Authorization", &format!("Bearer {}", self.token(token)))
            .send_json(body)
            .map_err(|e| format!("POST {path}: {e}"))?;
        let status = resp.status();
        let text = resp
            .body_mut()
            .read_to_string()
            .map_err(|e| format!("POST {path}: {e}"))?;
        if status.is_success() {
            Ok(text)
        } else {
            Err(error_message(&text))
        }
    }

    /// `WriteSchema`.
    ///
    /// # Errors
    ///
    /// SpiceDB's message if it refuses the schema or cannot be reached.
    pub fn write_schema(&self, token: &str, schema: &str) -> Result<(), String> {
        self.post(token, "/v1/schema/write", &json!({ "schema": schema }))
            .map(drop)
            .map_err(|e| format!("SpiceDB refused the schema: {e}"))
    }

    /// `WriteRelationships` with `OPERATION_TOUCH`, in chunks.
    ///
    /// # Errors
    ///
    /// SpiceDB's message if it refuses a tuple or cannot be reached.
    pub fn write_tuples(&self, token: &str, tuples: &[Relationship]) -> Result<(), String> {
        for chunk in tuples.chunks(WRITE_CHUNK) {
            let updates: Vec<Value> = chunk
                .iter()
                .map(|t| {
                    json!({
                        "operation": "OPERATION_TOUCH",
                        "relationship": {
                            "resource": object(&t.resource),
                            "relation": t.relation.as_str(),
                            "subject": subject(&t.subject),
                        }
                    })
                })
                .collect();
            self.post(
                token,
                "/v1/relationships/write",
                &json!({ "updates": updates }),
            )
            .map_err(|e| format!("SpiceDB refused the tuples: {e}"))?;
        }
        Ok(())
    }

    /// `DeleteRelationships` for every resource type in `types`: `serve-testing` keeps
    /// each token's datastore for the life of the server, so a finished case frees what it
    /// can.
    ///
    /// # Errors
    ///
    /// SpiceDB's message if a delete fails.
    pub fn clear(&self, token: &str, types: &[&str]) -> Result<(), String> {
        for ty in types {
            let body = json!({ "relationshipFilter": { "resourceType": ty } });
            self.post(token, "/v1/relationships/delete", &body)
                .map_err(|e| format!("SpiceDB could not delete {ty} tuples: {e}"))?;
        }
        Ok(())
    }

    /// `CheckBulkPermissions` for every [`Question::Check`] in `checks`, in order.
    ///
    /// # Errors
    ///
    /// If a request fails as a whole; a per-item error is an [`Answer::Error`].
    pub(crate) fn check_bulk(
        &self,
        token: &str,
        checks: &[&Question],
    ) -> Result<Vec<Answer>, String> {
        let mut out = Vec::with_capacity(checks.len());
        for chunk in checks.chunks(CHECK_CHUNK) {
            let items: Vec<Value> = chunk
                .iter()
                .filter_map(|q| match q {
                    Question::Check(r, p, s) => Some(json!({
                        "resource": object(r),
                        "permission": p.as_str(),
                        "subject": subject(s),
                    })),
                    _ => None,
                })
                .collect();
            let body = json!({ "consistency": { "fullyConsistent": true }, "items": items });
            let text = self.post(token, "/v1/permissions/checkbulk", &body)?;
            let v: Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
            let pairs = v["pairs"]
                .as_array()
                .ok_or_else(|| format!("no `pairs` in {text}"))?;
            if pairs.len() != items.len() {
                return Err(format!(
                    "{} results for {} checks",
                    pairs.len(),
                    items.len()
                ));
            }
            out.extend(pairs.iter().map(|p| {
                if let Some(e) = p.get("error") {
                    Answer::Error(error_message(&e.to_string()))
                } else {
                    permissionship(&p["item"]["permissionship"])
                }
            }));
        }
        Ok(out)
    }

    /// `LookupResources`, every result.
    pub(crate) fn lookup_resources(
        &self,
        token: &str,
        ty: &TypeName,
        permission: &RelName,
        s: &SubjectRef,
    ) -> Answer {
        let body = json!({
            "consistency": { "fullyConsistent": true },
            "resourceObjectType": ty.as_str(),
            "permission": permission.as_str(),
            "subject": subject(s),
        });
        let results = match self.stream(token, "/v1/permissions/resources", &body) {
            Ok(r) => r,
            Err(e) => return Answer::Error(e),
        };
        let mut ids = Vec::new();
        for r in results {
            if r["permissionship"] != "LOOKUP_PERMISSIONSHIP_HAS_PERMISSION" {
                return Answer::Error(format!("unexpected result {r}"));
            }
            match r["resourceObjectId"].as_str() {
                Some(id) => ids.push(id.to_owned()),
                None => return Answer::Error(format!("unexpected result {r}")),
            }
        }
        ids.sort();
        ids.dedup();
        Answer::Resources(ids)
    }

    /// `LookupSubjects`, every result.
    pub(crate) fn lookup_subjects(
        &self,
        token: &str,
        resource: &ObjectRef,
        permission: &RelName,
        subject_type: &TypeName,
    ) -> Answer {
        let body = json!({
            "consistency": { "fullyConsistent": true },
            "resource": object(resource),
            "permission": permission.as_str(),
            "subjectObjectType": subject_type.as_str(),
        });
        let results = match self.stream(token, "/v1/permissions/subjects", &body) {
            Ok(r) => r,
            Err(e) => return Answer::Error(e),
        };
        let (mut ids, mut wildcard, mut excluded) = (BTreeSet::new(), false, BTreeSet::new());
        for r in results {
            let s = &r["subject"];
            if s["permissionship"] != "LOOKUP_PERMISSIONSHIP_HAS_PERMISSION" {
                return Answer::Error(format!("unexpected result {r}"));
            }
            match s["subjectObjectId"].as_str() {
                Some("*") => {
                    wildcard = true;
                    for e in r["excludedSubjects"].as_array().into_iter().flatten() {
                        if let Some(id) = e["subjectObjectId"].as_str() {
                            excluded.insert(id.to_owned());
                        }
                    }
                }
                Some(id) => {
                    ids.insert(id.to_owned());
                }
                None => return Answer::Error(format!("unexpected result {r}")),
            }
        }
        if wildcard {
            // pemba's answer lists no ids beside a wildcard; the excluded ids say it all.
            Answer::Subjects(Vec::new(), true, excluded.into_iter().collect())
        } else {
            Answer::Subjects(ids.into_iter().collect(), false, Vec::new())
        }
    }

    /// A streaming endpoint's `result` objects; an `error` object is an `Err`.
    fn stream(&self, token: &str, path: &str, body: &Value) -> Result<Vec<Value>, String> {
        let text = self.post(token, path, body)?;
        let mut out = Vec::new();
        for line in text.lines().filter(|l| !l.trim().is_empty()) {
            let v: Value = serde_json::from_str(line).map_err(|e| format!("{e}: {line}"))?;
            if let Some(e) = v.get("error") {
                return Err(error_message(&e.to_string()));
            }
            out.push(v["result"].clone());
        }
        Ok(out)
    }
}

fn object(o: &ObjectRef) -> Value {
    json!({ "objectType": o.object_type.as_str(), "objectId": o.object_id.as_str() })
}

fn subject(s: &SubjectRef) -> Value {
    match s {
        SubjectRef::Object(o) => json!({ "object": object(o) }),
        SubjectRef::Set {
            object: o,
            relation,
        } => {
            json!({ "object": object(o), "optionalRelation": relation.as_str() })
        }
        SubjectRef::Wildcard { object_type } => {
            json!({ "object": { "objectType": object_type.as_str(), "objectId": "*" } })
        }
    }
}

fn permissionship(p: &Value) -> Answer {
    match p.as_str() {
        Some("PERMISSIONSHIP_HAS_PERMISSION") => Answer::Allowed(true),
        Some("PERMISSIONSHIP_NO_PERMISSION") => Answer::Allowed(false),
        _ => Answer::Error(format!("unexpected permissionship {p}")),
    }
}

/// The `message` of a gRPC-style error body, or the body itself.
fn error_message(body: &str) -> String {
    serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|v| v["message"].as_str().map(str::to_owned))
        .unwrap_or_else(|| body.to_owned())
}
