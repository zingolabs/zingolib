#![forbid(unsafe_code)]

use std::fmt;
use std::sync::Arc;
use std::task::{Context, Poll};

use hyper_rustls::HttpsConnector;
use hyper_util::client::legacy::connect::HttpConnector;
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::{CryptoProvider, WebPkiSupportedAlgorithms};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{DigitallySignedStruct, SignatureScheme};
use sha2::{Digest, Sha256};
use x509_parser::error::X509Error;
use x509_parser::extensions::GeneralName;

const SPKI_SHA256_LEN: usize = 32;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SpkiSha256([u8; SPKI_SHA256_LEN]);

impl SpkiSha256 {
    fn of_spki(spki_der: &[u8]) -> Self {
        Self(Sha256::digest(spki_der).into())
    }
}

impl fmt::Display for SpkiSha256 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.iter().try_for_each(|byte| write!(f, "{byte:02x}"))
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct PinnedHost {
    pub host: &'static str,
    pub pins: &'static [SpkiSha256],
}

pub const PINNED_HOSTS: &[PinnedHost] = &[PinnedHost {
    host: "mainnet.zainod.zingolabs.dev",
    pins: &[
        SpkiSha256([
            0x3e, 0x56, 0x89, 0x11, 0x75, 0xd0, 0xe4, 0x99, 0xbe, 0xdc, 0xfa, 0xb5, 0x3e, 0xbd,
            0xdb, 0x34, 0x56, 0x00, 0x9a, 0x5c, 0x58, 0xce, 0xee, 0x95, 0xc4, 0x71, 0xf8, 0x11,
            0x7e, 0x0d, 0xa4, 0xff,
        ]),
        SpkiSha256([
            0x61, 0x2a, 0xf4, 0xc4, 0xb7, 0x2b, 0x8f, 0xab, 0xc1, 0x0e, 0xaa, 0xb0, 0x2d, 0x58,
            0x54, 0x85, 0xb7, 0x61, 0x66, 0x0f, 0xde, 0xdb, 0x9f, 0xb3, 0x0d, 0x69, 0xb7, 0xa2,
            0x3b, 0xed, 0x4a, 0x53,
        ]),
        SpkiSha256([
            0x09, 0xd1, 0x9e, 0xa8, 0x6a, 0x76, 0x44, 0x05, 0x67, 0xb1, 0xd1, 0x67, 0xf0, 0xbd,
            0x31, 0xd8, 0xe0, 0x91, 0x27, 0x15, 0x8e, 0xbb, 0xd9, 0xb2, 0xf5, 0xba, 0x12, 0x57,
            0xe0, 0x9f, 0xf4, 0x92,
        ]),
        SpkiSha256([
            0x8f, 0x1f, 0x0d, 0x96, 0xf7, 0x3e, 0x50, 0x75, 0x7e, 0xc5, 0xd2, 0x95, 0xc5, 0x73,
            0x31, 0x1c, 0xcb, 0x42, 0x95, 0x68, 0xe4, 0x8c, 0x87, 0x02, 0x42, 0xcc, 0xe9, 0x54,
            0x22, 0x0b, 0x1f, 0xdd,
        ]),
    ],
}];

pub fn pinned_host(host: &str) -> Option<&'static PinnedHost> {
    PINNED_HOSTS
        .iter()
        .find(|pinned| pinned.host.eq_ignore_ascii_case(host))
}

#[derive(Debug, thiserror::Error)]
pub enum KeyPinError {
    #[error("the leaf key {observed} matches no pin of the host")]
    NoMatchingKey { observed: SpkiSha256 },
    #[error("the certificate is not valid before unix time {not_before}")]
    NotYetValid { not_before: i64 },
    #[error("the certificate expired at unix time {not_after}")]
    Expired { not_after: i64 },
    #[error("the certificate does not cover the host name {host}")]
    HostNotCovered { host: String },
    #[error("the certificate does not parse")]
    Unparseable(#[from] X509Error),
}

#[derive(Debug)]
pub struct Verifier {
    pins: &'static [SpkiSha256],
    algorithms: WebPkiSupportedAlgorithms,
}

impl Verifier {
    pub fn new(pins: &'static [SpkiSha256], provider: &CryptoProvider) -> Self {
        Self {
            pins,
            algorithms: provider.signature_verification_algorithms,
        }
    }

