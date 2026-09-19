use std::time::Duration;

use zcash_protocol::consensus::BlockHeight;

use super::fixtures::{
    TIP, assigned_transfer, current_bucket_of, migration_error, migration_of, scheduled_state,
    wallet_with_migration_note,
};
use crate::lightclient::LightClient;
use crate::lightclient::error::MigrationError;
use crate::wallet::disk::testing::examples::{
    HospitalMuseumVersion, NetworkSeedVersion, RegtestSeedVersion,
};
use crate::wallet::migration::{MigrationParams, MigrationPhase, schedule};

const FIXTURE_TIP: u32 = 460;

async fn fixture_client() -> LightClient {
    NetworkSeedVersion::Regtest(RegtestSeedVersion::HospitalMuseum(
        HospitalMuseumVersion::V42Migration,
    ))
    .load_example_wallet()
    .await
}

#[tokio::test]
async fn broadcast_due_transfers_refuses_a_wallet_that_last_saw_the_chain_over_a_window_ago() {
    let mut client = fixture_client().await;
    {
        let wallet = client.wallet().read().await;
        let (height, block) = wallet
            .wallet_blocks
            .iter()
            .next_back()
            .expect("the fixture scanned blocks");
        assert_eq!(*height, BlockHeight::from_u32(FIXTURE_TIP));
        let params = MigrationParams::provisional(wallet.chain_type());
        let window_seconds =
            u64::from(params.bucket_modulus) * schedule::TARGET_BLOCK_SPACING_SECONDS;
        assert!(
            u64::from(crate::utils::now()) - u64::from(block.time()) > window_seconds,
            "premise: the newest block is older than one window"
        );
        assert_eq!(
            wallet.migration.as_ref().map(|state| &state.phase),
            Some(&MigrationPhase::Scheduled),
            "premise: the fixture holds a scheduled migration"
        );
    }

    client
        .reconcile_migration()
        .await
        .expect("reconciliation runs");
    let settled = migration_of(&client).await;

    let result = client.broadcast_due_transfers(Duration::ZERO).await;

    assert!(
        matches!(
            migration_error(result),
            MigrationError::StaleChainView { last_known_height }
                if last_known_height == BlockHeight::from_u32(FIXTURE_TIP)
        ),
        "the refusal names the height the wallet last saw"
    );
    assert_eq!(
        migration_of(&client).await,
        settled,
        "nothing was attempted against a stale view"
    );
}

#[tokio::test]
async fn broadcast_missed_now_does_not_apply_the_stale_view_rule() {
    let mut client = fixture_client().await;
    client.migration_transmission_uri = Some("http://127.0.0.1:1".parse().expect("static uri"));

    let result = client.broadcast_missed_now(Duration::ZERO).await;

    assert!(
        !matches!(
            result,
            Err(crate::lightclient::error::LightClientError::MigrationError(
                MigrationError::StaleChainView { .. }
            ))
        ),
        "the disclosed send-now is the user's explicit choice, got {result:?}"
    );
}

#[tokio::test]
async fn a_wallet_with_no_scanned_blocks_is_never_stale() {
    let (mut wallet, bound_note) = wallet_with_migration_note(TIP);
    let params = MigrationParams::provisional(wallet.chain_type());
    let current_bucket = current_bucket_of(&wallet, &params);
    assert!(
        wallet.wallet_blocks.is_empty(),
        "premise: a synthetic wallet holds no blocks"
    );
    wallet.migration = Some(scheduled_state(
        params,
        vec![assigned_transfer(0, bound_note, current_bucket)],
    ));
    let mut client = LightClient::new_for_test(wallet).await;
    client.consent_to_clearnet_for_tests().await;
    client.migration_transmission_uri = Some("http://127.0.0.1:1".parse().expect("static uri"));

    let result = client.broadcast_due_transfers(Duration::ZERO).await;

    assert!(
        !matches!(
            result,
            Err(crate::lightclient::error::LightClientError::MigrationError(
                MigrationError::StaleChainView { .. }
            ))
        ),
        "the batch reached the transport, got {result:?}"
    );
}
