//! Connecting with or without TLS (`docs/10-postgres.md` §2.1).
//!
//! With the `tls` feature (default) every connection offers TLS through rustls, and the
//! DSN's `sslmode` decides: `disable` never uses it, `prefer` (the default) uses it when
//! the server offers it, `require` refuses a server that does not. The server certificate
//! is always verified against the platform's trust store; there is no unverified mode.
//! Without the feature only plaintext is possible, so `sslmode=require` fails to connect.
//!
//! [`PostgresConfig::pinned_certificate_pem`](crate::PostgresConfig::pinned_certificate_pem)
//! replaces the trust store with one exact certificate and requires TLS (ADR-017).

use std::fmt;

use pemba_core::Error;
use postgres::config::SslMode;
use postgres::{Client, Config, NoTls};

#[cfg(feature = "tls")]
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
#[cfg(feature = "tls")]
use rustls::crypto::CryptoProvider;
#[cfg(feature = "tls")]
use rustls::pki_types::pem::{self, SectionKind};
#[cfg(feature = "tls")]
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
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
    /// The connector this build supports, trusting only `pinned` when it is given.
    #[cfg(feature = "tls")]
    pub(crate) fn new(pinned: Option<&str>) -> Result<Self, Error> {
        // Our own provider rather than rustls's process default, which an application may
        // not have installed (or may have installed differently).
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let builder = rustls::ClientConfig::builder_with_provider(Arc::clone(&provider))
            .with_safe_default_protocol_versions()
            .map_err(|e| Error::InvalidRequest {
                message: format!("cannot set up TLS: {e}"),
            })?;
        let config = if let Some(pem) = pinned {
            let verifier = PinnedCertificate {
                certificate: parse_pinned_certificate(pem)?,
                provider,
            };
            builder
                .dangerous()
                .with_custom_certificate_verifier(Arc::new(verifier))
                .with_no_client_auth()
        } else {
            builder
                .with_root_certificates(platform_roots())
                .with_no_client_auth()
        };
        Ok(Self::Rustls(MakeRustlsConnect::new(config)))
    }

    /// The connector this build supports. A pinned certificate cannot be honoured without
    /// TLS, so it is an error rather than a plaintext connection.
    #[cfg(not(feature = "tls"))]
    pub(crate) fn new(pinned: Option<&str>) -> Result<Self, Error> {
        if pinned.is_some() {
            return Err(Error::InvalidRequest {
                message: "PostgresConfig::pinned_certificate_pem needs the `tls` feature"
                    .to_owned(),
            });
        }
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

/// Makes the DSN's `sslmode` honour a pinned certificate. `prefer` would connect in
/// plaintext to anything that answers "no TLS", which an on-path attacker can do, so a pin
/// requires TLS. `disable` contradicts the pin: refuse it rather than ignore either.
pub(crate) fn require_tls_for_pin(config: &mut Config) -> Result<(), Error> {
    if config.get_ssl_mode() == SslMode::Disable {
        return Err(Error::InvalidRequest {
            message: "PostgresConfig::pinned_certificate_pem cannot be used with \
                      sslmode=disable, which would ignore it"
                .to_owned(),
        });
    }
    config.ssl_mode(SslMode::Require);
    Ok(())
}

#[cfg(feature = "tls")]
fn platform_roots() -> rustls::RootCertStore {
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
    roots
}

/// Reads the one certificate in `pem`. The messages are fixed text: PEM parser errors can
/// quote the input, which must not reach an error message or a log.
#[cfg(feature = "tls")]
fn parse_pinned_certificate(pem: &str) -> Result<CertificateDer<'static>, Error> {
    let invalid = |problem: &str| Error::InvalidRequest {
        message: format!("PostgresConfig::pinned_certificate_pem {problem}"),
    };
    let mut rest = pem.as_bytes();
    let mut certificate = None;
    loop {
        let section = pem::from_buf(&mut rest).map_err(|_| {
            invalid(
                "is not valid PEM (if it came from an environment variable, check that its \
                 line breaks are real newlines rather than a literal \\n)",
            )
        })?;
        match section {
            None => break,
            Some((SectionKind::Certificate, der)) if certificate.is_none() => {
                certificate = Some(CertificateDer::from(der));
            }
            Some((SectionKind::Certificate, _)) => {
                return Err(invalid(
                    "holds more than one certificate; give only the server's own",
                ));
            }
            // A pasted private key must be reported, never quietly skipped.
            Some(_) => {
                return Err(invalid(
                    "holds something other than a certificate; give only the server's own \
                     certificate, and no private key",
                ));
            }
        }
    }
    let certificate = certificate.ok_or_else(|| invalid("holds no certificate"))?;
    rustls::server::ParsedCertificate::try_from(&certificate)
        .map_err(|_| invalid("does not hold a readable X.509 certificate"))?;
    Ok(certificate)
}

/// Accepts exactly one certificate, by its bytes, from a server proving it holds the key.
///
/// The bytes are the whole trust decision (as DANE-EE, RFC 7671 §5.1), so issuer, name,
/// CA flag and expiry are deliberately not checked: webpki refuses the certificates this
/// exists for, such as a `CA:TRUE` certificate used by the server itself. It is not a way
/// to trust a CA without its name check: a CA's certificate only matches a server
/// presenting that very certificate, never one it issued (ADR-017).
#[cfg(feature = "tls")]
#[derive(Debug)]
struct PinnedCertificate {
    certificate: CertificateDer<'static>,
    provider: Arc<CryptoProvider>,
}

#[cfg(feature = "tls")]
impl ServerCertVerifier for PinnedCertificate {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        if end_entity.as_ref() == self.certificate.as_ref() {
            Ok(ServerCertVerified::assertion())
        } else {
            Err(rustls::Error::InvalidCertificate(
                rustls::CertificateError::Other(rustls::OtherError(Arc::new(NotThePinned))),
            ))
        }
    }

    // A certificate is public, so its bytes alone prove nothing: these two signatures are
    // what show the server holds its key.
    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        self.provider
            .signature_verification_algorithms
            .supported_schemes()
    }
}

/// rustls shows a custom certificate error through `Debug`, hence the hand-written one.
#[cfg(feature = "tls")]
struct NotThePinned;

#[cfg(feature = "tls")]
impl fmt::Display for NotThePinned {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("the server's certificate is not the pinned certificate")
    }
}

#[cfg(feature = "tls")]
impl fmt::Debug for NotThePinned {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

#[cfg(feature = "tls")]
impl std::error::Error for NotThePinned {}
