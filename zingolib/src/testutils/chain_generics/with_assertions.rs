//! lightclient functions with added assertions. used for tests.

use nonempty::NonEmpty;

use zcash_primitives::transaction::TxId;
use zcash_protocol::PoolType;
use zcash_protocol::consensus::BlockHeight;
use zcash_protocol::value::Zatoshis;

use zingo_netutils::Indexer as _;
use zingo_status::confirmation_status::ConfirmationStatus;

use crate::lightclient::DEFAULT_REQUEST_TIMEOUT;
use crate::lightclient::LightClient;
use crate::testutils::assertions::compare_fee;
use crate::testutils::assertions::for_each_proposed_transaction;
use crate::testutils::chain_generics::conduct_chain::ConductChain;
use crate::testutils::lightclient::from_inputs;
use crate::testutils::lightclient::get_base_address;
use crate::testutils::timed;
use crate::testutils::timestamped_test_log;
use crate::wallet::spend::proposal::Proposal;

/// this function handles inputs and their lifetimes to create a proposal
pub async fn to_clients_proposal(
    sender: &mut LightClient,
    sends: &[(&LightClient, PoolType, u64, Option<&str>)],
) -> Proposal {
    let mut subraw_receivers = vec![];
    for (recipient, pooltype, amount, memo_str) in sends {
        let address = get_base_address(recipient, *pooltype).await;
        subraw_receivers.push((address, amount, memo_str));
    }

    let raw_receivers = subraw_receivers
        .iter()
        .map(|(address, amount, opt_memo)| (address.as_str(), **amount, **opt_memo))
        .collect();

    from_inputs::propose(sender, raw_receivers).await.unwrap()
}

/// sends to any combo of recipient clients checks that each recipient also received the expected balances
/// test-only generic
/// NOTICE this function bumps the chain and syncs the client
/// `test_mempool` can be enabled when the test harness supports it
/// returns `Ok(total_fee`, `total_received`, `total_change`)
/// transparent address discovery is disabled due to generic test framework needing to be darkside compatible
pub async fn assure_propose_send_bump_sync_all_recipients<CC>(
    environment: &mut CC,
    sender: &mut LightClient,
    payments: Vec<(&str, u64, Option<&str>)>,
    recipients: Vec<&mut LightClient>,
    test_mempool: bool,
) -> Result<(Zatoshis, Zatoshis, Zatoshis), String>
where
    CC: ConductChain,
{
    timestamped_test_log("started integration-test send.");
    // Mine one block and sync the sender to the real tip before proposing.
    // This cures zebra's "could not validate orchard proof ... until the
    // next chain tip block" rejection, but not because tip-block notes are
    // unspendable: tip_spend_rejection's tip_note_to_orchard proves those
    // spend fine. The rejection hits orchard-output transactions built
    // adjacent to the height-5 NU6.1/6.2 co-activation (a wrong consensus
    // branch id from a stale wallet view); syncing to the true tip keeps
    // the builder on the post-activation branch.
    timed(
        "assure_send::increase_chain_height",
        environment.increase_chain_height(),
    )
    .await;
    timed(
        "assure_send::sync_sender_to_tip",
        environment.sync_client_to_tip(sender),
    )
    .await;
    timestamped_test_log("syncked.");
    let proposal = timed(
        "assure_send::propose",
        from_inputs::propose(sender, payments.clone()),
    )
    .await
    .unwrap();
    timestamped_test_log(format!("proposed the following payments: {payments:?}").as_str());
    let txids = timed(
        "assure_send::send_stored_proposal",
        sender.send_stored_proposal(true),
    )
    .await
    .unwrap();
    timestamped_test_log("Transmitted send.");

    timed(
        "assure_send::follow_proposal",
        follow_proposal(
            environment,
            sender,
            recipients,
            &proposal,
            txids,
            test_mempool,
        ),
    )
    .await
}

