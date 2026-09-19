//! Regenerates the pinned wallet file that holds a live migration.
//!
//! The file proves that a wallet written by an older release still loads
//! and that its migration section upgrades in place. The tests in
//! `zingolib/src/wallet/disk/testing/tests.rs` read it.
//!
//! The binary drives the migration on the in-process mock chain: a regtest
//! chain with NU6.3 a few blocks ahead, three funding notes mined before
//! activation, a commit, a schedule with one transfer per window, and two
//! broadcast windows, which leaves one transfer confirmed, one broadcast,
//! and one assigned. It then writes the wallet bytes to the pinned path, or
//! to the path given as the first argument, and prints the plan hashes and
//! one line for each transfer.
//!
//! Run it only when the wallet format changes on purpose:
//!
//! ```text
//! cargo run -p zingolib_testutils --bin gen-migration-fixture [output path]
//! ```
//!
//! The pinned file carries a migration section of an older version. Running
//! the binary rewrites it at the current version, and the upgrade tests then
//! have nothing old to read.

use std::path::PathBuf;

use zingolib_testutils::fixtures::{
    MIGRATION_FIXTURE_RELATIVE_PATH, generate_migration_fixture, write_migration_fixture,
};

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn output_path() -> PathBuf {
    match std::env::args().nth(1) {
        Some(path) => PathBuf::from(path),
        None => PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join(MIGRATION_FIXTURE_RELATIVE_PATH),
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let output = output_path();
    let fixture = generate_migration_fixture().await;
    write_migration_fixture(&fixture, &output);

    let state = &fixture.state;
    println!("wrote {} ({} bytes)", output.display(), fixture.bytes.len());
    println!("tip {}", fixture.tip);
    println!("params_hash {}", hex(&state.commitment().params_hash));
    println!("plan_hash {}", hex(&state.commitment().plan_hash));
    println!("committed_at {}", state.commitment().committed_at);
    for transfer in state.transfers() {
        let note = transfer.note.unwrap();
        println!(
            "transfer {} denomination {} funding note {}:{} window {:?} scheduled_broadcast_height {:?} state {:?} txid {:?} expiry {:?} attempts {} previous_txids {:?} missed_windows {} witness_position {:?}",
            transfer.id.0,
            transfer.denomination,
            note.output_id.txid(),
            note.output_id.output_index(),
            transfer.bucket_index,
            transfer.target_height,
            transfer.state,
            transfer.txid,
            transfer.expiry_height,
            transfer.attempts,
            transfer.previous_txids,
            transfer.missed_windows,
            transfer
                .anchor_witness
                .as_ref()
                .map(|witness| witness.position),
        );
    }
}
