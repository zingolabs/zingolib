use super::*;
use crate::testutils::synthetic_wallet;
use crate::wallet::transparent::{OP_PUSHDATA1, OP_RETURN};
use tokio_stream::StreamExt as _;

const REWARD: Zatoshis = Zatoshis::const_from_u64(1_000_000);
const SPEND_VALUE: Zatoshis = Zatoshis::const_from_u64(990_000);
const OVERSPEND_BY: Zatoshis = Zatoshis::const_from_u64(1);
const PAYMENT: u64 = 10_000;
const HEIGHT_ONE: BlockHeight = BlockHeight::from_u32(1);
const HEIGHT_TWO: BlockHeight = BlockHeight::from_u32(2);

async fn funding_bytes(chain: &MockChain) -> Vec<u8> {
    chain
        .funding_transaction(vec![(
            &synthetic_wallet::external_address(PoolType::ORCHARD),
            PAYMENT,
            None,
        )])
        .await
}

fn lax_anchors() -> MockChain {
    let mut chain = MockChain::new();
    chain.rules.anchors = false;
    chain
}

fn pre_ironwood_activation_heights() -> ActivationHeights {
    crate::testutils::mock_activation_heights_deferring_nu6_3(1_000)
}

fn transparent_spend(
    chain: &MockChain,
    outpoint: OutPoint,
    value: Zatoshis,
    recipient: &str,
) -> Vec<u8> {
    let empty_script = Script::read([0u8].as_slice()).unwrap();
    let input = TxIn::from_parts(outpoint, empty_script, u32::MAX);
    let output = TxOut::new(value, taddr_script(recipient, &chain.chain_type()).unwrap());
    transparent_only_transaction(
        &chain.chain_type(),
        chain.next_height(),
        vec![input],
        vec![output],
    )
}

fn txid_of(chain: &MockChain, bytes: &[u8]) -> TxId {
    chain.parse_transaction(bytes, chain.next_height()).txid()
}

fn first_output(txid: TxId) -> OutPoint {
    OutPoint::new(*txid.as_ref(), 0)
}

#[test]
fn zip317_check_rejects_fee_below_minimum_rate() {
    let size = 2_000;
    assert_eq!(
        check_zip317_mempool_policy(4, Zatoshis::const_from_u64(100), size),
        Err(Rejection::FeeBelowMinimumRate {
            fee: Zatoshis::const_from_u64(100),
            required: Zatoshis::const_from_u64(200),
        })
    );
    assert_eq!(
        check_zip317_mempool_policy(4, Zatoshis::const_from_u64(200), size),
        Ok(())
    );
}

#[test]
fn zip317_check_rejects_unpaid_actions_above_block_limit() {
    assert_eq!(
        check_zip317_mempool_policy(BLOCK_UNPAID_ACTION_LIMIT + 2, MARGINAL_FEE, 100),
        Err(Rejection::UnpaidActions {
            unpaid: BLOCK_UNPAID_ACTION_LIMIT + 1,
            limit: BLOCK_UNPAID_ACTION_LIMIT
        })
    );
    assert_eq!(
        check_zip317_mempool_policy(BLOCK_UNPAID_ACTION_LIMIT + 1, MARGINAL_FEE, 100),
        Ok(())
    );
}

#[tokio::test]
async fn faucet_transaction_is_rejected_for_unknown_anchor() {
    let mut chain = MockChain::new();
    let bytes = funding_bytes(&chain).await;
    let rejection = chain.submit_transaction(bytes).unwrap_err();
    assert!(
        matches!(rejection, Rejection::UnknownAnchor(_)),
        "{rejection}"
    );
    assert_eq!(chain.mempool_len(), 0);
}

