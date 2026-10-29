//! Downstream construction and string-helper contract.
#![allow(clippy::unwrap_used)]
#![cfg(feature = "memory")]

use pemba::model::{Consistency, Revision, WriteOp, WriteRequest};
use pemba::{CheckItem, CheckRequest, Engine, Error, MemoryStore};

#[test]
fn downstream_requests_and_check_str_preserve_answers_and_errors() {
    let engine = Engine::builder().store(MemoryStore::new()).build().unwrap();
    assert!(matches!(
        engine.check_str("document:x", "view", "user:a", Consistency::Latest),
        Err(Error::NoSchema)
    ));
    engine.write_schema("definition user {} definition document { relation viewer: user permission view = viewer }").unwrap();
    let revision = engine
        .write(WriteRequest::new(vec![WriteOp::Touch(
            "document:x#viewer@user:a".parse().unwrap(),
        )]))
        .unwrap();
    let mut request = CheckRequest::new(
        "document:x".parse().unwrap(),
        "view".parse().unwrap(),
        "user:a".parse().unwrap(),
    );
    request.consistency = Consistency::Exact(revision);
    assert!(engine.check(request).unwrap().allowed);
    let item = CheckItem::new(
        "document:x".parse().unwrap(),
        "view".parse().unwrap(),
        "user:a".parse().unwrap(),
    );
    assert!(
        engine.check_bulk(vec![item], Consistency::Latest).unwrap()[0]
            .as_ref()
            .unwrap()
    );
    assert!(
        engine
            .check_str(
                "document:x",
                "view",
                "user:a",
                Consistency::AtLeast(revision)
            )
            .unwrap()
    );
    assert!(
        !engine
            .check_str("document:x", "view", "user:b", Consistency::Latest)
            .unwrap()
    );
    assert!(
        !engine
            .check_str(
                "document:x",
                "view",
                "user:a",
                Consistency::Exact(Revision(1))
            )
            .unwrap()
    );
    assert!(matches!(
        engine.check_str(
            "document:x",
            "view",
            "user:a",
            Consistency::Exact(Revision(u64::MAX))
        ),
        Err(Error::RevisionNotYetAvailable { .. })
    ));
    assert!(matches!(
        engine.check_str("document:x", "missing", "user:a", Consistency::Latest),
        Err(Error::InvalidRequest { .. })
    ));
    assert!(matches!(
        engine.check_str("document:x", "view", "user:*", Consistency::Latest),
        Err(Error::InvalidRequest { .. })
    ));
    for (resource, permission, subject, input, position) in [
        ("document", "view", "user:a", "document", 8),
        ("document:x", "bad name", "user:a", "bad name", 3),
        ("document:x", "view", "user", "user", 4),
    ] {
        let Err(Error::Parse(error)) =
            engine.check_str(resource, permission, subject, Consistency::Latest)
        else {
            panic!("expected a parse error")
        };
        assert_eq!(error.input, input);
        assert_eq!(error.position, position);
    }
}
