use std::{env, time::Duration};

use ffi::{Chain, Performance, UFVKImportParams, WalletEngine, WalletEvent, WalletListener};

struct PrintListener;
impl WalletListener for PrintListener {
    fn on_event(&self, event: WalletEvent) {
        println!("[event] {event:?}");
    }
}

fn require_env(name: &str) -> String {
    env::var(name).unwrap_or_else(|_| {
        eprintln!("Missing required env var: {name}");
        eprintln!("Example:");
        eprintln!("  {name}=...");
        std::process::exit(2);
    })
}

pub fn main() {
    let ufvk = require_env("ZINGO_UFVK");
    let indexer_uri = require_env("ZINGO_INDEXER_URI");

    let birthday = 1;

    let chain = Chain::Regtest;

    let perf = Performance::High;

    let minconf = 1;

    let engine = WalletEngine::new().expect("engine new");
    engine
        .set_listener(Box::new(PrintListener))
        .expect("set listener");

    engine
        .init_from_ufvk(UFVKImportParams {
            ufvk: ufvk.to_string(),
            birthday,
            indexer_uri,
            chain,
            perf,
            minconf,
        })
        .unwrap();

    engine.start_sync().unwrap();
    std::thread::sleep(Duration::from_secs(20));
}