#[tokio::test]
async fn resubmission_with_revealed_nullifier_is_rejected() {
    let mut chain = lax_anchors();
    let bytes = funding_bytes(&chain).await;
    chain.submit_transaction(bytes.clone()).unwrap();
    let in_mempool = chain.submit_transaction(bytes.clone()).unwrap_err();
    assert!(matches!(in_mempool, Rejection::DuplicateNullifier(_)));
    chain.mine_mempool();
    let mined = chain.submit_transaction(bytes).unwrap_err();
    assert!(matches!(mined, Rejection::DuplicateNullifier(_)), "{mined}");
}

#[tokio::test]
async fn reorg_removes_nullifiers_of_evicted_blocks() {
    let mut chain = lax_anchors();
    let bytes = funding_bytes(&chain).await;
    chain.mine_empty_blocks(2);
    chain.submit_transaction(bytes.clone()).unwrap();
    chain.mine_mempool();
    assert!(chain.submit_transaction(bytes.clone()).is_err());
    let evicted = chain.reorg_to(2);
    assert_eq!(evicted, vec![bytes.clone()]);
    chain.submit_transaction(bytes).unwrap();
}

#[tokio::test]
async fn expired_mempool_transactions_are_evicted_when_a_block_is_mined() {
    let mut chain = lax_anchors();
    let bytes = funding_bytes(&chain).await;
    let expiry = chain.parse_transaction(&bytes, HEIGHT_ONE).expiry_height();
    chain.submit_transaction(bytes).unwrap();
    chain.mine_empty_blocks(u32::from(expiry) - 1);
    assert_eq!(chain.mempool_len(), 1);
    chain.mine_empty_blocks(1);
    assert_eq!(chain.mempool_len(), 0);
}

#[tokio::test]
async fn expired_transaction_is_rejected() {
    let mut chain = lax_anchors();
    let bytes = funding_bytes(&chain).await;
    let expiry = chain.parse_transaction(&bytes, HEIGHT_ONE).expiry_height();
    assert!(expiry > NO_EXPIRY);
    chain.mine_empty_blocks(u32::from(expiry));
    let rejection = chain.submit_transaction(bytes).unwrap_err();
    assert_eq!(
        rejection,
        Rejection::Expired {
            expiry,
            next_height: expiry + 1
        }
    );
}

#[tokio::test]
async fn transaction_with_wrong_branch_id_is_rejected() {
    let mut chain = MockChain::with_activation_heights(pre_ironwood_activation_heights());
    chain.rules.anchors = false;
    let bytes = faucet_funding_transaction_on(
        crate::testutils::mock_activation_heights(),
        chain.next_height(),
        vec![(
            &synthetic_wallet::external_address(PoolType::ORCHARD),
            PAYMENT,
            None,
        )],
    )
    .await;
    let rejection = chain.submit_transaction(bytes).unwrap_err();
    assert!(
        matches!(
            rejection,
            Rejection::BranchId { .. } | Rejection::Unparseable(_)
        ),
        "{rejection}"
    );
}

#[test]
fn coinbase_outputs_mature_after_one_hundred_blocks() {
    let mut chain = MockChain::new();
    let miner = synthetic_wallet::external_address(PoolType::Transparent);
    let coinbase = chain.mine_block_rewarding(&miner, REWARD, vec![]);
    let coinbase_outpoint = first_output(txid_of(&chain, &coinbase));
    assert!(
        chain
            .parse_transaction(&coinbase, HEIGHT_ONE)
            .transparent_bundle()
            .unwrap()
            .is_coinbase()
    );

    let spend = transparent_spend(&chain, coinbase_outpoint.clone(), SPEND_VALUE, &miner);
    assert_eq!(
        chain.submit_transaction(spend.clone()).unwrap_err(),
        Rejection::ImmatureCoinbase {
            mined_at: HEIGHT_ONE,
            next_height: HEIGHT_TWO
        }
    );
    chain.mine_empty_blocks(COINBASE_MATURITY_BLOCKS - 2);
    assert!(matches!(
        chain.submit_transaction(spend.clone()).unwrap_err(),
        Rejection::ImmatureCoinbase { .. }
    ));
    chain.mine_empty_blocks(1);
    chain.submit_transaction(spend.clone()).unwrap();
    assert_eq!(
        chain.submit_transaction(spend.clone()).unwrap_err(),
        Rejection::SpentInput(coinbase_outpoint.clone())
    );
    chain.mine_mempool();
    assert_eq!(
        chain.submit_transaction(spend).unwrap_err(),
        Rejection::SpentInput(coinbase_outpoint)
    );
}