/// a test-only generic version of shield that includes assertions that the proposal was fulfilled
/// NOTICE this function bumps the chain and syncs the client
/// only compatible with zip317
/// returns `Ok(total_fee`, `total_shielded`)
pub async fn assure_propose_shield_bump_sync<ChainConductor>(
    environment: &mut ChainConductor,
    client: &mut LightClient,
    test_mempool: bool,
) -> Result<(Zatoshis, Zatoshis), String>
where
    ChainConductor: ConductChain,
{
    timestamped_test_log("started integration-test shield.");
    // The same tip-separation as assure_propose_send_bump_sync_all_recipients.
    timed(
        "assure_shield::increase_chain_height",
        environment.increase_chain_height(),
    )
    .await;
    timed(
        "assure_shield::sync_client_to_tip",
        environment.sync_client_to_tip(client),
    )
    .await;
    timestamped_test_log("syncked.");
    let proposal = timed(
        "assure_shield::propose_shield",
        client.propose_shield(zip32::AccountId::ZERO),
    )
    .await
    .map_err(|e| e.to_string())?;
    timestamped_test_log(format!("proposed a shield: {proposal:#?}").as_str());

    let txids = timed(
        "assure_shield::send_stored_proposal",
        client.send_stored_proposal(true),
    )
    .await
    .unwrap();
    timestamped_test_log("Transmitted shield.");

    let (total_fee, _, s_shielded) = timed(
        "assure_shield::follow_proposal",
        follow_proposal(environment, client, vec![], &proposal, txids, test_mempool),
    )
    .await?;
    Ok((total_fee, s_shielded))
}

