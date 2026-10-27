//! Connecting with or without TLS (`docs/10-postgres.md` §2.1).
//!
//! With the `tls` feature (default) every connection offers TLS through rustls, and the
//! DSN's `sslmode` decides: `disable` never uses it, `prefer` (the default) uses it when
//! the server offers it, `require` refuses a server that does not. The server certificate
//! is always verified against the platform's trust store; there is no unverified mode.
//! Without the feature only plaintext is possible, so `sslmode=require` fails to connect.

use std::fmt;

use pemba_core::Error;
use postgres::{Client, Config, NoTls};

#[cfg(feature = "tls")]
use std::sync::Arc;
#[cfg(feature = "tls")]
use tokio_postgres_rustls::MakeRustlsConnect;

/// How a connection is made: plaintext only, or TLS-capable.
#[derive(Clone)]
pub(crate) enum Connector {
    /// Plaintext only (built without the `tls` feature).
    #[cfg_attr(feature = "tls", allow(dead_code))]
    Plain,
    /// TLS through rustls when the DSN's `sslmode` and the server agree.
    #[cfg(feature = "tls")]
    Rustls(MakeRustlsConnect),
}

impl fmt::Debug for Connector {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Plain => f.write_str("Plain"),
            #[cfg(feature = "tls")]
            Self::Rustls(_) => f.write_str("Rustls"),
        }
    }
}

impl Connector {
    /// The connector this build supports.
    #[cfg(feature = "tls")]
    pub(crate) fn new() -> Result<Self, Error> {
        let loaded = rustls_native_certs::load_native_certs();
        for e in &loaded.errors {
            tracing::warn!(error = %e, "pemba: could not load a platform root certificate");
        }
        let mut roots = rustls::RootCertStore::empty();
        let (_, unparsable) = roots.add_parsable_certificates(loaded.certs);
        if unparsable > 0 {
            tracing::warn!(
                unparsable,
                "pemba: skipped unparsable platform root certificates"
            );
        }
        // Our own provider rather than rustls's process default, which an application may
        // not have installed (or may have installed differently).
        let config = rustls::ClientConfig::builder_with_provider(Arc::new(
            rustls::crypto::ring::default_provider(),
        ))
        .with_safe_default_protocol_versions()
        .map_err(|e| Error::InvalidRequest {
            message: format!("cannot set up TLS: {e}"),
        })?
        .with_root_certificates(roots)
        .with_no_client_auth();
        Ok(Self::Rustls(MakeRustlsConnect::new(config)))
    }

    /// The connector this build supports.
    #[cfg(not(feature = "tls"))]
    #[expect(
        clippy::unnecessary_wraps,
        reason = "same signature as the `tls` build"
    )]
    pub(crate) fn new() -> Result<Self, Error> {
        Ok(Self::Plain)
    }

    pub(crate) fn connect(&self, config: &Config) -> Result<Client, postgres::Error> {
        match self {
            Self::Plain => config.connect(NoTls),
            #[cfg(feature = "tls")]
            Self::Rustls(m) => config.connect(m.clone()),
        }
    }
}
