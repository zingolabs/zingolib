//! The chain-cache mechanism of ADR 0003 (`docs/adr/0003-test-owned-chain-caches.md`).
//!
//! Each test owns at most one cache under `chain_caches/<binary>/<test>/`,
//! holding a `chain/` clone of the Validator's data directory plus a
//! `manifest.json` recording the chain-determining inputs. A cache is
//! built inline by the test that finds none (or when
//! [`REGENERATE_ENV`] is set), and the building run relaunches from its
//! own snapshot so every run — including the builder's — reaches its
//! assertions through the load path. Superseded caches are discarded,
//! never moved aside.

use std::path::{Path, PathBuf};

use zcash_local_net::validator::Validator;
use zcash_protocol::PoolType;
use zingo_common_components::protocol::ActivationHeights;

use crate::setup_metrics::{self, MeteredNet};

/// Environment variable that forces cache rebuilds. Scope it to specific
/// tests with ordinary nextest selection; any non-empty value other than
/// `0` counts.
pub const REGENERATE_ENV: &str = "ZINGO_REGENERATE_CHAIN_CACHE";

/// How a scenario constructor treats the chain cache. `PerTest` is the
/// default regime of ADR 0003; `Disabled` is the explicit opt-out for
/// tests that must generate their chain live.
pub enum ChainCachePolicy {
    /// Use (and on miss, build) this test's own cache.
    PerTest,
    /// Always generate the chain live; never read or write a cache.
    Disabled,
    /// Load a raw `cache_chain` output directory verbatim, with no
    /// manifest check and no build-on-miss. For hand-managed caches
    /// like the ignored `store_all_checkpoints` pair.
    LoadRaw(PathBuf),
}

/// The resolved fate of one scenario launch.
pub(crate) enum Disposition {
    /// Generate live; no cache involved.
    Live,
    /// Launch from this chain directory (a data-dir clone).
    Load(PathBuf),
    /// No usable cache: generate live, snapshot into `CacheDir`, then
    /// relaunch from it.
    Build(CacheDir),
}

/// This test's cache directory: `chain_caches/<binary>/<test>/`.
pub(crate) struct CacheDir {
    root: PathBuf,
}

impl CacheDir {
    fn for_current_test() -> Self {
        let test = setup_metrics::current_test_name().replace("::", "__");
        CacheDir {
            root: setup_metrics::chain_caches_root()
                .join(setup_metrics::current_binary_name())
                .join(test),
        }
    }

    /// The data-dir clone the Validator launches from.
    pub(crate) fn chain(&self) -> PathBuf {
        self.root.join("chain")
    }

    fn manifest(&self) -> PathBuf {
        self.root.join("manifest.json")
    }

    /// Remove the cache entirely (discard-on-regenerate, ADR 0003).
    /// Missing directories are fine; anything else is a real error.
    fn discard(&self) {
        if let Err(e) = std::fs::remove_dir_all(&self.root)
            && e.kind() != std::io::ErrorKind::NotFound
        {
            panic!("failed to discard chain cache {}: {e}", self.root.display());
        }
    }
}

/// Which setup stage a cache holds: the bare launch chain, or the
/// funded chain that includes the faucet's shielded-offload
/// transactions. Recorded in the manifest so a test that switches
/// scenario constructors between runs cannot load a wrong-stage chain
/// past an otherwise-matching manifest.
#[derive(Debug)]
pub(crate) enum CachedStage {
    Bare,
    Funded,
}

/// The chain-determining inputs a cache records at build time and every
/// later run compares at load time. A mismatch is a miss: the cache is
/// discarded and rebuilt, so consensus-parameter drift (e.g. activation
/// heights moving during the ironwood migration) cannot silently serve
/// a chain mined under old rules.
pub(crate) struct CacheManifest(serde_json::Value);

impl CacheManifest {
    pub(crate) fn describe(
        mine_to_pool: PoolType,
        configured_activation_heights: &ActivationHeights,
        stage: CachedStage,
    ) -> Self {
        CacheManifest(serde_json::json!({
            "schema": 1,
            "validator": std::any::type_name::<crate::scenarios::network_combo::DefaultValidator>(),
            "indexer": std::any::type_name::<crate::scenarios::network_combo::DefaultIndexer>(),
            "stage": format!("{stage:?}"),
            "miner_pool": format!("{mine_to_pool:?}"),
            "activation_heights": format!("{configured_activation_heights:?}"),
        }))
    }

    fn matches_stored(&self, manifest_path: &Path) -> bool {
        let Ok(stored) = std::fs::read_to_string(manifest_path) else {
            return false;
        };
        let Ok(stored) = serde_json::from_str::<serde_json::Value>(&stored) else {
            return false;
        };
        stored == self.0
    }
}

fn regenerate_requested() -> bool {
    std::env::var_os(REGENERATE_ENV).is_some_and(|v| !v.is_empty() && v != *"0")
}

/// Decide this launch's fate from the policy and the on-disk state.
pub(crate) fn resolve(policy: ChainCachePolicy, manifest: &CacheManifest) -> Disposition {
    match policy {
        ChainCachePolicy::Disabled => Disposition::Live,
        ChainCachePolicy::LoadRaw(chain_dir) => Disposition::Load(chain_dir),
        ChainCachePolicy::PerTest => {
            let dir = CacheDir::for_current_test();
            if regenerate_requested() {
                eprintln!(
                    "{REGENERATE_ENV} set: discarding and rebuilding {}",
                    dir.root.display()
                );
                dir.discard();
                return Disposition::Build(dir);
            }
            if dir.chain().exists() {
                if manifest.matches_stored(&dir.manifest()) {
                    return Disposition::Load(dir.chain());
                }
                eprintln!(
                    "chain cache {} is stale (manifest mismatch): rebuilding",
                    dir.root.display()
                );
            }
            dir.discard();
            Disposition::Build(dir)
        }
    }
}

/// Snapshot a live-generated chain into the cache. Consumes the net —
/// `cache_chain` stops the Validator, so the caller must relaunch from
/// the snapshot (the uniform load path) to continue. The snapshot is
/// assembled in a `.building` sibling and renamed into place so a
/// crashed build never leaves a half-cache where a later run would
/// load it.
pub(crate) async fn snapshot(mut net: MeteredNet, dir: &CacheDir, manifest: &CacheManifest) {
    // This net is build scaffolding: the test's measured net is the one
    // relaunched from the snapshot, so this one must not write a
    // metrics row.
    net.disarm();

    let building = dir.root.with_extension("building");
    if building.exists() {
        std::fs::remove_dir_all(&building).expect("stale .building dir must be removable");
    }
    std::fs::create_dir_all(&building).expect("cache parent dirs must be creatable");

    let output = net
        .validator_mut()
        .cache_chain(building.join("chain"))
        .await;
    assert!(
        output.status.success(),
        "cache_chain copy failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    std::fs::write(
        building.join("manifest.json"),
        serde_json::to_string_pretty(&manifest.0).expect("manifest serializes"),
    )
    .expect("manifest must be writable");

    std::fs::rename(&building, &dir.root).expect("completed cache must rename into place");
}
