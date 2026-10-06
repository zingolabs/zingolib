#![forbid(unsafe_code)]

use workbench::{binding_changelog, repo_root, run};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    run(
        binding_changelog::BINARY,
        || binding_changelog::dispatch(&repo_root()?, &args),
        |()| (),
    );
}
