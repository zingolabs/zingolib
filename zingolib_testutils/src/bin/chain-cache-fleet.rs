#![forbid(unsafe_code)]
//! Prints the chain-cache fleet's freshness as `fresh=N stale=M`.
//!
//! `fresh` counts stored per-test caches whose manifest matches the
//! current fleet key (schema, setup semantics, validator, indexer);
//! `stale` counts the rest, including manifest-less hand-managed caches.
//! The test harness (`fresh_chain_cache_count` in Makefile.toml's
//! base-script) reads `fresh` before launching nextest: a cold fleet —
//! fewer than five fresh caches — means a bulk (re)build is coming, and
//! the launch is capped at `--test-threads 4` so concurrent builds do
//! not contend past their per-test timeout ceilings.

fn main() {
    let (fresh, stale) = zingolib_testutils::chain_cache::fleet_status();
    println!("fresh={fresh} stale={stale}");
}
