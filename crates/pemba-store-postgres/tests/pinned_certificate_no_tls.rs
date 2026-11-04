//! A pinned certificate cannot be honoured by a build without TLS (ADR-017).

#![cfg(not(feature = "tls"))]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use pemba_core::Error;
use pemba_store_postgres::{PostgresConfig, PostgresStore};

#[test]
fn a_pin_without_the_tls_feature_is_refused_rather_than_plaintext() {
    // Nothing listens on the port, so an attempt to connect would be a store error.
    let err = PostgresStore::open(
        "host=127.0.0.1 port=1 user=x",
        PostgresConfig::default().with_pinned_certificate_pem("anything"),
    )
    .err()
    .unwrap();
    match err {
        Error::InvalidRequest { message } => {
            assert!(message.contains("`tls` feature"), "{message}");
        }
        other => panic!("{other:?}"),
    }
}
