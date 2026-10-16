#![forbid(unsafe_code)]

use workbench::cache_families;
use workbench::run;

const PROG: &str = "prune-cache-families";

fn main() {
    run(PROG, cache_families::prune, |pruned| {
        for line in cache_families::outcome_lines(&pruned.deleted, &pruned.gone) {
            println!("{line}");
        }
    })
}
