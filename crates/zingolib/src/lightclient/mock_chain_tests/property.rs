use proptest::prelude::*;
use zaino_proto::tonic::Code;
use zcash_protocol::{PoolType, ShieldedPool};

use super::*;
use crate::testutils::lightclient::from_inputs;
use crate::testutils::mock_indexer::{Fault, Rpc};
use crate::testutils::synthetic_wallet;
use pepper_sync::wallet::OutgoingNoteInterface as _;

const CASE_FUNDING: u64 = 300_000;
const FETCH_FAILURES: usize = 2;
const SENDS: [u64; 3] = [1_000, 10_000, 50_000];
const RECEIPTS: [u64; 1] = [30_000];

#[derive(Debug, Clone)]
enum Step {
    Sync,
    Send(u64),
    Receive(u64),
    MineMempool,
    MineEmpty(u32),
    WithholdFetches(bool),
}

fn any_step() -> impl Strategy<Value = Step> {
    prop_oneof![
        3 => Just(Step::Sync),
        3 => (0..SENDS.len()).prop_map(|index| Step::Send(SENDS[index])),
        1 => (0..RECEIPTS.len()).prop_map(|index| Step::Receive(RECEIPTS[index])),
        3 => Just(Step::MineMempool),
        2 => (0u32..4).prop_map(Step::MineEmpty),
        1 => proptest::bool::ANY.prop_map(Step::WithholdFetches),
    ]
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(4))]
    #[test]
    fn every_step_of_a_generated_session_holds_the_wallet_invariants(
        steps in prop::collection::vec(any_step(), 1..16),
    ) {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime
            .block_on(run_case(&steps))
            .map_err(|failure| TestCaseError::Fail(failure.into()))?;
    }
}

async fn run_case(steps: &[Step]) -> Result<(), String> {
    let mut net = MockNet::launch().await;
    let mut client = funded_client(&mut net, &[(PoolType::ORCHARD, CASE_FUNDING)], 1).await;
    let mut issued = CASE_FUNDING;

    for step in steps {
        match step {
            Step::Sync => (),
            Step::Send(value) => {
                let _ = from_inputs::quick_send(
                    &mut client,
                    vec![(
                        &synthetic_wallet::external_address(PoolType::ORCHARD),
                        *value,
                        None,
                    )],
                )
                .await;
            }
            Step::Receive(value) => {
                let address =
                    get_base_address(&client, PoolType::Shielded(ShieldedPool::Ironwood)).await;
                let funding = net
                    .funding_transaction(vec![(&address, *value, None)])
                    .await;
                net.chain.write().await.enter_mempool(funding);
                issued += value;
            }
            Step::MineMempool => net.chain.write().await.mine_mempool(),
            Step::MineEmpty(count) => net.chain.write().await.mine_empty_blocks(*count),
            Step::WithholdFetches(withhold) => {
                let mut chain = net.chain.write().await;
                if *withhold {
                    for _ in 0..FETCH_FAILURES {
                        chain.faults.inject(
                            Rpc::BlockRange,
                            Fault::Fail(Code::Unavailable, "mock outage".to_string()),
                        );
                    }
                } else {
                    chain.faults.clear(Rpc::BlockRange);
                }
            }
        }
        let _ = client.sync_and_await().await;
        check(&client, issued)
            .await
            .map_err(|failure| format!("after {step:?}: {failure}"))?;
    }

    net.chain.write().await.faults.clear(Rpc::BlockRange);
    net.chain.write().await.mine_mempool();
    net.chain.write().await.mine_empty_blocks(1);
    client
        .sync_and_await()
        .await
        .map_err(|error| format!("the settling session errors: {error}"))?;
    check_settled(&client, issued)
        .await
        .map_err(|failure| format!("at settlement: {failure}"))
}

/// The invariants hold and the wallet holds no more value than was issued.
async fn check(client: &LightClient, issued: u64) -> Result<(), String> {
    let wallet = client.wallet().read().await;
    let violations = wallet.invariant_violations();
    if !violations.is_empty() {
        return Err(violations.join("\n"));
    }
    let balance = wallet
        .account_balance(zip32::AccountId::ZERO)
        .map_err(|error| format!("account_balance errored: {error}"))?;
    let held = balance
        .total_ironwood_balance
        .expect("an ironwood-capable wallet reports an ironwood balance")
        .into_u64();
    if held > issued {
        return Err(format!(
            "the wallet holds {held}, more than the {issued} issued"
        ));
    }
    Ok(())
}

/// With every chain side settled, the held value equals funding plus receipts
/// minus what the sent transactions took.
async fn check_settled(client: &LightClient, issued: u64) -> Result<(), String> {
    let wallet = client.wallet().read().await;
    let violations = wallet.invariant_violations();
    if !violations.is_empty() {
        return Err(violations.join("\n"));
    }
    let mut expected = issued;
    for transaction in wallet.wallet_transactions.values() {
        if transaction.status().is_failed() {
            continue;
        }
        let payment: u64 = transaction
            .outgoing_orchard_notes()
            .iter()
            .map(|note| note.value())
            .sum::<u64>()
            + transaction
                .outgoing_sapling_notes()
                .iter()
                .map(|note| note.value())
                .sum::<u64>()
            + transaction
                .outgoing_ironwood_notes()
                .iter()
                .map(|note| note.value())
                .sum::<u64>();
        if payment == 0 {
            continue;
        }
        let fee = wallet
            .calculate_transaction_fee(transaction)
            .ok()
            .map(zcash_protocol::value::Zatoshis::into_u64)
            .unwrap_or(0);
        expected = expected.saturating_sub(payment + fee);
    }
    let balance = wallet
        .account_balance(zip32::AccountId::ZERO)
        .map_err(|error| format!("account_balance errored: {error}"))?;
    let held = balance
        .total_ironwood_balance
        .expect("an ironwood-capable wallet reports an ironwood balance")
        .into_u64();
    if held != expected {
        return Err(format!(
            "the wallet holds {held}, expected {expected} of the {issued} issued"
        ));
    }
    Ok(())
}
