use super::*;

#[test]
fn the_bundle_loads_and_builds_a_verifier() {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let verifier = Verifier::new(provider).expect("webpki bundle must load");
    assert!(!verifier.supported_verify_schemes().is_empty());
}
