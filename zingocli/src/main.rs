#![forbid(unsafe_code)]
pub fn main() {
    // install default crypto provider (ring)
    if let Err(e) = rustls::crypto::ring::default_provider().install_default() {
        eprintln!("Error installing crypto provider: {:?}", e)
    };
    tracing_subscriber::fmt().init();
    zingo_cli::run_cli();
}
