//! T6.0 spike: the engine on the memory store, exposed to JS with wasm-bindgen.
//! Every method returns a JSON string so the JS side needs no glue types.

use std::fmt::Write as _;
use std::path::Path;

use pemba_core::model::{ObjectRef, Relationship, SubjectRef, WriteOp, WriteRequest};
use pemba_core::{
    CheckRequest, Engine, EngineConfig, Error, LookupResourcesRequest, LookupSubjectsRequest,
    SchemaError,
};
use pemba_store_memory::MemoryStore;
use wasm_bindgen::prelude::*;

#[wasm_bindgen]
pub struct Playground {
    engine: Engine<MemoryStore>,
}

fn s(out: &mut String, v: &str) {
    out.push('"');
    for c in v.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

fn err(e: &Error) -> String {
    let mut o = String::from("{\"error\":{\"code\":");
    s(&mut o, e.code());
    o.push_str(",\"message\":");
    s(&mut o, &e.to_string());
    o.push_str("}}");
    o
}

fn diags(out: &mut String, list: &[SchemaError]) {
    out.push('[');
    for (i, d) in list.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        out.push_str("{\"code\":");
        s(out, &format!("{:?}", d.code));
        out.push_str(",\"message\":");
        s(out, &d.message);
        let _ = write!(
            out,
            ",\"line\":{},\"col\":{},\"start\":{},\"end\":{}",
            d.line, d.col, d.span.start, d.span.end
        );
        out.push_str(",\"help\":");
        match &d.help {
            Some(h) => s(out, h),
            None => out.push_str("null"),
        }
        out.push('}');
    }
    out.push(']');
}

fn ids<T: AsRef<str>>(out: &mut String, list: impl IntoIterator<Item = T>) {
    out.push('[');
    for (i, id) in list.into_iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        s(out, id.as_ref());
    }
    out.push(']');
}

fn parse<T: std::str::FromStr>(v: &str, what: &str) -> Result<T, String>
where
    T::Err: std::fmt::Display,
{
    v.trim().parse().map_err(|e| {
        let mut o = String::from("{\"error\":{\"code\":\"E_PARSE\",\"message\":");
        s(&mut o, &format!("{what} `{v}`: {e}"));
        o.push_str("}}");
        o
    })
}

#[wasm_bindgen]
impl Playground {
    /// A fresh engine on an empty memory store.
    #[wasm_bindgen(constructor)]
    pub fn new() -> Result<Playground, JsError> {
        let engine = Engine::builder()
            .store(MemoryStore::new())
            .config(EngineConfig::default())
            .build()
            .map_err(|e| JsError::new(&e.to_string()))?;
        Ok(Playground { engine })
    }

    /// Compiles and stores a schema: `{ok, errors, warnings}`.
    pub fn write_schema(&self, source: &str) -> String {
        let mut o = String::new();
        match self.engine.write_schema(source) {
            Ok(r) => {
                o.push_str("{\"ok\":true,\"errors\":[],\"warnings\":");
                diags(&mut o, &r.warnings);
                o.push('}');
            }
            Err(Error::Schema(e)) => {
                o.push_str("{\"ok\":false,\"errors\":");
                diags(&mut o, &e.errors);
                o.push_str(",\"warnings\":");
                diags(&mut o, &e.warnings);
                o.push('}');
            }
            Err(e) => return err(&e),
        }
        o
    }

    /// Touches one relationship per non-empty, non-`//` line.
    pub fn write(&self, text: &str) -> String {
        let mut ops = Vec::new();
        for line in text.lines().map(str::trim) {
            if line.is_empty() || line.starts_with("//") {
                continue;
            }
            match parse::<Relationship>(line, "relationship") {
                Ok(r) => ops.push(WriteOp::Touch(r)),
                Err(e) => return e,
            }
        }
        let n = ops.len();
        match self.engine.write(WriteRequest {
            ops,
            preconditions: Vec::new(),
        }) {
            Ok(rev) => format!("{{\"revision\":\"{rev}\",\"written\":{n}}}"),
            Err(e) => err(&e),
        }
    }

    fn req(query: &str) -> Result<CheckRequest, String> {
        let q: Relationship = parse(query, "check")?;
        Ok(CheckRequest {
            resource: q.resource,
            permission: q.relation,
            subject: q.subject,
            consistency: pemba_core::model::Consistency::Latest,
        })
    }