    fn verify_leaf(
        &self,
        leaf: &CertificateDer<'_>,
        server_name: &ServerName<'_>,
        now: UnixTime,
    ) -> Result<(), KeyPinError> {
        let (_, cert) =
            x509_parser::parse_x509_certificate(leaf.as_ref()).map_err(X509Error::from)?;

        let observed = SpkiSha256::of_spki(cert.public_key().raw);
        if !self.pins.contains(&observed) {
            return Err(KeyPinError::NoMatchingKey { observed });
        }

        let now = i128::from(now.as_secs());
        let not_before = cert.validity().not_before.timestamp();
        if now < i128::from(not_before) {
            return Err(KeyPinError::NotYetValid { not_before });
        }
        let not_after = cert.validity().not_after.timestamp();
        if now > i128::from(not_after) {
            return Err(KeyPinError::Expired { not_after });
        }

        let host = server_name.to_str();
        let covered = match server_name {
            ServerName::DnsName(dns) => cert
                .subject_alternative_name()?
                .into_iter()
                .flat_map(|san| san.value.general_names.iter())
                .any(|name| match name {
                    GeneralName::DNSName(pattern) => dns_name_covers(pattern, dns.as_ref()),
                    _ => false,
                }),
            _ => false,
        };
        if covered {
            Ok(())
        } else {
            Err(KeyPinError::HostNotCovered {
                host: host.into_owned(),
            })
        }
    }
}

fn dns_name_covers(pattern: &str, host: &str) -> bool {
    match pattern.strip_prefix("*.") {
        Some(parent) => host
            .split_once('.')
            .is_some_and(|(label, rest)| !label.is_empty() && rest.eq_ignore_ascii_case(parent)),
        None => pattern.eq_ignore_ascii_case(host),
    }
}

fn into_tls_error(err: KeyPinError) -> rustls::Error {
    rustls::Error::Other(rustls::OtherError(Arc::new(err)))
}

macro_rules! signature_checks_under_the_leaf_key {
    ($($check:ident),+) => {$(
        fn $check(
            &self,
            message: &[u8],
            cert: &CertificateDer<'_>,
            dss: &DigitallySignedStruct,
        ) -> Result<HandshakeSignatureValid, rustls::Error> {
            rustls::crypto::$check(message, cert, dss, &self.algorithms)
        }
    )+};
}

impl ServerCertVerifier for Verifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        self.verify_leaf(end_entity, server_name, now)
            .map(|()| ServerCertVerified::assertion())
            .map_err(into_tls_error)
    }

    signature_checks_under_the_leaf_key!(verify_tls12_signature, verify_tls13_signature);

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.algorithms.supported_schemes()
    }
}

pub(crate) fn client_config(
    pinned: &PinnedHost,
    provider: Arc<CryptoProvider>,
) -> Result<rustls::ClientConfig, rustls::Error> {
    let verifier = Verifier::new(pinned.pins, &provider);
    Ok(rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()?
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(verifier))
        .with_no_client_auth())
}

#[derive(Clone)]
pub(crate) struct Connector {
    https: HttpsConnector<HttpConnector>,
    destination: http::Uri,
}

impl Connector {
    pub(crate) fn new(
        config: rustls::ClientConfig,
        tcp: HttpConnector,
        destination: http::Uri,
    ) -> Self {
        Self {
            https: hyper_rustls::HttpsConnectorBuilder::new()
                .with_tls_config(config)
                .https_only()
                .enable_http2()
                .wrap_connector(tcp),
            destination,
        }
    }
}

