//! lightclient functions with added assertions. used for tests.

use crate::lightclient::LightClient;
use crate::testutils::assertions::compare_fee;
use crate::testutils::assertions::for_each_proposed_record;
use crate::testutils::chain_generics::conduct_chain::ConductChain;
use crate::testutils::lightclient::from_inputs;
use crate::testutils::lightclient::get_base_address;
use crate::wallet::notes::query::OutputQuery;
use nonempty::NonEmpty;
use zcash_client_backend::proposal::Proposal;
use zcash_client_backend::PoolType;
use zcash_primitives::consensus::BlockHeight;
use zcash_primitives::transaction::TxId;
use zingo_status::confirmation_status::ConfirmationStatus;

/// this function handles inputs and their lifetimes to create a proposal
pub async fn to_clients_proposal(
    sender: &LightClient,
    sends: &[(&LightClient, PoolType, u64, Option<&str>)],
) -> zcash_client_backend::proposal::Proposal<
    zcash_primitives::transaction::fees::zip317::FeeRule,
    zcash_client_backend::wallet::NoteId,
> {
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
/// test_mempool can be enabled when the test harness supports it
/// returns Ok(total_fee, total_received, total_change)
pub async fn propose_send_bump_sync_all_recipients<CC>(
    environment: &mut CC,
    sender: &LightClient,
    sends: Vec<(&LightClient, PoolType, u64, Option<&str>)>,
    test_mempool: bool,
) -> Result<(u64, u64, u64), String>
where
    CC: ConductChain,
{
    sender.do_sync(false).await.unwrap();
    let proposal = to_clients_proposal(sender, &sends).await;

    let txids = sender
        .complete_and_broadcast_stored_proposal()
        .await
        .unwrap();

    follow_proposal(
        environment,
        sender,
        sends
            .iter()
            .map(|(recipient, _, _, _)| *recipient)
            .collect(),
        &proposal,
        txids,
        test_mempool,
    )
    .await
}

/// a test-only generic version of shield that includes assertions that the proposal was fulfilled
/// NOTICE this function bumps the chain and syncs the client
/// only compatible with zip317
/// returns Ok(total_fee, total_shielded)
pub async fn assure_propose_shield_bump_sync<CC>(
    environment: &mut CC,
    client: &LightClient,
    test_mempool: bool,
) -> Result<(u64, u64), String>
where
    CC: ConductChain,
{
    let proposal = client.propose_shield().await.map_err(|e| e.to_string())?;

    let txids = client
        .complete_and_broadcast_stored_proposal()
        .await
        .unwrap();

    let (total_fee, r_shielded, s_shielded) = follow_proposal(
        environment,
        client,
        vec![client],
        &proposal,
        txids,
        test_mempool,
    )
    .await?;
    assert_eq!(r_shielded, s_shielded);
    Ok((total_fee, s_shielded))
}

/// given a just-broadcast proposal, confirms that it achieves all expected checkpoints.
/// returns Ok(total_fee, total_received, total_change)
pub async fn follow_proposal<CC, NoteRef>(
    environment: &mut CC,
    sender: &LightClient,
    recipients: Vec<&LightClient>,
    proposal: &Proposal<zcash_primitives::transaction::fees::zip317::FeeRule, NoteRef>,
    txids: NonEmpty<TxId>,
    test_mempool: bool,
) -> Result<(u64, u64, u64), String>
where
    CC: ConductChain,
{
    println!("following proposal, preparing to unwind if an assertion fails.");

    let server_height_at_send = BlockHeight::from(
        crate::grpc_connector::get_latest_block(environment.lightserver_uri().unwrap())
            .await
            .unwrap()
            .height as u32,
    );

    // check that each record has the expected fee and status, returning the fee
    let (sender_recorded_fees, (sender_recorded_outputs, sender_recorded_statuses)): (
        Vec<u64>,
        (Vec<u64>, Vec<ConfirmationStatus>),
    ) = for_each_proposed_record(sender, proposal, &txids, |records, record, step| {
        (
            compare_fee(records, record, step),
            (record.query_sum_value(OutputQuery::any()), record.status),
        )
    })
    .await
    .into_iter()
    .map(|stepwise_result| {
        stepwise_result
            .map(|(fee_comparison_result, others)| (fee_comparison_result.unwrap(), others))
            .unwrap()
    })
    .unzip();

    for status in sender_recorded_statuses {
        assert_eq!(
            status,
            ConfirmationStatus::Transmitted(server_height_at_send + 1)
        );
    }

    let option_recipient_mempool_outputs = if test_mempool {
        // mempool scan shows the same
        sender.do_sync(false).await.unwrap();

        // let the mempool monitor get a chance
        // to listen
        tokio::time::sleep(std::time::Duration::from_secs(6)).await;

        // check that each record has the expected fee and status, returning the fee and outputs
        let (sender_mempool_fees, (sender_mempool_outputs, sender_mempool_statuses)): (
            Vec<u64>,
            (Vec<u64>, Vec<ConfirmationStatus>),
        ) = for_each_proposed_record(sender, proposal, &txids, |records, record, step| {
            (
                compare_fee(records, record, step),
                (record.query_sum_value(OutputQuery::any()), record.status),
            )
        })
        .await
        .into_iter()
        .map(|stepwise_result| {
            stepwise_result
                .map(|(fee_comparison_result, others)| (fee_comparison_result.unwrap(), others))
                .unwrap()
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

        let mut recipients_mempool_outputs: Vec<Vec<u64>> = vec![];
        for recipient in recipients.clone() {
            recipient.do_sync(false).await.unwrap();

            // check that each record has the status, returning the output value
            let (recipient_mempool_outputs, recipient_mempool_statuses): (
                Vec<u64>,
                Vec<ConfirmationStatus>,
            ) = for_each_proposed_record(recipient, proposal, &txids, |_records, record, _step| {
                (record.query_sum_value(OutputQuery::any()), record.status)
            })
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

    environment.bump_chain().await;
    // chain scan shows the same
    sender.do_sync(false).await.unwrap();

    // check that each record has the expected fee and status, returning the fee and outputs
    let (sender_confirmed_fees, (sender_confirmed_outputs, sender_confirmed_statuses)): (
        Vec<u64>,
        (Vec<u64>, Vec<ConfirmationStatus>),
    ) = for_each_proposed_record(sender, proposal, &txids, |records, record, step| {
        (
            compare_fee(records, record, step),
            (record.query_sum_value(OutputQuery::any()), record.status),
        )
    })
    .await
    .into_iter()
    .map(|stepwise_result| {
        stepwise_result
            .map(|(fee_comparison_result, others)| (fee_comparison_result.unwrap(), others))
            .unwrap()
    })
    .unzip();

    assert_eq!(sender_confirmed_fees, sender_recorded_fees);
    assert_eq!(sender_confirmed_outputs, sender_recorded_outputs);
    for status in sender_confirmed_statuses {
        assert_eq!(
            status,
            ConfirmationStatus::Confirmed(server_height_at_send + 1)
        );
    }

    let mut recipients_confirmed_outputs = vec![];
    for recipient in recipients {
        recipient.do_sync(false).await.unwrap();

        // check that each record has the status, returning the output value
        let (recipient_confirmed_outputs, recipient_confirmed_statuses): (
            Vec<u64>,
            Vec<ConfirmationStatus>,
        ) = for_each_proposed_record(recipient, proposal, &txids, |_records, record, _step| {
            (record.query_sum_value(OutputQuery::any()), record.status)
        })
        .await
        .into_iter()
        .map(|stepwise_result| stepwise_result.unwrap())
        .collect();
        for status in recipient_confirmed_statuses {
            assert_eq!(
                status,
                ConfirmationStatus::Confirmed(server_height_at_send + 1)
            );
        }
        recipients_confirmed_outputs.push(recipient_confirmed_outputs);
    }

    option_recipient_mempool_outputs.inspect(|recipient_mempool_outputs| {
        assert_eq!(recipients_confirmed_outputs, *recipient_mempool_outputs);
    });

    Ok((
        sender_confirmed_fees.iter().sum(),
        recipients_confirmed_outputs.into_iter().flatten().sum(), // this construction is problematic, because receivers are often duplicates
        sender_confirmed_outputs.iter().sum(), // this construction will be problematic when 2-step transactions mean some value is received and respent.
    ))
}
