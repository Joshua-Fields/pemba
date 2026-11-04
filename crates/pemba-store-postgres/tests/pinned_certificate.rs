//! `PostgresConfig::pinned_certificate_pem` (ADR-017, `docs/10-postgres.md` §2.1).
//!
//! The certificate shapes and attacks run against a small local TLS server, so they need
//! no database and always run. The rest need real servers, and report "not run" without
//! them unless `PEMBA_PG_REQUIRED` is set:
//! - `PEMBA_PG_TLS_URL` and `PEMBA_PG_TLS_CERT_FILE`: the self-signed non-CA snakeoil server.
//! - `PEMBA_PG_TLS_CA_LEAF_URL` and `PEMBA_PG_TLS_CA_LEAF_CERT_FILE`: Railway's public
//!   `postgres-ssl` image, with a `CA:TRUE` certificate issued by a private root.

#![cfg(feature = "tls")]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, channel};
use std::time::Duration;

use common::{TestSchema, admin, unique, with_param};
use pemba_core::model::{Consistency, WriteOp, WriteRequest};
use pemba_core::store::StoreError;
use pemba_core::{CheckRequest, Engine, Error};
use pemba_store_postgres::{PgTxn, PostgresConfig, PostgresStore};
use rcgen::{BasicConstraints, CertificateParams, IsCa, Issuer, KeyPair, date_time_ymd};
use rustls::crypto::ring::default_provider;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use rustls::server::{ClientHello, ResolvesServerCert};
use rustls::sign::CertifiedKey;

const SCHEMA: &str = "\
definition user {}
definition document {
    relation viewer: user
    permission view = viewer
}";

/// A certificate and its private key.
struct Identity {
    pem: String,
    der: Vec<u8>,
    key_der: Vec<u8>,
}

fn params(name: &str, is_ca: IsCa) -> CertificateParams {
    let mut p = CertificateParams::new(vec![name.to_owned()]).unwrap();
    p.is_ca = is_ca;
    p
}

fn self_signed(p: &CertificateParams) -> Identity {
    let key = KeyPair::generate().unwrap();
    let cert = p.self_signed(&key).unwrap();
    Identity {
        pem: cert.pem(),
        der: cert.der().to_vec(),
        key_der: key.serialize_der(),
    }
}

fn leaf(name: &str) -> Identity {
    self_signed(&params(name, IsCa::ExplicitNoCa))
}

/// As Railway's image: `CA:TRUE`, issued by a private root the client never trusts.
fn ca_true_issued_by_private_root(name: &str) -> Identity {
    let root_key = KeyPair::generate().unwrap();
    let root = Issuer::new(
        params("root-ca", IsCa::Ca(BasicConstraints::Unconstrained)),
        root_key,
    );
    let key = KeyPair::generate().unwrap();
    let cert = params(name, IsCa::Ca(BasicConstraints::Unconstrained))
        .signed_by(&key, &root)
        .unwrap();
    Identity {
        pem: cert.pem(),
        der: cert.der().to_vec(),
        key_der: key.serialize_der(),
    }
}

#[derive(Debug)]
struct Fixed(Arc<CertifiedKey>);

impl ResolvesServerCert for Fixed {
    fn resolve(&self, _: ClientHello<'_>) -> Option<Arc<CertifiedKey>> {
        Some(Arc::clone(&self.0))
    }
}