/// given a just-transmitted proposal, confirms that it achieves all expected checkpoints.
/// returns `Ok(total_fee`, `total_received`, `total_change`)
pub async fn follow_proposal<ChainConductor>(
    environment: &mut ChainConductor,
    sender: &mut LightClient,
    mut recipients: Vec<&mut LightClient>,
    proposal: &Proposal,
    txids: NonEmpty<TxId>,
    test_mempool: bool,
) -> Result<(Zatoshis, Zatoshis, Zatoshis), String>
where
    ChainConductor: ConductChain,
{
    let patience = environment.confirmation_patience_blocks();

    timestamped_test_log("following proposal, preparing to unwind if an assertion fails.");

    let mut indexer = timed(
        "follow_proposal::grpc_indexer_connect",
        zingo_netutils::GrpcIndexer::new(environment.lightserver_uri().unwrap()),
    )
    .await
    .unwrap();
    let server_height_at_send = BlockHeight::from(
        timed(
            "follow_proposal::get_latest_block",
            indexer.get_latest_block(DEFAULT_REQUEST_TIMEOUT),
        )
        .await
        .unwrap()
        .height as u32,
    );
    let last_known_chain_height = timed("follow_proposal::read_wallet_height", async {
        sender
            .wallet()
            .read()
            .await
            .sync_state
            .last_known_chain_height()
            .unwrap()
    })
    .await;
    timestamped_test_log(format!("wallet height at send {last_known_chain_height}").as_str());

    // check that each record has the expected fee and status, returning the fee
    let (sender_recorded_fees, (sender_recorded_outputs, sender_recorded_statuses)): (
        Vec<Zatoshis>,
        (Vec<Zatoshis>, Vec<ConfirmationStatus>),
    ) = timed(
        "follow_proposal::check_recorded_records",
        for_each_proposed_transaction(sender, proposal, &txids, |wallet, transaction, step| {
            (
                compare_fee(wallet, transaction, step),
                (
                    Zatoshis::from_u64(transaction.total_value_received()),
                    transaction.status(),
                ),
            )
        }),
    )
    .await
    .into_iter()
    .map(|stepwise_result| {
        let (fee_comparison_result, others) = stepwise_result.unwrap();
        let (balance, confirmation_status) = others;
        (
            fee_comparison_result.unwrap(),
            (balance.unwrap(), confirmation_status),
        )
    })
    .unzip();

    for status in sender_recorded_statuses {
        if !matches!(
            status,
            ConfirmationStatus::Transmitted(transmitted_status_height) if transmitted_status_height == last_known_chain_height + 1
        ) {
            tracing::debug!("{status:?}");
            tracing::debug!("{last_known_chain_height:?}");
            panic!();
        }
    }

    let option_recipient_mempool_outputs = if test_mempool {
        timestamped_test_log("syncking transaction from mempool.");
        // mempool scan shows the same
        timed(
            "follow_proposal::mempool_sender_sync",
            sender.sync_and_await(),
        )
        .await
        .unwrap();
        timestamped_test_log("cross-checking mempool records.");

        // Wait for the mempool monitor to observe every transaction, polling
        // for the exact condition the assertions below check instead of
        // sleeping a fixed interval. The 6-second ceiling matches the old
        // fixed sleep, so the worst case is unchanged while the typical
        // wait drops to the monitor's actual latency.
        let poll_start = std::time::Instant::now();
        loop {
            let all_observed = {
                let wallet = sender.wallet();
                let wallet = wallet.read().await;
                txids.iter().all(|txid| {
                    wallet
                        .wallet_transactions
                        .get(txid)
                        .is_some_and(|transaction| {
                            matches!(transaction.status(), ConfirmationStatus::Mempool(_))
                        })
                })
            };
            if all_observed || poll_start.elapsed() > std::time::Duration::from_secs(6) {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }

        // check that each record has the expected fee and status, returning the fee and outputs
        let (sender_mempool_fees, (sender_mempool_outputs, sender_mempool_statuses)): (
            Vec<Zatoshis>,
            (Vec<Zatoshis>, Vec<ConfirmationStatus>),
        ) = for_each_proposed_transaction(sender, proposal, &txids, |wallet, transaction, step| {
            (
                compare_fee(wallet, transaction, step),
                (
                    Zatoshis::from_u64(transaction.total_value_received()),
                    transaction.status(),
                ),
            )
        })
        .await
        .into_iter()
        .map(|stepwise_result| {
            let (fee_comparison_result, others) = stepwise_result.unwrap();
            let (balance, confirmation_status) = others;
            (
                fee_comparison_result.unwrap(),
                (balance.unwrap(), confirmation_status),
            )
        })
        .unzip();

        assert_eq!(sender_mempool_fees, sender_recorded_fees);
        assert_eq!(sender_mempool_outputs, sender_recorded_outputs);
        for status in sender_mempool_statuses {
            assert_eq!(
                status,
                ConfirmationStatus::Mempool(server_height_at_send + 1)
            );
        }

        let mut recipients_mempool_outputs: Vec<Vec<Zatoshis>> = vec![];
        for (recipient_index, recipient) in recipients.iter_mut().enumerate() {
            timed(
                format!("follow_proposal::mempool_recipient[{recipient_index}]_sync").as_str(),
                recipient.sync_and_await(),
            )
            .await
            .unwrap();

            // check that each record has the status, returning the output value
            let (recipient_mempool_outputs, recipient_mempool_statuses): (
                Vec<Zatoshis>,
                Vec<ConfirmationStatus>,
            ) = for_each_proposed_transaction(
                recipient,
                proposal,
                &txids,
                |_wallet, transaction, _step| {
                    (
                        Zatoshis::from_u64(transaction.total_value_received()).unwrap(),
                        transaction.status(),
                    )
                },
            )
            .await
            .into_iter()
            .map(|stepwise_result| stepwise_result.unwrap())
            .unzip();
            for status in recipient_mempool_statuses {
                assert_eq!(
                    status,
                    ConfirmationStatus::Mempool(server_height_at_send + 1)
                );
            }
            recipients_mempool_outputs.push(recipient_mempool_outputs);
        }
        Some(recipients_mempool_outputs)
    } else {
        None
    };

    timestamped_test_log("cross-checked mempool records.");

    let mut attempts = 0;
    loop {
        timed(
            format!("follow_proposal::confirm[{attempts}]::increase_chain_height").as_str(),
            environment.increase_chain_height(),
        )
        .await;
        timestamped_test_log("syncking transaction confirmation.");
        // chain scan shows the same
        timed(
            format!("follow_proposal::confirm[{attempts}]::sender_sync_to_tip").as_str(),
            environment.sync_client_to_tip(sender),
        )
        .await;
        let last_known_chain_height = sender
            .wallet()
            .read()
            .await
            .sync_state
            .last_known_chain_height()
            .unwrap();
        timestamped_test_log(format!("wallet height now {last_known_chain_height}").as_str());
        timestamped_test_log("cross-checking confirmed records.");

        // check that each record has the expected fee and status, returning the fee and outputs
        let (sender_confirmed_fees, (sender_confirmed_outputs, sender_confirmed_statuses)): (
            Vec<Zatoshis>,
            (Vec<Zatoshis>, Vec<ConfirmationStatus>),
        ) = timed(
            format!("follow_proposal::confirm[{attempts}]::check_confirmed_records").as_str(),
            for_each_proposed_transaction(sender, proposal, &txids, |wallet, transaction, step| {
                (
                    compare_fee(wallet, transaction, step),
                    (
                        Zatoshis::from_u64(transaction.total_value_received()),
                        transaction.status(),
                    ),
                )
            }),
        )
        .await
        .into_iter()
        .map(|stepwise_result| {
            let (fee_comparison_result, others) = stepwise_result.unwrap();
            let (balance, confirmation_status) = others;
            (
                fee_comparison_result.unwrap(),
                (balance.unwrap(), confirmation_status),
            )
        })
        .unzip();

        assert_eq!(sender_confirmed_fees, sender_recorded_fees);
        assert_eq!(sender_confirmed_outputs, sender_recorded_outputs);

        let mut any_transaction_not_yet_confirmed = false;
        for status in sender_confirmed_statuses {
            timestamped_test_log(format!("matching on transaction status {status}.").as_str());
            match status {
                ConfirmationStatus::Calculated(_block_height) => {
                    panic!("status regression to Calculated")
                }
                ConfirmationStatus::Transmitted(_block_height) => {
                    // Not a regression: status updates are monotonic, so this
                    // means the wallet has not yet observed the transaction in
                    // the mempool or a block. With instant regtest mining a tx
                    // is often mined before the mempool monitor sees it, and
                    // the confirming block may not be scanned yet (the Indexer
                    // lags the Validator by up to ~500ms after
                    // generate_blocks). Keep polling; the patience bound turns
                    // a persistent Transmitted into a failure.
                    any_transaction_not_yet_confirmed = true;
                }
                ConfirmationStatus::Mempool(_block_height) => {
                    any_transaction_not_yet_confirmed = true;
                }
                ConfirmationStatus::Confirmed(confirmed_height) => {
                    assert!(last_known_chain_height >= confirmed_height);
                }
                ConfirmationStatus::Failed(_block_height) => {
                    panic!("transaction failed")
                }
            }
        }
        if any_transaction_not_yet_confirmed {
            attempts += 1;
            assert!((attempts <= patience), "ran out of patience");
        } else {
            break;
        }
    }

    let mut recipients_confirmed_outputs = vec![];
    for (recipient_index, recipient) in recipients.iter_mut().enumerate() {
        timed(
            format!("follow_proposal::confirmed_recipient[{recipient_index}]_sync").as_str(),
            recipient.sync_and_await(),
        )
        .await
        .unwrap();

        // check that each record has the status, returning the output value
        let (recipient_confirmed_outputs, recipient_confirmed_statuses): (
            Vec<Zatoshis>,
            Vec<ConfirmationStatus>,
        ) = for_each_proposed_transaction(
            recipient,
            proposal,
            &txids,
            |_wallet, transaction, _step| {
                (transaction.total_value_received(), transaction.status())
            },
        )
        .await
        .into_iter()
        .map(|stepwise_result| stepwise_result.unwrap())
        .map(|(value, status)| (Zatoshis::from_u64(value).unwrap(), status))
        .collect();
        for status in recipient_confirmed_statuses {
            assert_eq!(
                status,
                ConfirmationStatus::Confirmed(server_height_at_send + 1)
            );
        }
        recipients_confirmed_outputs.push(recipient_confirmed_outputs);
    }
    timestamped_test_log("cross-checked confirmed records.");

    option_recipient_mempool_outputs.inspect(|recipient_mempool_outputs| {
        assert_eq!(recipients_confirmed_outputs, *recipient_mempool_outputs);
    });

    Ok((
        sender_recorded_fees
            .into_iter()
            .fold(Zatoshis::ZERO, |acc, x| (acc + x).unwrap()),
        recipients_confirmed_outputs
            .into_iter()
            .flatten()
            .fold(Zatoshis::ZERO, |acc, x| (acc + x).unwrap()),
        sender_recorded_outputs
            .into_iter()
            .fold(Zatoshis::ZERO, |acc, x| (acc + x).unwrap()), // this construction will be problematic when 2-step transactions mean some value is received and respent.
    ))
}
