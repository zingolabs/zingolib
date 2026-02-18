use std::{env, time::Duration};

use bip0039::{English, Mnemonic};
use ffi::{
    Chain, Performance, RestoreParams, SeedPhrase, WalletEngine, WalletEvent, WalletListener,
};

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

fn preflight(seed_words: &str, indexer_uri: &str) {
    if let Err(e) = Mnemonic::<English>::from_phrase(seed_words.to_string()) {
        eprintln!("Invalid ZINGO_SEED mnemonic: {e}");
        std::process::exit(2);
    }

    match indexer_uri.parse::<http::Uri>() {
        Ok(uri) => {
            let scheme_ok = uri.scheme_str() == Some("http") || uri.scheme_str() == Some("https");
            let has_authority = uri.authority().is_some();
            if !scheme_ok || !has_authority {
                eprintln!(
                    "Invalid ZINGO_INDEXER_URI='{indexer_uri}'. Expected http(s)://host:port"
                );
                std::process::exit(2);
            }
        }
        Err(e) => {
            eprintln!("Invalid ZINGO_INDEXER_URI='{indexer_uri}': {e}");
            std::process::exit(2);
        }
    }
}

pub fn main() {
    let seed_words = require_env("ZINGO_SEED");
    let indexer_uri = require_env("ZINGO_INDEXER_URI");

    let birthday = 1;

    let chain = Chain::Regtest;

    let perf = Performance::High;

    let minconf = 1;

    preflight(&seed_words, &indexer_uri);

    let engine = WalletEngine::new().expect("engine new");
    engine
        .set_listener(Box::new(PrintListener))
        .expect("set listener");

    engine
        .init_from_seed(RestoreParams {
            seed_phrase: SeedPhrase { words: seed_words },
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