/// One connection's worth of a server that speaks Postgres's TLS request and then TLS,
/// with `cert` and the possibly unrelated `key`. Says whether the handshake completed.
fn tls_server(
    cert: &Identity,
    key_der: &[u8],
    version: &'static rustls::SupportedProtocolVersion,
) -> (SocketAddr, Receiver<bool>) {
    let provider = Arc::new(default_provider());
    let signer = provider
        .key_provider
        .load_private_key(PrivateKeyDer::try_from(key_der.to_vec()).unwrap())
        .unwrap();
    let certified = CertifiedKey::new(vec![CertificateDer::from(cert.der.clone())], signer);
    let config = rustls::ServerConfig::builder_with_provider(provider)
        .with_protocol_versions(&[version])
        .unwrap()
        .with_no_client_auth()
        .with_cert_resolver(Arc::new(Fixed(Arc::new(certified))));
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let (tx, rx) = channel();
    std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut request = [0u8; 8];
        stream.read_exact(&mut request).unwrap();
        stream.write_all(b"S").unwrap();
        let mut conn = rustls::ServerConnection::new(Arc::new(config)).unwrap();
        let mut done = true;
        while conn.is_handshaking() {
            if conn.complete_io(&mut stream).is_err() {
                done = false;
                break;
            }
        }
        tx.send(done).unwrap();
    });
    (addr, rx)
}

/// Opens with `pem` against the local server for `name`: the server's view of the
/// handshake, and the client's error.
fn open_against(
    cert: &Identity,
    key_der: &[u8],
    version: &'static rustls::SupportedProtocolVersion,
    name: &str,
    pem: Option<&str>,
) -> (bool, String) {
    let (addr, rx) = tls_server(cert, key_der, version);
    let dsn = format!(
        "host={name} hostaddr=127.0.0.1 port={} user=x sslmode=require",
        addr.port()
    );
    let mut config = PostgresConfig::default();
    if let Some(pem) = pem {
        config = config.with_pinned_certificate_pem(pem);
    }
    let err = PostgresStore::open(&dsn, config).expect_err("no Postgres behind it");
    let handshake = rx.recv_timeout(Duration::from_secs(10)).unwrap();
    (handshake, err.to_string())
}

/// `cert` is accepted when pinned, on both TLS versions.
fn assert_pin_accepts(cert: &Identity, connecting_to: &str) {
    for version in [&rustls::version::TLS13, &rustls::version::TLS12] {
        let (handshake, err) =
            open_against(cert, &cert.key_der, version, connecting_to, Some(&cert.pem));
        assert!(handshake, "{version:?}: {err}");
        assert!(!err.contains("certificate"), "{version:?}: {err}");
    }
}

#[test]
fn a_self_signed_non_ca_leaf_is_accepted() {
    assert_pin_accepts(&leaf("db.test"), "db.test");
}

#[test]
fn a_ca_true_certificate_from_a_private_root_is_accepted() {
    assert_pin_accepts(&ca_true_issued_by_private_root("db.test"), "db.test");
}

#[test]
fn a_certificate_for_another_name_is_accepted_when_pinned() {
    assert_pin_accepts(&leaf("localhost"), "shortline.proxy.example");
    assert_pin_accepts(&ca_true_issued_by_private_root("localhost"), "127.0.0.1");
}

#[test]
fn an_expired_certificate_is_accepted_when_pinned() {
    let mut p = params("db.test", IsCa::ExplicitNoCa);
    p.not_before = date_time_ymd(2020, 1, 1);
    p.not_after = date_time_ymd(2021, 1, 1);
    assert_pin_accepts(&self_signed(&p), "db.test");
}

#[test]
fn without_a_pin_those_certificates_are_refused() {
    for (cert, reason) in [
        (leaf("db.test"), "UnknownIssuer"),
        (
            ca_true_issued_by_private_root("db.test"),
            "CaUsedAsEndEntity",
        ),
    ] {
        let (handshake, err) = open_against(
            &cert,
            &cert.key_der,
            &rustls::version::TLS13,
            "db.test",
            None,
        );
        assert!(!handshake, "{err}");
        assert!(err.contains("TLS handshake"), "{err}");
        assert!(err.contains("invalid peer certificate"), "{err}");
        assert!(err.contains(reason), "{reason}: {err}");
    }
}