#[test]
fn the_genesis_block_precedes_the_first_block_and_follows_nothing() {
    let mut chain = MockChain::new();
    chain.mine_empty_blocks(1);
    let genesis = chain.genesis_compact_block();
    assert_eq!(genesis.height, 0);
    assert_eq!(genesis.prev_hash, NO_PREDECESSOR_HASH.to_vec());
    assert_eq!(
        genesis.chain_metadata,
        Some(chain_metadata(
            &SaplingTree::empty(),
            &OrchardTree::empty(),
            &OrchardTree::empty()
        ))
    );
    assert_eq!(chain.blocks[0].prev_hash, genesis.hash);
    assert_eq!(
        chain.tree_state_at(0).map(|state| state.hash),
        Some(hex::encode(&genesis.hash))
    );
}

#[test]
fn unknown_transparent_input_is_rejected() {
    let mut chain = MockChain::new();
    let miner = synthetic_wallet::external_address(PoolType::Transparent);
    let outpoint = OutPoint::new([7; 32], 0);
    let spend = transparent_spend(&chain, outpoint.clone(), SPEND_VALUE, &miner);
    assert_eq!(
        chain.submit_transaction(spend).unwrap_err(),
        Rejection::UnknownInput(outpoint)
    );
}

#[test]
fn mempool_outputs_are_spendable_before_they_are_mined() {
    let mut chain = MockChain::new();
    let miner = synthetic_wallet::external_address(PoolType::Transparent);
    let coinbase = chain.mine_block_rewarding(&miner, REWARD, vec![]);
    chain.mine_empty_blocks(COINBASE_MATURITY_BLOCKS);
    let first = transparent_spend(
        &chain,
        first_output(txid_of(&chain, &coinbase)),
        SPEND_VALUE,
        &miner,
    );
    let first_txid = chain.submit_transaction(first).unwrap();
    let chained_value = Zatoshis::const_from_u64(980_000);
    let second = transparent_spend(&chain, first_output(first_txid), chained_value, &miner);
    chain.submit_transaction(second.clone()).unwrap();
    assert_eq!(
        chain.submit_transaction(second).unwrap_err(),
        Rejection::SpentInput(first_output(first_txid))
    );
    assert_eq!(chain.mempool_len(), 2);
    chain.mine_mempool();
    let unspent = chain.unspent_outputs(&miner, HEIGHT_ONE).unwrap();
    assert_eq!(unspent.len(), 1);
    assert_eq!(
        unspent[0].value_zat,
        i64::from(ZatBalance::from(chained_value))
    );
}

#[test]
fn outputs_above_inputs_are_rejected() {
    let mut chain = MockChain::new();
    let miner = synthetic_wallet::external_address(PoolType::Transparent);
    let coinbase = chain.mine_block_rewarding(&miner, REWARD, vec![]);
    let coinbase_outpoint = first_output(txid_of(&chain, &coinbase));
    chain.mine_empty_blocks(COINBASE_MATURITY_BLOCKS);
    let overspend = transparent_spend(
        &chain,
        coinbase_outpoint,
        (REWARD + OVERSPEND_BY).unwrap(),
        &miner,
    );
    assert_eq!(
        chain.submit_transaction(overspend).unwrap_err(),
        Rejection::NegativeFee(OVERSPEND_BY)
    );
}