    /// `resource#permission@subject` -> `{allowed, revision}`.
    pub fn check(&self, query: &str) -> String {
        let req = match Self::req(query) {
            Ok(r) => r,
            Err(e) => return e,
        };
        match self.engine.check(req) {
            Ok(r) => format!("{{\"allowed\":{},\"revision\":\"{}\"}}", r.allowed, r.revision),
            Err(e) => err(&e),
        }
    }

    /// The `pemba check --json` trace.
    pub fn check_with_trace(&self, query: &str) -> String {
        let req = match Self::req(query) {
            Ok(r) => r,
            Err(e) => return e,
        };
        match self.engine.check_with_trace(req) {
            Ok((_, t)) => t.to_json(),
            Err(e) => err(&e),
        }
    }

    /// `pemba check --explain` text.
    pub fn explain(&self, query: &str) -> String {
        let req = match Self::req(query) {
            Ok(r) => r,
            Err(e) => return e,
        };
        match self.engine.check_with_trace(req) {
            Ok((_, t)) => t.render(),
            Err(e) => format!("error: {}", e),
        }
    }

    pub fn lookup_resources(&self, resource_type: &str, permission: &str, subject: &str) -> String {
        let (t, p, sub) = match (
            parse(resource_type, "type"),
            parse(permission, "permission"),
            parse::<SubjectRef>(subject, "subject"),
        ) {
            (Ok(t), Ok(p), Ok(s)) => (t, p, s),
            (Err(e), _, _) | (_, Err(e), _) | (_, _, Err(e)) => return e,
        };
        match self.engine.lookup_resources(LookupResourcesRequest::new(t, p, sub)) {
            Ok(r) => {
                let mut o = String::from("{\"ids\":");
                ids(&mut o, r.resource_ids.iter().map(|i| i.as_str()));
                let _ = write!(o, ",\"revision\":\"{}\"}}", r.revision);
                o
            }
            Err(e) => err(&e),
        }
    }

    pub fn lookup_subjects(&self, resource: &str, permission: &str, subject_type: &str) -> String {
        let (r, p, t) = match (
            parse::<ObjectRef>(resource, "resource"),
            parse(permission, "permission"),
            parse(subject_type, "type"),
        ) {
            (Ok(r), Ok(p), Ok(t)) => (r, p, t),
            (Err(e), _, _) | (_, Err(e), _) | (_, _, Err(e)) => return e,
        };
        match self.engine.lookup_subjects(LookupSubjectsRequest::new(r, p, t)) {
            Ok(r) => {
                let mut o = String::from("{\"ids\":");
                ids(&mut o, r.subject_ids.iter().map(|i| i.as_str()));
                let _ = write!(o, ",\"wildcard\":{},\"excluded\":", r.wildcard);
                ids(&mut o, r.excluded_ids.iter().map(|i| i.as_str()));
                let _ = write!(o, ",\"revision\":\"{}\"}}", r.revision);
                o
            }
            Err(e) => err(&e),
        }
    }

    /// Times `n` checks inside WASM with `performance.now()` from JS (see web/bench.js).
    pub fn check_n(&self, query: &str, n: u32) -> u32 {
        let mut allowed = 0;
        for _ in 0..n {
            if let Ok(req) = Self::req(query) {
                if self.engine.check(req).is_ok_and(|r| r.allowed) {
                    allowed += 1;
                }
            }
        }
        allowed
    }
}

#[cfg(feature = "fixtures")]
/// Runs a fixture YAML (the `pemba test` format) on the memory store: `{passed, checked, failures}`.
#[wasm_bindgen]
pub fn run_fixture(yaml: &str) -> String {
    match pemba_fixtures::run_str(yaml, Path::new("."), pemba_fixtures::StoreKind::Memory) {
        Ok(r) => {
            let mut o = format!("{{\"passed\":{},\"checked\":{},\"failures\":", r.passed(), r.checked);
            ids(&mut o, r.failures.iter().map(|f| f.to_string()));
            o.push('}');
            o
        }
        Err(e) => {
            let mut o = String::from("{\"error\":");
            s(&mut o, &e.to_string());
            o.push('}');
            o
        }
    }
}
