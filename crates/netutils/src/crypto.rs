//! Process-level rustls `CryptoProvider` management.

/// Installs rustls's `aws-lc-rs` provider as the process-level default
/// if no provider is installed yet.
///
/// Call this before constructing anything that builds a rustls config.
/// rustls's own auto-selection is unreliable here: it works only when
/// exactly one of rustls's `ring` / `aws_lc_rs` features is enabled, and
/// dependency feature-unification can silently enable both, which panics
/// at runtime. Installing explicitly removes that dependence on the
/// feature graph.
///
/// aws-lc-rs is the workspace's sole provider (ring is excised): it is
/// the only rustls provider with post-quantum key exchange, and the
/// workspace enables rustls's `prefer-post-quantum` feature so the
/// X25519MLKEM768 hybrid group leads outbound (client-role) handshakes.
///
/// The process default is first-install-wins, so an embedder (e.g. a
/// mobile wallet) that has already installed a provider keeps it and
/// this crate handshakes through that provider instead.
pub fn ensure_default_crypto_provider() {
    if rustls::crypto::CryptoProvider::get_default().is_none() {
        // A racing concurrent install is the only error case; either
        // install serves.
        let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
    }
}