#[test]
fn unspent_outputs_exclude_spent_outpoints() {
    let mut chain = MockChain::new();
    let miner = synthetic_wallet::external_address(PoolType::Transparent);
    let coinbase = chain.mine_block_rewarding(&miner, REWARD, vec![]);
    let coinbase_txid = txid_of(&chain, &coinbase);
    let unspent = chain.unspent_outputs(&miner, HEIGHT_ONE).unwrap();
    assert_eq!(unspent.len(), 1);
    assert_eq!(unspent[0].value_zat, i64::from(ZatBalance::from(REWARD)));
    assert_eq!(unspent[0].txid, coinbase_txid.as_ref().to_vec());
    assert_eq!(unspent[0].height, 1);
    assert!(
        chain
            .unspent_outputs(&miner, HEIGHT_TWO)
            .unwrap()
            .is_empty()
    );

    chain.mine_empty_blocks(COINBASE_MATURITY_BLOCKS);
    let spend = transparent_spend(&chain, first_output(coinbase_txid), SPEND_VALUE, &miner);
    chain.submit_transaction(spend).unwrap();
    chain.mine_mempool();
    let unspent = chain.unspent_outputs(&miner, HEIGHT_ONE).unwrap();
    assert_eq!(unspent.len(), 1);
    assert_eq!(
        unspent[0].value_zat,
        i64::from(ZatBalance::from(SPEND_VALUE))
    );
    assert_eq!(unspent[0].height, u64::from(chain.tip()));
}

#[tokio::test]
async fn mempool_stream_yields_new_transactions_and_closes_on_new_block() {
    let chain = Arc::new(RwLock::new(lax_anchors()));
    let service = MockIndexerService::new(chain.clone());
    let bytes = funding_bytes(&*chain.read().await).await;
    let mut stream = service
        .get_mempool_stream(Request::new(Empty {}))
        .await
        .unwrap()
        .into_inner();
    chain
        .write()
        .await
        .submit_transaction(bytes.clone())
        .unwrap();
    let arrived = stream.next().await.unwrap().unwrap();
    assert_eq!(arrived.data, bytes);
    chain.write().await.mine_mempool();
    assert!(stream.next().await.is_none());

    let mut replayed = service
        .get_mempool_stream(Request::new(Empty {}))
        .await
        .unwrap()
        .into_inner();
    assert!(
        tokio::time::timeout(Duration::from_millis(50), replayed.next())
            .await
            .is_err()
    );
}

#[tokio::test]
async fn mempool_stream_yields_existing_mempool_on_subscription() {
    let chain = Arc::new(RwLock::new(lax_anchors()));
    let service = MockIndexerService::new(chain.clone());
    let bytes = funding_bytes(&*chain.read().await).await;
    chain
        .write()
        .await
        .submit_transaction(bytes.clone())
        .unwrap();
    let mut stream = service
        .get_mempool_stream(Request::new(Empty {}))
        .await
        .unwrap()
        .into_inner();
    assert_eq!(stream.next().await.unwrap().unwrap().data, bytes);
}

#[tokio::test]
async fn address_utxos_stream_applies_injected_stream_faults() {
    let chain = Arc::new(RwLock::new(MockChain::new()));
    let miner = synthetic_wallet::external_address(PoolType::Transparent);
    {
        let mut chain = chain.write().await;
        chain.mine_block_rewarding(&miner, REWARD, vec![]);
        chain.mine_block_rewarding(&miner, REWARD, vec![]);
        chain
            .faults
            .inject(Rpc::AddressUtxos, Fault::TruncateStream { after: 1 });
    }
    let service = MockIndexerService::new(chain.clone());
    let arg = GetAddressUtxosArg {
        addresses: vec![miner],
        start_height: 1,
        max_entries: 0,
    };
    let mut stream = service
        .get_address_utxos_stream(Request::new(arg))
        .await
        .unwrap()
        .into_inner();
    assert!(stream.next().await.unwrap().is_ok());
    assert_eq!(
        stream.next().await.unwrap().unwrap_err().code(),
        Code::Unavailable
    );
    assert!(stream.next().await.is_none());
    assert_eq!(chain.read().await.faults.pending(Rpc::AddressUtxos), 0);
}