#[test]
fn a_different_certificate_is_refused() {
    let pinned = leaf("db.test");
    // The same name with a different key, then another name.
    for other in [leaf("db.test"), leaf("other.test")] {
        for version in [&rustls::version::TLS13, &rustls::version::TLS12] {
            let (handshake, err) = open_against(
                &other,
                &other.key_der,
                version,
                "db.test",
                Some(&pinned.pem),
            );
            assert!(!handshake, "{err}");
            assert!(err.contains("TLS handshake"), "{err}");
            assert!(err.contains("not the pinned certificate"), "{err}");
        }
    }
}

#[test]
fn a_certificate_issued_by_a_pinned_ca_is_refused() {
    // Trusting a CA without checking names would let it vouch for any host: never that.
    let ca_key = KeyPair::generate().unwrap();
    let ca_params = params("private-ca", IsCa::Ca(BasicConstraints::Unconstrained));
    let ca_pem = ca_params.self_signed(&ca_key).unwrap().pem();
    let issuer = Issuer::new(ca_params, ca_key);
    let key = KeyPair::generate().unwrap();
    let cert = params("db.test", IsCa::ExplicitNoCa)
        .signed_by(&key, &issuer)
        .unwrap();
    let presented = Identity {
        pem: cert.pem(),
        der: cert.der().to_vec(),
        key_der: key.serialize_der(),
    };
    let (handshake, err) = open_against(
        &presented,
        &presented.key_der,
        &rustls::version::TLS13,
        "db.test",
        Some(&ca_pem),
    );
    assert!(!handshake, "{err}");
    assert!(err.contains("not the pinned certificate"), "{err}");
}

#[test]
fn the_pinned_certificate_without_its_key_is_refused() {
    // A certificate is public, so anyone can present it: the handshake signature is the proof.
    let pinned = leaf("db.test");
    let attacker = leaf("db.test");
    for version in [&rustls::version::TLS13, &rustls::version::TLS12] {
        let (handshake, err) = open_against(
            &pinned,
            &attacker.key_der,
            version,
            "db.test",
            Some(&pinned.pem),
        );
        assert!(!handshake, "{version:?}: {err}");
        assert!(
            err.contains("TLS handshake") && err.contains("BadSignature"),
            "{err}"
        );
    }
}

/// A server that refuses TLS and reports what the client sends next.
fn no_tls_server() -> (SocketAddr, Receiver<Vec<u8>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let (tx, rx) = channel();
    std::thread::spawn(move || {
        let (mut stream, _): (TcpStream, _) = listener.accept().unwrap();
        let mut request = [0u8; 8];
        stream.read_exact(&mut request).unwrap();
        stream.write_all(b"N").unwrap();
        stream
            .set_read_timeout(Some(Duration::from_millis(500)))
            .unwrap();
        let mut after = [0u8; 256];
        let n = stream.read(&mut after).unwrap_or(0);
        tx.send(after[..n].to_vec()).unwrap();
    });
    (addr, rx)
}

#[test]
fn a_pin_never_falls_back_to_plaintext() {
    let pem = leaf("db.test").pem;
    // No sslmode at all is `prefer`, the driver's default.
    for mode in ["", "sslmode=prefer", "sslmode=require"] {
        let (addr, rx) = no_tls_server();
        let dsn = format!("host=127.0.0.1 port={} user=victim {mode}", addr.port());
        let err = PostgresStore::open(
            &dsn,
            PostgresConfig::default().with_pinned_certificate_pem(pem.as_str()),
        )
        .err()
        .unwrap()
        .to_string();
        assert!(err.contains("server does not support TLS"), "{mode}: {err}");
        let sent = rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(sent.is_empty(), "{mode}: sent in clear: {sent:?}");
    }
}

#[test]
fn without_a_pin_prefer_still_falls_back_as_documented() {
    let (addr, rx) = no_tls_server();
    let dsn = format!(
        "host=127.0.0.1 port={} user=someone sslmode=prefer",
        addr.port()
    );
    assert!(PostgresStore::open(&dsn, PostgresConfig::default()).is_err());
    let sent = rx.recv_timeout(Duration::from_secs(5)).unwrap();
    let sent = String::from_utf8_lossy(&sent);
    assert!(
        sent.contains("someone"),
        "the startup message, in clear: {sent}"
    );
}

