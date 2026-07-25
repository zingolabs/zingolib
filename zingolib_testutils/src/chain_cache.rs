//! The chain-cache mechanism of ADR 0003 (`docs/adr/0003-test-owned-chain-caches.md`).
//!
//! Each test owns at most one cache under `chain_caches/<binary>/<test>/`,
//! holding a `blocks.hex` replay record of the blocks its setup generated
//! (one hex-serialized block per line, heights 1..=tip) plus a
//! `manifest.json` recording the chain-determining inputs. A cache is
//! built inline by the test that finds none (or when [`REGENERATE_ENV`]
//! is set): the run generates its chain live as always and exports the
//! blocks over the Validator's JSON-RPC at the snapshot point. A
//! cache-hit run launches a fresh network and resubmits the recorded
//! blocks through `submitblock`, so the Validator revalidates every
//! cached block and the Indexer ingests them through the same flow live
//! mining uses. Superseded caches are discarded, never moved aside.
//!
//! Blocks are the cache medium because Validator state directories are
//! not: zebra keeps everything within its finalization depth
//! (`zebra_state::MAX_BLOCK_REORG_HEIGHT`, mirrored in this repository
//! by `pepper_sync::sync::MAX_REORG_ALLOWANCE`) of the tip in its
//! in-memory non-finalized state, so a copied data dir of a short
//! regtest chain contains essentially genesis (see the ADR's history).

use std::path::{Path, PathBuf};

use zcash_local_net::validator::Validator;
use zcash_protocol::PoolType;
use zingo_common_components::protocol::ActivationHeights;

use crate::setup_metrics::{self, MeteredNet};
use crate::validator_rpc;

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
    /// Replay this blocks file verbatim, with no manifest check and no
    /// build-on-miss. For hand-managed caches like the ignored
    /// `store_all_checkpoints` pair.
    LoadRaw(PathBuf),
}

/// The resolved fate of one scenario launch.
pub(crate) enum Disposition {
    /// Generate live; no cache involved.
    Live,
    /// Launch fresh, then replay this blocks file.
    Replay(PathBuf),
    /// No usable cache: generate live, then export the chain into
    /// `CacheDir` at the snapshot point.
    Export(CacheDir),
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