#[tokio::test]
async fn injected_faults_fail_delay_and_truncate_rpcs() {
    let chain = Arc::new(RwLock::new(MockChain::new()));
    chain.write().await.mine_empty_blocks(3);
    let service = MockIndexerService::new(chain.clone());
    {
        let mut chain = chain.write().await;
        chain.faults.inject(
            Rpc::LatestBlock,
            Fault::Fail(Code::Unavailable, "mock outage".to_string()),
        );
        chain
            .faults
            .inject(Rpc::LatestBlock, Fault::Delay(Duration::from_millis(20)));
        chain
            .faults
            .inject(Rpc::BlockRange, Fault::TruncateStream { after: 1 });
        chain
            .faults
            .inject(Rpc::BlockRange, Fault::EndStream { after: 2 });
    }

    let outage = service
        .get_latest_block(Request::new(ChainSpec {}))
        .await
        .unwrap_err();
    assert_eq!(outage.code(), Code::Unavailable);
    let started = std::time::Instant::now();
    let tip = service
        .get_latest_block(Request::new(ChainSpec {}))
        .await
        .unwrap()
        .into_inner();
    assert!(started.elapsed() >= Duration::from_millis(20));
    assert_eq!(tip.height, 3);
    assert_eq!(chain.read().await.faults.pending(Rpc::LatestBlock), 0);

    let range = BlockRange {
        start: Some(BlockId {
            height: 1,
            hash: vec![],
        }),
        end: Some(BlockId {
            height: 3,
            hash: vec![],
        }),
        pool_types: vec![],
    };
    let mut truncated = service
        .get_block_range(Request::new(range.clone()))
        .await
        .unwrap()
        .into_inner();
    assert_eq!(truncated.next().await.unwrap().unwrap().height, 1);
    let eof = truncated.next().await.unwrap().unwrap_err();
    assert!(eof.message().contains(STREAM_EOF_MESSAGE));
    assert!(truncated.next().await.is_none());

    let mut ended = service
        .get_block_range(Request::new(range.clone()))
        .await
        .unwrap()
        .into_inner();
    assert_eq!(ended.next().await.unwrap().unwrap().height, 1);
    assert_eq!(ended.next().await.unwrap().unwrap().height, 2);
    assert!(ended.next().await.is_none());

    let mut whole = service
        .get_block_range(Request::new(range))
        .await
        .unwrap()
        .into_inner();
    let mut served = 0;
    while let Some(block) = whole.next().await {
        block.unwrap();
        served += 1;
    }
    assert_eq!(served, 3);
}

#[tokio::test]
async fn send_transaction_returns_rejection_in_zaino_error_format() {
    let chain = Arc::new(RwLock::new(MockChain::new()));
    let service = MockIndexerService::new(chain.clone());
    let bytes = funding_bytes(&*chain.read().await).await;
    let status = service
        .send_transaction(Request::new(RawTransaction {
            data: bytes,
            height: 0,
        }))
        .await
        .unwrap_err();
    assert_eq!(status.code(), Code::Internal);
    assert!(status.message().starts_with(ZAINO_SEND_ERROR_PREFIX));
    assert!(status.message().contains("anchor"));
    assert_eq!(
        crate::lightclient::transmit::classify_rejection(status.message()),
        crate::lightclient::transmit::RejectionClass::Transient
    );
    assert_eq!(chain.read().await.mempool_len(), 0);
}