#[test]
fn a_pin_with_sslmode_disable_is_refused_before_connecting() {
    // Nothing listens on the port, so an attempt to connect would be a store error.
    let err = PostgresStore::open(
        "host=127.0.0.1 port=1 user=x sslmode=disable",
        PostgresConfig::default().with_pinned_certificate_pem(leaf("db.test").pem),
    )
    .err()
    .unwrap();
    match err {
        Error::InvalidRequest { message } => {
            assert!(message.contains("sslmode=disable"), "{message}");
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn unusable_pem_fails_clearly_at_open_without_quoting_it() {
    let good = leaf("db.test");
    let key = KeyPair::generate().unwrap().serialize_pem();
    let second = leaf("db.test").pem;
    let one_line = good
        .pem
        .trim_end()
        .replace("\r\n", "\\n")
        .replace('\n', "\\n");
    let not_x509 = "-----BEGIN CERTIFICATE-----\naGVsbG8gc2VjcmV0\n-----END CERTIFICATE-----\n";
    let bad_base64 = "-----BEGIN CERTIFICATE-----\nnot*base64*secret\n-----END CERTIFICATE-----\n";
    let cases: Vec<(&str, String, &str)> = vec![
        ("empty", String::new(), "holds no certificate"),
        ("blank", " \n\t\n".to_owned(), "holds no certificate"),
        (
            "a path",
            "/etc/ssl/secret-path.pem".to_owned(),
            "holds no certificate",
        ),
        ("bad base64", bad_base64.to_owned(), "is not valid PEM"),
        ("not X.509", not_x509.to_owned(), "readable X.509"),
        ("a literal \\n", one_line, "literal \\n"),
        ("only a key", key.clone(), "other than a certificate"),
        (
            "key then certificate",
            format!("{key}{}", good.pem),
            "other than a certificate",
        ),
        (
            "certificate then key",
            format!("{}{key}", good.pem),
            "other than a certificate",
        ),
        (
            "two certificates",
            format!("{}{second}", good.pem),
            "more than one certificate",
        ),
    ];
    for (label, pem, expected) in cases {
        let err = PostgresStore::open(
            // A password that must not appear either; nothing listens on the port, so
            // connecting instead of refusing would be a store error.
            "host=127.0.0.1 port=1 user=u password=dsn-password-value", // ggignore: fake password; the test checks it never reaches an error message
            PostgresConfig::default().with_pinned_certificate_pem(pem.clone()),
        )
        .err()
        .unwrap();
        let Error::InvalidRequest { message } = err else {
            panic!("{label}: {err:?}");
        };
        assert!(message.contains(expected), "{label}: {message}");
        assert!(
            !message.contains("dsn-password-value"),
            "{label}: {message}"
        );
        assert!(!message.contains("secret"), "{label}: {message}");
        for line in pem
            .lines()
            .filter(|l| l.len() > 20 && !l.starts_with("-----"))
        {
            assert!(!message.contains(line), "{label}: {message}");
        }
    }
}

#[test]
fn debug_does_not_print_the_pem() {
    let pem = leaf("db.test").pem;
    let debug = format!(
        "{:?}",
        PostgresConfig::default().with_pinned_certificate_pem(pem)
    );
    assert!(debug.contains("<set>"), "{debug}");
    assert!(!debug.contains("BEGIN"), "{debug}");
    assert!(format!("{:?}", PostgresConfig::default()).contains("pinned_certificate_pem: None"));
}

// ---- Against real servers ----

struct Server {
    url: String,
    pem: String,
}

fn server(url: &str, cert_file: &str) -> Option<Server> {
    let found = std::env::var(url)
        .ok()
        .filter(|u| !u.is_empty())
        .zip(std::env::var(cert_file).ok().filter(|f| !f.is_empty()));
    let Some((url_value, file)) = found else {
        assert!(
            std::env::var_os("PEMBA_PG_REQUIRED").is_none(),
            "PEMBA_PG_REQUIRED is set but {url} or {cert_file} is not"
        );
        eprintln!("not run: set {url} and {cert_file}");
        return None;
    };
    Some(Server {
        url: url_value,
        pem: std::fs::read_to_string(file).unwrap(),
    })
}

fn snakeoil() -> Option<Server> {
    server("PEMBA_PG_TLS_URL", "PEMBA_PG_TLS_CERT_FILE")
}

fn ca_leaf() -> Option<Server> {
    server("PEMBA_PG_TLS_CA_LEAF_URL", "PEMBA_PG_TLS_CA_LEAF_CERT_FILE")
}

/// Both certificates name `localhost`, so this host is one neither is valid for.
fn at_ip(url: &str) -> String {
    url.replace("@localhost:", "@127.0.0.1:")
}

fn pinned(schema: &TestSchema, pem: &str) -> PostgresConfig {
    PostgresConfig::default()
        .with_schema(&schema.name)
        .with_pool_size(2)
        .with_auto_migrate(true)
        .with_pinned_certificate_pem(pem)
}

fn can_view(engine: &Engine<PostgresStore>, user: &str) -> bool {
    let mut request = CheckRequest::new(
        "document:readme".parse().unwrap(),
        "view".parse().unwrap(),
        format!("user:{user}").parse().unwrap(),
    );
    request.consistency = Consistency::Latest;
    engine.check(request).unwrap().allowed
}

fn viewer(user: &str) -> WriteRequest {
    WriteRequest::new(vec![WriteOp::Touch(
        format!("document:readme#viewer@user:{user}")
            .parse()
            .unwrap(),
    )])
}

/// Schema, write and check on a pinned store, then a write in the app's own transaction.
fn assert_works(url: &str, pem: &str) {
    let schema = TestSchema::new(url);
    let store = PostgresStore::open(url, pinned(&schema, pem)).unwrap();
    let engine = Engine::builder().store(store).build().unwrap();
    engine.write_schema(SCHEMA).unwrap();
    engine.write(viewer("anne")).unwrap();
    assert!(can_view(&engine, "anne"));
    assert!(!can_view(&engine, "bob"));

    // The app's connection has its own TLS settings; only the engine's own is pinned.
    let mut app = admin(url);
    let mut tx = app.transaction().unwrap();
    let pending = engine
        .write_in(&mut PgTxn::new(&mut tx), viewer("bob"))
        .unwrap();
    assert!(!can_view(&engine, "bob"), "not committed");
    tx.commit().unwrap();
    engine.resolve(&pending).unwrap();
    assert!(can_view(&engine, "bob"));
}

#[test]
fn the_correct_certificate_connects_and_works() {
    let Some(s) = snakeoil() else { return };
    assert_works(&s.url, &s.pem);
}

#[test]
fn a_real_certificate_for_another_name_connects_and_works() {
    let Some(s) = snakeoil() else { return };
    assert_works(&at_ip(&s.url), &s.pem);
}

#[test]
fn a_ca_true_certificate_connects_and_works_over_scram() {
    let Some(s) = ca_leaf() else { return };
    assert_works(&s.url, &s.pem);
    assert_works(&at_ip(&s.url), &s.pem);
    // SCRAM bound to the pinned certificate. Only the engine's DSN: the plaintext test
    // connections cannot bind to anything.
    let schema = TestSchema::new(&s.url);
    let store = PostgresStore::open(
        &with_param(&at_ip(&s.url), "channel_binding", "require"),
        pinned(&schema, &s.pem),
    )
    .unwrap();
    let engine = Engine::builder().store(store).build().unwrap();
    engine.write_schema(SCHEMA).unwrap();
    engine.write(viewer("anne")).unwrap();
    assert!(can_view(&engine, "anne"));
}

/// The error Railway's image gives without a pin, which is the one Needle logged in
/// production, whatever name the client connects to.
#[test]
fn a_ca_true_certificate_without_a_pin_fails_with_ca_used_as_end_entity() {
    let Some(s) = ca_leaf() else { return };
    for url in [s.url.clone(), at_ip(&s.url)] {
        for mode in ["prefer", "require"] {
            let err = PostgresStore::open(
                &with_param(&url, "sslmode", mode),
                PostgresConfig::default(),
            )
            .err()
            .unwrap();
            let Error::Store(StoreError::Backend { message }) = err else {
                panic!("{err:?}");
            };
            assert_eq!(
                message,
                "error performing TLS handshake: invalid peer certificate: \
                 Other(OtherError(CaUsedAsEndEntity))",
                "{mode}"
            );
        }
    }
}

#[test]
fn the_default_config_refuses_an_untrusted_server() {
    let Some(s) = snakeoil() else { return };
    for mode in ["prefer", "require"] {
        let err = PostgresStore::open(
            &with_param(&s.url, "sslmode", mode),
            PostgresConfig::default(),
        )
        .err()
        .unwrap();
        let Error::Store(StoreError::Backend { message }) = err else {
            panic!("{err:?}");
        };
        assert!(message.contains("TLS handshake"), "{mode}: {message}");
        assert!(message.contains("certificate"), "{mode}: {message}");
    }
}

#[test]
fn a_different_real_certificate_is_refused() {
    let Some(s) = snakeoil() else { return };
    // The same name as the server's certificate, with a different key.
    let schema = TestSchema::new(&s.url);
    let err = PostgresStore::open(&s.url, pinned(&schema, &leaf("localhost").pem))
        .err()
        .unwrap()
        .to_string();
    assert!(err.contains("TLS handshake"), "{err}");
    assert!(err.contains("not the pinned certificate"), "{err}");
}

#[test]
fn sslmode_require_and_disable_keep_their_meaning() {
    let Some(s) = snakeoil() else { return };
    let schema = TestSchema::new(&s.url);
    let config = || {
        PostgresConfig::default()
            .with_schema(&schema.name)
            .with_auto_migrate(true)
    };
    // Without a pin: `require` verifies and fails, `disable` connects without TLS.
    let require = with_param(&s.url, "sslmode", "require");
    let disable = with_param(&s.url, "sslmode", "disable");
    assert!(PostgresStore::open(&require, config()).is_err());
    PostgresStore::open(&disable, config()).unwrap();
    // With one: `require` connects, `disable` is refused.
    PostgresStore::open(&require, pinned(&schema, &s.pem)).unwrap();
    assert!(matches!(
        PostgresStore::open(&disable, pinned(&schema, &s.pem)),
        Err(Error::InvalidRequest { .. })
    ));
}

#[test]
fn prefer_with_a_pin_really_uses_tls() {
    let Some(s) = snakeoil() else { return };
    let schema = TestSchema::new(&s.url);
    let name = unique("pemba_pin_");
    let url = with_param(
        &with_param(&s.url, "sslmode", "prefer"),
        "application_name",
        &name,
    );
    let store = PostgresStore::open(&url, pinned(&schema, &s.pem)).unwrap();
    store.check_storage_version().unwrap();
    let row = admin(&s.url)
        .query_one(
            "SELECT count(*), count(*) FILTER (WHERE s.ssl) FROM pg_stat_activity a \
             JOIN pg_stat_ssl s USING (pid) WHERE a.application_name = $1",
            &[&name],
        )
        .unwrap();
    let (connections, encrypted): (i64, i64) = (row.get(0), row.get(1));
    assert!(connections > 0);
    assert_eq!(connections, encrypted);
}