    /// The replay record: one hex-serialized block per line, height
    /// order, heights 1..=tip.
    fn blocks(&self) -> PathBuf {
        self.root.join("blocks.hex")
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

/// Which setup stage a cache holds: the bare launch chain, the funded
/// chain that includes the faucet's shielded-offload transactions, or
/// the recipient-funded chain that additionally embeds
/// `faucet_funded_recipient`'s funding sends (whose returned txids
/// live in the cache's `outputs.json`). Recorded in the manifest so a
/// test that switches scenario constructors — or changes its funding
/// amounts — between runs cannot load a wrong chain past an
/// otherwise-matching manifest.
#[derive(Debug)]
pub(crate) enum CachedStage {
    Bare,
    Funded,
    // The fields are consumed through the derived Debug impl, which
    // serializes the stage — amounts included — into the manifest;
    // dead-code analysis ignores derive-only reads by design.
    #[allow(dead_code)]
    RecipientFunded {
        orchard_funds: Option<u64>,
        sapling_funds: Option<u64>,
        transparent_funds: Option<u64>,
    },
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
            // Schema 3 (2026-07-17): the immediate migration rewrite changed cached-chain
            // SEMANTICS (self-send → orchard drain) without changing any
            // manifest key, and pre-drain caches replayed as green-shaped
            // chains against post-drain expectations for a full day of
            // misdiagnosis. Any change to what setup writes into the chain
            // must bump `setup_semantics` (or the schema), so stale caches
            // self-discard instead of impersonating current behavior.
            "schema": 3,
            "setup_semantics": "offload+drain-v1",
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
        ChainCachePolicy::LoadRaw(blocks_file) => Disposition::Replay(blocks_file),
        ChainCachePolicy::PerTest => {
            let dir = CacheDir::for_current_test();
            if regenerate_requested() {
                eprintln!(
                    "{REGENERATE_ENV} set: discarding and rebuilding {}",
                    dir.root.display()
                );
                dir.discard();
                return Disposition::Export(dir);
            }
            if dir.blocks().exists() {
                if manifest.matches_stored(&dir.manifest()) {
                    return Disposition::Replay(dir.blocks());
                }
                eprintln!(
                    "chain cache {} is stale (manifest mismatch): rebuilding",
                    dir.root.display()
                );
            }
            dir.discard();
            Disposition::Export(dir)
        }
    }
}

/// Read the chain out of the running Validator, block by block, as the
/// replay record. Heights 1..=tip; genesis is the Validator's own. All
/// traffic crosses the monitored hop, so exports appear in the
/// observability record.
async fn read_chain(local_net: &MeteredNet) -> String {
    // The accessor returns the observing front, so exports appear in
    // the zebrad front's record.
    let rpc_port = local_net.validator().rpc_listen_port();
    let tip = local_net.validator().get_chain_height().await;
    let mut blocks = String::new();
    for height in 1..=tip {
        blocks.push_str(&validator_rpc::get_block_hex(rpc_port, height).await);
        blocks.push('\n');
    }
    blocks
}

/// Export the live-generated chain into this test's cache. Called at
/// the scenario's snapshot point; the net stays up and the building run
/// simply continues. The record is assembled in a `.building` sibling
/// and renamed into place so a crashed build never leaves a half-cache
/// where a later run would load it.
pub(crate) async fn export(local_net: &MeteredNet, dir: &CacheDir, manifest: &CacheManifest) {
    export_inner(local_net, dir, manifest, None).await;
}

/// [`export`], additionally recording the scenario's returned outputs
/// (e.g. `faucet_funded_recipient`'s funding txids) in the cache's
/// `outputs.json`. The outputs ride the same atomic rename as the
/// blocks, so a cache with an outputs-bearing stage can never exist
/// without them.
pub(crate) async fn export_with_outputs(
    local_net: &MeteredNet,
    dir: &CacheDir,
    manifest: &CacheManifest,
    outputs: serde_json::Value,
) {
    export_inner(local_net, dir, manifest, Some(outputs)).await;
}

async fn export_inner(
    local_net: &MeteredNet,
    dir: &CacheDir,
    manifest: &CacheManifest,
    outputs: Option<serde_json::Value>,
) {
    let building = dir.root.with_extension("building");
    if building.exists() {
        std::fs::remove_dir_all(&building).expect("stale .building dir must be removable");
    }
    std::fs::create_dir_all(&building).expect("cache parent dirs must be creatable");

    std::fs::write(building.join("blocks.hex"), read_chain(local_net).await)
        .expect("blocks record must be writable");
    std::fs::write(
        building.join("manifest.json"),
        serde_json::to_string_pretty(&manifest.0).expect("manifest serializes"),
    )
    .expect("manifest must be writable");
    if let Some(outputs) = outputs {
        std::fs::write(
            building.join("outputs.json"),
            serde_json::to_string_pretty(&outputs).expect("outputs serialize"),
        )
        .expect("outputs record must be writable");
    }

    std::fs::rename(&building, &dir.root).expect("completed cache must rename into place");
}

/// Read the recorded scenario outputs that live beside a blocks
/// record. `None` when no `outputs.json` exists — legitimate only for
/// hand-managed [`ChainCachePolicy::LoadRaw`] caches; a `PerTest`
/// cache whose stage records outputs writes them atomically with the
/// blocks, so absence there means a corrupt cache (discard it).
pub(crate) fn load_outputs(blocks_file: &Path) -> Option<serde_json::Value> {
    let outputs_path = blocks_file.with_file_name("outputs.json");
    let text = std::fs::read_to_string(outputs_path).ok()?;
    serde_json::from_str(&text).ok()
}

/// Export a bare replay record to an explicit path, for hand-managed
/// caches consumed via [`ChainCachePolicy::LoadRaw`].
pub async fn export_raw(local_net: &MeteredNet, path: &Path) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("blocks record parent must be creatable");
    }
    std::fs::write(path, read_chain(local_net).await).expect("blocks record must be writable");
}

/// Resubmit a cached chain to a freshly launched Validator, returning
/// the replayed tip height. Every block passes back through
/// `submitblock` validation, and the call returns once the Validator
/// reports the replayed tip, so callers continue exactly as if the
/// chain had just been mined.
///
/// A freshly launched regtest Validator is at height 1, not 0:
/// `Zebrad::launch` mines one block to prove the mining service (the
/// launch block). Replay therefore submits the cached chain as a
/// competitor: the cached block 1 either IS the launch block
/// (transparent chains are byte-deterministic — `submitblock` says
/// "duplicate", which is acceptance) or forks around it, and the cached
/// branch wins the reorg because every cache is at least three blocks
/// long. The height-≤1 preflight still catches genuinely foreign
/// chain-mutating traffic, with the observatory timeline attached.
pub(crate) async fn replay(local_net: &MeteredNet, blocks_file: &Path) -> u32 {
    let blocks = std::fs::read_to_string(blocks_file)
        .unwrap_or_else(|e| panic!("unreadable blocks record {}: {e}", blocks_file.display()));
    // The accessor returns the observing front, so replay traffic
    // appears in the zebrad front's record.
    let rpc_port = local_net.validator().rpc_listen_port();

    let (height, hash) = validator_rpc::try_get_chain_info(rpc_port)
        .await
        .expect("freshly launched validator must answer getblockchaininfo");
    assert!(
        height <= 1,
        "freshly launched validator at height {height} (tip {hash}); at most the launch \
         block is expected before this test submits anything — foreign chain-mutating \
         traffic!\nzebrad timeline:\n{}",
        local_net.zebrad_watch().render(),
    );

    let mut tip = 0;
    for block_hex in blocks.lines().filter(|line| !line.is_empty()) {
        validator_rpc::submit_block(rpc_port, block_hex).await;
        tip += 1;
    }
    assert!(
        tip > 1,
        "blocks record {} holds {tip} block(s); the cached branch must outrank the \
         launch block to win the reorg",
        blocks_file.display()
    );
    local_net.validator().poll_chain_height(tip).await;
    tip
}