#[tokio::test]
async fn lightd_info_returns_sapling_activation_height_and_branch_id() {
    let chain = Arc::new(RwLock::new(MockChain::with_activation_heights(
        pre_ironwood_activation_heights(),
    )));
    let service = MockIndexerService::new(chain.clone());
    let info = service
        .get_lightd_info(Request::new(Empty {}))
        .await
        .unwrap()
        .into_inner();
    assert_eq!(info.sapling_activation_height, 1);
    let expected: u32 = BranchId::Nu6_2.into();
    assert_eq!(info.consensus_branch_id, format!("{expected:08x}"));
}

fn with_expiry(chain: &MockChain, bytes: &[u8], expiry: BlockHeight) -> Vec<u8> {
    let data = chain
        .parse_transaction(bytes, chain.next_height())
        .into_data();
    let transparent = data
        .transparent_bundle()
        .cloned()
        .unwrap_or(TransparentBundle {
            vin: vec![],
            vout: vec![],
            authorization: TransparentAuthorized,
        });
    transaction_bytes_for_branch(
        data.consensus_branch_id(),
        expiry,
        transparent,
        data.sapling_bundle().cloned(),
        data.orchard_bundle().cloned(),
        data.ironwood_bundle().cloned(),
    )
}

fn with_corrupted_slice(bytes: &[u8], slice: &[u8]) -> Vec<u8> {
    let offset = bytes
        .windows(slice.len())
        .position(|window| window == slice)
        .expect("a bundle's proof is serialized verbatim inside its transaction");
    let mut corrupted = bytes.to_vec();
    corrupted[offset] ^= 1;
    corrupted
}

fn null_data_transaction(chain: &MockChain, payloads: &[usize]) -> Vec<u8> {
    let outputs = payloads
        .iter()
        .map(|&payload| {
            let mut script = vec![OP_RETURN, OP_PUSHDATA1, payload as u8];
            script.extend(std::iter::repeat_n(0xab, payload));
            let mut serialized = vec![script.len() as u8];
            serialized.extend(&script);
            TxOut::new(
                Zatoshis::ZERO,
                Script::read(serialized.as_slice())
                    .expect("a length-prefixed byte vector reads as a script"),
            )
        })
        .collect();
    let input = TxIn::from_parts(
        OutPoint::new([0; 32], 0),
        Script::read([0u8].as_slice()).expect("an empty script reads"),
        u32::MAX,
    );
    transparent_only_transaction(
        &chain.chain_type(),
        chain.next_height(),
        vec![input],
        outputs,
    )
}

fn only_standard_scripts() -> MockChain {
    let mut chain = MockChain::new();
    chain.rules = Rules::LAX;
    chain.rules.standard_scripts = true;
    chain
}

#[tokio::test]
async fn orchard_bundle_signed_over_another_sighash_is_rejected() {
    let mut chain = lax_anchors();
    let bytes = funding_bytes(&chain).await;
    let expiry = chain.parse_transaction(&bytes, HEIGHT_ONE).expiry_height();
    let resigned = with_expiry(&chain, &bytes, expiry + 1);
    let rejection = chain
        .submit_transaction(resigned)
        .expect_err("signatures over the old header do not verify over the new one");
    assert!(
        matches!(rejection, Rejection::InvalidOrchardBundle(_)),
        "{rejection}"
    );
    assert_eq!(chain.mempool_len(), 0);
}

#[tokio::test]
async fn orchard_bundle_with_corrupted_proof_is_rejected() {
    let mut chain = lax_anchors();
    let bytes = funding_bytes(&chain).await;
    let transaction = chain.parse_transaction(&bytes, HEIGHT_ONE);
    let bundle = transaction
        .ironwood_bundle()
        .or(transaction.orchard_bundle())
        .expect("the faucet pays an orchard-family receiver through an orchard-family bundle");
    let corrupted = with_corrupted_slice(&bytes, bundle.authorization().proof().as_ref());
    let rejection = chain
        .submit_transaction(corrupted)
        .expect_err("a corrupted proof does not verify");
    assert!(
        matches!(rejection, Rejection::InvalidOrchardBundle(_)),
        "{rejection}"
    );
}

