#![forbid(unsafe_code)]
pub fn main() {
    tracing_subscriber::fmt().init();
    // install default crypto provider (ring)
    if let Err(e) = rustls::crypto::ring::default_provider().install_default() {
        eprintln!("Error installing crypto provider: {:?}", e)
    };
    zingo_cli::run_cli();
}
