#![forbid(unsafe_code)]

use workbench::{dupes_gate, repo_root, run};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    run(
        dupes_gate::BINARY,
        || dupes_gate::dispatch(&repo_root()?, &args),
        |()| (),
    );
}