#[tokio::test]
async fn sapling_bundle_signed_over_another_sighash_is_rejected() {
    let mut chain = lax_anchors();
    let bytes = chain
        .funding_transaction(vec![(
            &synthetic_wallet::external_address(PoolType::SAPLING),
            PAYMENT,
            None,
        )])
        .await;
    let expiry = chain.parse_transaction(&bytes, HEIGHT_ONE).expiry_height();
    let resigned = with_expiry(&chain, &bytes, expiry + 1);
    let rejection = chain
        .submit_transaction(resigned)
        .expect_err("the binding signature over the old header does not verify");
    assert_eq!(rejection, Rejection::InvalidSaplingBundle);
}

#[tokio::test]
async fn sapling_bundle_with_corrupted_output_proof_is_rejected() {
    let mut chain = lax_anchors();
    let bytes = chain
        .funding_transaction(vec![(
            &synthetic_wallet::external_address(PoolType::SAPLING),
            PAYMENT,
            None,
        )])
        .await;
    let transaction = chain.parse_transaction(&bytes, HEIGHT_ONE);
    let output = transaction
        .sapling_bundle()
        .and_then(|bundle| bundle.shielded_outputs().first())
        .expect("the faucet pays a sapling receiver through a sapling output");
    let corrupted = with_corrupted_slice(&bytes, output.zkproof().as_slice());
    let rejection = chain
        .submit_transaction(corrupted)
        .expect_err("a corrupted output proof does not verify");
    assert_eq!(rejection, Rejection::InvalidSaplingBundle);
}

#[tokio::test]
async fn valid_sapling_and_orchard_bundles_are_accepted_under_verification() {
    let mut chain = lax_anchors();
    let bytes = chain
        .funding_transaction(vec![(
            &synthetic_wallet::external_address(PoolType::SAPLING),
            PAYMENT,
            None,
        )])
        .await;
    chain
        .submit_transaction(bytes)
        .expect("the faucet's own proofs and signatures verify");
    assert_eq!(chain.mempool_len(), 1);
}

#[test]
fn null_data_output_within_the_payload_limit_is_standard() {
    let chain = only_standard_scripts();
    let bytes = null_data_transaction(&chain, &[MAX_OP_RETURN_BYTES]);
    assert!(chain.validate(&bytes).is_ok());
}

#[test]
fn null_data_output_above_the_payload_limit_is_rejected() {
    let chain = only_standard_scripts();
    let bytes = null_data_transaction(&chain, &[MAX_OP_RETURN_BYTES + 1]);
    let rejection = chain
        .validate(&bytes)
        .expect_err("an oversized null-data output is non-standard");
    assert_eq!(rejection, Rejection::NonStandardScript { index: 0 });
}

#[test]
fn two_null_data_outputs_are_rejected() {
    let chain = only_standard_scripts();
    let bytes = null_data_transaction(&chain, &[1, 1]);
    let rejection = chain
        .validate(&bytes)
        .expect_err("a second null-data output is non-standard");
    assert_eq!(rejection, Rejection::MultipleNullDataOutputs { count: 2 });
}

#[test]
fn pay_to_public_key_hash_outputs_are_standard() {
    let chain = only_standard_scripts();
    let output = TxOut::new(
        REWARD,
        taddr_script(
            &synthetic_wallet::external_address(PoolType::Transparent),
            &chain.chain_type(),
        )
        .expect("a synthetic wallet's transparent address encodes a script"),
    );
    let input = TxIn::from_parts(
        OutPoint::new([0; 32], 0),
        Script::read([0u8].as_slice()).expect("an empty script reads"),
        u32::MAX,
    );
    let bytes = transparent_only_transaction(
        &chain.chain_type(),
        chain.next_height(),
        vec![input],
        vec![output],
    );
    assert!(chain.validate(&bytes).is_ok());
}