impl tower::Service<http::Uri> for Connector {
    type Response = <HttpsConnector<HttpConnector> as tower::Service<http::Uri>>::Response;
    type Error = <HttpsConnector<HttpConnector> as tower::Service<http::Uri>>::Error;
    type Future = <HttpsConnector<HttpConnector> as tower::Service<http::Uri>>::Future;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.https.poll_ready(cx)
    }

    fn call(&mut self, _endpoint: http::Uri) -> Self::Future {
        self.https.call(self.destination.clone())
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    use crate::default_indexer::default_indexer;
    use crate::indexers::IndexerChain;

    pub(crate) const LOCALHOST_PINS: &[SpkiSha256] = &[SpkiSha256([
        0x90, 0x7a, 0x89, 0xb5, 0x6a, 0x8d, 0x7e, 0xb7, 0x5a, 0x87, 0x17, 0xbe, 0x02, 0x92, 0xb8,
        0xa8, 0x2a, 0x9d, 0xcd, 0x28, 0x74, 0xd5, 0x1a, 0x53, 0xca, 0x3e, 0xf2, 0x4f, 0x01, 0xed,
        0xb1, 0xed,
    ])];
    pub(crate) const WRONG_PINS: &[SpkiSha256] = &[SpkiSha256([0; SPKI_SHA256_LEN])];
    const PINS_WITH_A_BACKUP: usize = 2;
    const LOCALHOST_NOT_BEFORE: u64 = 1_771_181_385;
    const SECONDS_PER_DAY: u64 = 86_400;

    fn localhost_verifier(pins: &'static [SpkiSha256]) -> Verifier {
        Verifier::new(pins, &rustls::crypto::aws_lc_rs::default_provider())
    }

    fn localhost_leaf() -> CertificateDer<'static> {
        let mut pem = crate::test_tls::LOCALHOST_CERT_PEM;
        rustls_pemfile::certs(&mut pem)
            .next()
            .expect("test-data/localhost.pem holds one certificate")
            .expect("test-data/localhost.pem is well-formed PEM")
    }

    fn localhost_name() -> ServerName<'static> {
        ServerName::try_from("localhost").expect("localhost is a valid DNS name")
    }

    fn within_validity() -> UnixTime {
        UnixTime::since_unix_epoch(std::time::Duration::from_secs(
            LOCALHOST_NOT_BEFORE + SECONDS_PER_DAY,
        ))
    }

    fn key_pin_error(err: &rustls::Error) -> &KeyPinError {
        let rustls::Error::Other(rustls::OtherError(source)) = err else {
            panic!("expected rustls::Error::Other, got {err}");
        };
        source
            .downcast_ref::<KeyPinError>()
            .expect("the Verifier maps every certificate failure from a KeyPinError")
    }

    #[test]
    fn the_correct_pin_accepts() {
        let verdict = localhost_verifier(LOCALHOST_PINS).verify_server_cert(
            &localhost_leaf(),
            &[],
            &localhost_name(),
            &[],
            within_validity(),
        );
        assert!(verdict.is_ok(), "{verdict:?}");
    }

    #[test]
    fn a_wrong_pin_has_no_matching_key() {
        let err = localhost_verifier(WRONG_PINS)
            .verify_server_cert(
                &localhost_leaf(),
                &[],
                &localhost_name(),
                &[],
                within_validity(),
            )
            .expect_err("no pin of WRONG_PINS is the localhost key");
        assert!(
            matches!(
                key_pin_error(&err),
                KeyPinError::NoMatchingKey { observed } if *observed == LOCALHOST_PINS[0]
            ),
            "the observed key must be the localhost key"
        );
    }

    #[test]
    fn a_wrong_server_name_is_not_covered() {
        let other = ServerName::try_from("example.com").expect("example.com is a valid DNS name");
        let err = localhost_verifier(LOCALHOST_PINS)
            .verify_server_cert(&localhost_leaf(), &[], &other, &[], within_validity())
            .expect_err("the localhost certificate names localhost alone");
        assert!(matches!(
            key_pin_error(&err),
            KeyPinError::HostNotCovered { host } if host == "example.com"
        ));
    }

    #[test]
    fn a_time_before_issue_is_not_yet_valid() {
        let before = UnixTime::since_unix_epoch(std::time::Duration::from_secs(
            LOCALHOST_NOT_BEFORE - SECONDS_PER_DAY,
        ));
        let err = localhost_verifier(LOCALHOST_PINS)
            .verify_server_cert(&localhost_leaf(), &[], &localhost_name(), &[], before)
            .expect_err("the localhost certificate starts on 2026-02-15");
        assert!(matches!(
            key_pin_error(&err),
            KeyPinError::NotYetValid { not_before }
                if u64::try_from(*not_before) == Ok(LOCALHOST_NOT_BEFORE)
        ));
    }

    #[test]
    fn a_wildcard_covers_one_label() {
        assert!(dns_name_covers("*.zingolabs.dev", "mainnet.zingolabs.dev"));
        assert!(!dns_name_covers(
            "*.zingolabs.dev",
            "a.mainnet.zingolabs.dev"
        ));
        assert!(!dns_name_covers("*.zingolabs.dev", "zingolabs.dev"));
    }

    #[test]
    fn the_default_indexer_is_a_pinned_host() {
        let uri = default_indexer(IndexerChain::Main).expect("mainnet has a Default Indexer");
        let host = uri.host().expect("the Default Indexer URI names a host");
        assert!(pinned_host(host).is_some(), "{host} is not in PINNED_HOSTS");
    }

    #[test]
    fn every_pinned_host_carries_a_backup_pin() {
        for pinned in PINNED_HOSTS {
            assert!(
                pinned.pins.len() >= PINS_WITH_A_BACKUP,
                "{} carries {} pin(s); the backup key is owed",
                pinned.host,
                pinned.pins.len()
            );
        }
    }
}
