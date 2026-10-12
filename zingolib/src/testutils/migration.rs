//! - Writes hand-built migration state into a test wallet.

use pepper_sync::wallet::OutputId;
use zip32::AccountId;

use crate::lightclient::LightClient;
use crate::wallet::migration::{
    BoundNote, ConsentBinding, MigrationMode, MigrationParams, MigrationPhase, MigrationState,
    PartId, PartRecord, SigningStrategy,
};

/// - Replaces the wallet's migration state under the client's wallet write lock.
pub async fn inject_scheduled_migration(
    client: &LightClient,
    params: MigrationParams,
    bound: Vec<(u64, OutputId, [u8; 32])>,
    bucket: Option<u64>,
    anchor_bucket: Option<u64>,
) {
    let mut wallet = client.wallet().write().await;
    let parts = bound
        .into_iter()
        .enumerate()
        .map(|(index, (denomination, output_id, nullifier))| {
            let mut part = PartRecord::new(
                PartId(u32::try_from(index).expect("test part count fits u32")),
                denomination,
                BoundNote {
                    output_id,
                    nullifier,
                    commitment: [0; 32],
                },
            );
            if let Some(bucket) = bucket {
                part.assign(bucket).expect("fresh parts are bound");
            }
            part.anchor_bucket = anchor_bucket;
            part
        })
        .collect();
    wallet.migration = Some(MigrationState {
        consent: ConsentBinding {
            params_hash: params.params_hash(),
            plan_hash: [0; 32],
            consented_at: 0,
        },
        params,
        strategy: SigningStrategy::LazyAtBoundary,
        mode: MigrationMode::Scheduled,
        account: AccountId::ZERO,
        phase: MigrationPhase::PartsScheduled,
        parts,
    });
}
