//! The webpki-roots stand-in for `rustls-platform-verifier` (ADR 0021,
//! zingolib#2531).
//!
//! The mobile mixnet shim cannot use the real crate: on Android it demands
//! JVM-side initialization with an app `Context` before any TLS handshake,
//! which requires hand-written JNI — `unsafe` the shim's ratified invariant
//! forbids — plus a Kotlin component in every consuming app. This crate is
//! substituted through the workspace `[patch.crates-io]` and verifies
//! against the compiled-in Mozilla root bundle (`webpki-root-certs`)
//! instead of the operating system's trust store, identically on every
//! platform.
//!
//! API parity is deliberately minimal: the only consumer in this workspace's
//! dependency tree is reqwest (`>=0.6, <0.8` requirement), which constructs
//! [`Verifier::new`] or [`Verifier::new_with_extra_roots`] and installs the
//! result as a rustls `ServerCertVerifier`. Nothing else from the upstream
//! crate's surface is provided, so a future consumer of a wider surface
//! fails loudly at compile time rather than silently diverging — that
//! compile failure is a tracked event of the ADR's update policy.
#![forbid(unsafe_code)]

use std::sync::Arc;

use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::client::WebPkiServerVerifier;
use rustls::crypto::CryptoProvider;
use rustls::{pki_types, DigitallySignedStruct, Error as TlsError, OtherError, SignatureScheme};

/// A TLS certificate verifier over the compiled-in Mozilla root bundle.
///
/// Name and constructor signatures mirror the upstream
/// `rustls_platform_verifier::Verifier` exactly, per the parity contract in
/// the crate docs.
#[derive(Debug)]
pub struct Verifier {
    inner: Arc<WebPkiServerVerifier>,
}

impl Verifier {
    /// Creates a verifier trusting the compiled-in Mozilla roots.
    pub fn new(crypto_provider: Arc<CryptoProvider>) -> Result<Self, TlsError> {
        Self::new_inner([], crypto_provider)
    }

    /// Creates a verifier trusting the compiled-in Mozilla roots plus the
    /// given extra root certificates.
    pub fn new_with_extra_roots(
        extra_roots: impl IntoIterator<Item = pki_types::CertificateDer<'static>>,
        crypto_provider: Arc<CryptoProvider>,
    ) -> Result<Self, TlsError> {
        Self::new_inner(extra_roots, crypto_provider)
    }

    fn new_inner(
        extra_roots: impl IntoIterator<Item = pki_types::CertificateDer<'static>>,
        crypto_provider: Arc<CryptoProvider>,
    ) -> Result<Self, TlsError> {
        let mut root_store = rustls::RootCertStore::empty();
        let (added, ignored) = root_store
            .add_parsable_certificates(webpki_root_certs::TLS_SERVER_ROOT_CERTS.iter().cloned());
        log::debug!("loaded {added} webpki root certificates ({ignored} ignored)");
        let (extra_added, extra_ignored) = root_store.add_parsable_certificates(extra_roots);
        if extra_added > 0 || extra_ignored > 0 {
            log::debug!("added {extra_added} extra roots ({extra_ignored} ignored)");
        }
        if root_store.is_empty() {
            return Err(TlsError::General(
                "no root certificates were loaded from the webpki bundle".to_owned(),
            ));
        }
        Ok(Self {
            inner: WebPkiServerVerifier::builder_with_provider(root_store.into(), crypto_provider)
                .build()
                .map_err(|e| TlsError::Other(OtherError(Arc::new(e))))?,
        })
    }
}

impl ServerCertVerifier for Verifier {
    fn verify_server_cert(
        &self,
        end_entity: &pki_types::CertificateDer<'_>,
        intermediates: &[pki_types::CertificateDer<'_>],
        server_name: &pki_types::ServerName<'_>,
        ocsp_response: &[u8],
        now: pki_types::UnixTime,
    ) -> Result<ServerCertVerified, TlsError> {
        self.inner
            .verify_server_cert(end_entity, intermediates, server_name, ocsp_response, now)
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &pki_types::CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, TlsError> {
        self.inner.verify_tls12_signature(message, cert, dss)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &pki_types::CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, TlsError> {
        self.inner.verify_tls13_signature(message, cert, dss)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.inner.supported_verify_schemes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_bundle_loads_and_builds_a_verifier() {
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let verifier = Verifier::new(provider).expect("webpki bundle must load");
        assert!(!verifier.supported_verify_schemes().is_empty());
    }
}
