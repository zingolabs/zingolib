use pepper_sync::test_support::block;
use zcash_protocol::ShieldedPool;
use zcash_protocol::consensus::{NetworkUpgrade, Parameters};
use zingo_netutils::lightwallet_protocol::{BlockId, BlockRange, ChainMetadata, CompactBlock};
use zingo_netutils::{GrpcIndexer, Indexer};
use zingolib::config::{ChainType, construct_indexer_uri};
use zingolib::lightclient::DEFAULT_REQUEST_TIMEOUT;

use super::TESTNET_INDEXER;

const BLOCKS_BEFORE_ACTIVATION: u64 = 2;
const BLOCKS_FROM_ACTIVATION: u64 = 200;
const POOLS: [ShieldedPool; 3] = [
    ShieldedPool::Sapling,
    ShieldedPool::Orchard,
    ShieldedPool::Ironwood,
];

fn commitment_tree_size(metadata: &ChainMetadata, pool: ShieldedPool) -> u32 {
    match pool {
        ShieldedPool::Sapling => metadata.sapling_commitment_tree_size,
        ShieldedPool::Orchard => metadata.orchard_commitment_tree_size,
        ShieldedPool::Ironwood => metadata.ironwood_commitment_tree_size,
    }
}

fn pool_mismatches(
    prev: &ChainMetadata,
    metadata: &ChainMetadata,
    block: &CompactBlock,
) -> Vec<String> {
    POOLS
        .into_iter()
        .filter_map(|pool| {
            let served = u64::from(block::shielded_output_count(block, pool));
            let metadata_delta = i64::from(commitment_tree_size(metadata, pool))
                - i64::from(commitment_tree_size(prev, pool));
            (metadata_delta != served as i64).then(|| {
                format!(
                    "height {}: {pool:?} metadata delta {metadata_delta} but {served} outputs \
                     served in vtx",
                    block.height
                )
            })
        })
        .collect()
}

#[tokio::test]
async fn served_outputs_match_chain_metadata_deltas() {
    zingolib::ensure_default_crypto_provider();

    let ironwood_activation = u64::from(u32::from(
        ChainType::Testnet
            .activation_height(NetworkUpgrade::Nu6_3)
            .expect("NU6.3 is active on testnet"),
    ));
    let start = ironwood_activation - BLOCKS_BEFORE_ACTIVATION;
    let end = ironwood_activation + BLOCKS_FROM_ACTIVATION;

    let uri = construct_indexer_uri(TESTNET_INDEXER.to_string())
        .expect("the hard-coded testnet indexer constant is a valid URI");
    let mut grpc_client = GrpcIndexer::new(uri)
        .await
        .expect("the testnet indexer accepts a connection");
    let mut stream = grpc_client
        .get_block_range(
            BlockRange {
                start: Some(BlockId {
                    height: start,
                    hash: vec![],
                }),
                end: Some(BlockId {
                    height: end,
                    hash: vec![],
                }),
                // Empty means the legacy default: the server must include all
                // shielded pools. This matches what pepper-sync sends.
                pool_types: vec![],
            },
            DEFAULT_REQUEST_TIMEOUT,
        )
        .await
        .expect("the testnet indexer streams the activation window");

    let mut mismatches = Vec::new();
    let mut prev: Option<ChainMetadata> = None;
    let mut ironwood_served_total = 0u64;
    let mut first_nonzero_ironwood_metadata: Option<u64> = None;
    let mut first_served_ironwood_action: Option<u64> = None;

    while let Some(block) = stream
        .message()
        .await
        .expect("the block stream delivers every block of the window")
    {
        let metadata = block.chain_metadata.unwrap_or_else(|| {
            panic!(
                "indexer serves no chain_metadata at height {}; pepper-sync \
                 cannot verify tree sizes against this server",
                block.height
            )
        });
        let ironwood_served =
            u64::from(block::shielded_output_count(&block, ShieldedPool::Ironwood));
        if commitment_tree_size(&metadata, ShieldedPool::Ironwood) > 0 {
            first_nonzero_ironwood_metadata.get_or_insert(block.height);
        }
        if ironwood_served > 0 {
            first_served_ironwood_action.get_or_insert(block.height);
        }
        ironwood_served_total += ironwood_served;
        if let Some(prev) = &prev {
            mismatches.extend(pool_mismatches(prev, &metadata, &block));
        }
        prev = Some(metadata);
    }

    assert!(
        mismatches.is_empty(),
        "indexer served blocks where chain_metadata growth does not match the \
         outputs present in vtx; the serving side drops outputs or misreports \
         tree sizes:\n{}",
        mismatches.join("\n"),
    );

    // The window must actually exercise ironwood serving, otherwise the delta
    // checks above pass vacuously on an indexer that serves neither ironwood
    // actions nor ironwood metadata.
    assert!(
        first_nonzero_ironwood_metadata.is_some_and(|height| height >= ironwood_activation),
        "ironwood metadata must first appear at or after the activation height \
         {ironwood_activation}, found {first_nonzero_ironwood_metadata:?}"
    );
    assert!(
        first_served_ironwood_action.is_some_and(|height| height >= ironwood_activation),
        "ironwood actions must first be served at or after the activation height \
         {ironwood_activation}, found {first_served_ironwood_action:?}"
    );
    assert!(
        ironwood_served_total > 0,
        "no ironwood action served across blocks [{start}, {end}]"
    );
}
