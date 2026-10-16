#![forbid(unsafe_code)]

use workbench::cache_families;
use workbench::run;

const PROG: &str = "prune-cache-families";

fn main() {
    run(PROG, cache_families::prune, |deleted| {
        for entry in deleted {
            println!("deleted {} {}", entry.id, entry.key);
        }
    })
}
