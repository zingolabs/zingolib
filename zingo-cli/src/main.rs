#![forbid(unsafe_code)]

use tracing_subscriber::EnvFilter;
pub fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();
    // install default crypto provider (ring)
    if let Err(e) = rustls::crypto::ring::default_provider().install_default() {
        eprintln!("Error installing crypto provider: {e:?}");
    }
    zingo_cli::run_cli();
}
