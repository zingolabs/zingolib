//! The binary e2e proof scenario: drive the shipped zingo-cli binary
//! through every `Wallet` trait operation against a live
//! `LocalNet<Zebrad, Zainod>` on an NU6.2 regtest chain.
//!
//! This test pins the invocation contract (flags, stdout shapes,
//! `--activation-heights` alignment) against the real binary — the
//! parsers in `zingo_cli_harness::parse` are only trusted because this
//! exercises them live. The NU6.3/Ironwood leg (shield landing in the
//! ironwood pool, `ironwood_spendable` nonzero) is deliberately absent:
//! this suite starts at NU6.2, and the bump is follow-on work.
//!
//! Requires `zingo-cli`, `zebrad`, and `zainod` in `TEST_BINARIES_DIR`
//! or on `PATH` — the container-test flow provides all three, building
//! zingo-cli from this workspace.

#![forbid(unsafe_code)]

use zcash_local_net::LocalNet;
use zcash_local_net::indexer::zainod::{Zainod, ZainodConfig};
use zcash_local_net::validator::ValidatorConfig as _;
use zcash_local_net::validator::zebrad::{Zebrad, ZebradConfig};
use zcash_local_net::wallet::{Wallet, WalletBalance};
use zingo_cli_harness::{ZingoCli, ZingoCliConfig};

/// Per-block miner reward in zats once the default regtest fixture's
/// post-NU6 funding stream (1% to `Deferred`, active from height 2)
/// starts deducting from the 6.25 ZEC subsidy.
const POST_NU6_MINER_REWARD: u64 = 618_750_000;
/// Block 1 predates the funding stream: full subsidy, mined to the
/// sapling receiver of the unified miner address (NU5 activates at
/// height 2, so block 1's coinbase cannot be orchard).
const BLOCK_1_SAPLING_REWARD: u64 = 625_000_000;

const SEND_VALUE: u64 = 250_000;
const SHIELD_FUNDING_VALUE: u64 = 2_000_000;

/// zingo-cli builds its wallet with a hard-coded three-confirmation
/// spendability policy, so every funding event is buried under this
/// many blocks before the scenario spends it.
const MIN_CONFIRMATIONS: u32 = 3;

/// The NU6.2 chain shape: pre-NU5 upgrades at 1, NU5 through NU6.2 at
/// 2, NU6.3 unconfigured (it never activates, and the harness-written
/// `--activation-heights` TOML omits its key — which is also what the
/// NU6.2 zingo-cli schema accepts). All configured upgrades are active
/// by height 2 because zebra's shielded-coinbase block templates fail
/// their own orchard-proof verification while a configured upgrade is
/// still in the future.
fn nu6_2_regtest_heights() -> zcash_local_net::protocol::ActivationHeights {
    zcash_local_net::protocol::ActivationHeights::builder()
        .set_overwinter(Some(1))
        .set_sapling(Some(1))
        .set_blossom(Some(1))
        .set_heartwood(Some(1))
        .set_canopy(Some(1))
        .set_nu5(Some(2))
        .set_nu6(Some(2))
        .set_nu6_1(Some(2))
        .set_nu6_2(Some(2))
        .set_nu6_3(None)
        .set_nu7(None)
        .build()
}

/// An orchard-mining zebrad + zainod stack on the NU6.2 shape: every
/// post-height-1 coinbase lands in a pool the abandon-art wallet can
/// spend without transparent coinbase maturity.
async fn launch_orchard_net() -> LocalNet<Zebrad, Zainod> {
    let mut validator_config = ZebradConfig::default();
    validator_config.set_test_parameters(
        zcash_local_net::protocol::MinerPool::Orchard,
        nu6_2_regtest_heights(),
        None,
    );
    LocalNet::<Zebrad, Zainod>::launch_from_two_configs(validator_config, ZainodConfig::default())
        .await
        .unwrap()
}

/// Sync the wallet until its view of the chain tip reaches
/// `target_height`. The validator reports a mined block immediately,
/// but the indexer serves the wallet on its own cadence — poll sync
/// rather than assume one pass suffices.
async fn sync_to_height(wallet: &ZingoCli, target_height: u32) -> WalletBalance {
    const ATTEMPTS: u32 = 120;
    for _ in 0..ATTEMPTS {
        wallet.sync().await.unwrap();
        let balance = wallet.balance().await.unwrap();
        if balance.chain_tip_height >= target_height {
            return balance;
        }
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }
    panic!("wallet did not reach height {target_height} after {ATTEMPTS} sync attempts");
}

/// Mine `blocks` and wait for the indexer to serve them, tracking the
/// expected tip.
async fn mine(net: &LocalNet<Zebrad, Zainod>, tip: &mut u32, blocks: u32) {
    net.generate_blocks_converged(blocks).await.unwrap();
    *tip += blocks;
}

/// The spec's proof scenario, one trait operation after another:
/// launch faucet → mine → sync → verify miner rewards → send to
/// recipient → shield transparent funds → rescan → validate balance
/// persistence.
#[tokio::test]
#[ignore = "binary e2e: requires zingo-cli, zebrad and zainod binaries (container-test flow)"]
async fn proof_scenario() {
    let net = launch_orchard_net().await;
    // Zebrad::launch mines one block — the launch block — to prove the
    // mining service (adjudicated by the observatory, commit ffdb126e8),
    // so a fresh validator is at height 1, and block 1's coinbase is the
    // sapling launch-block reward.
    let mut tip: u32 = 1;

    // launch: restore the faucet from the abandon-art mnemonic.
    let faucet: ZingoCli = net.launch_wallet(ZingoCliConfig::faucet).await.unwrap();

    // get_info: the wallet can reach and talk to its indexer.
    let info = faucet.get_info().await.unwrap();
    assert_eq!(info.chain_name, "regtest", "unexpected chain: {info:?}");
    assert!(!info.server_uri.is_empty(), "empty server_uri: {info:?}");

    // mine + sync: the faucet sees the miner rewards, mature enough to
    // spend under the three-confirmation policy.
    mine(&net, &mut tip, 1 + MIN_CONFIRMATIONS).await;
    let balance = sync_to_height(&faucet, tip).await;
    assert_eq!(
        balance.total,
        BLOCK_1_SAPLING_REWARD + u64::from(tip - 1) * POST_NU6_MINER_REWARD,
        "faucet should hold the full mined reward ladder: {balance:?}"
    );
    assert_eq!(balance.sapling_spendable, BLOCK_1_SAPLING_REWARD);
    assert_eq!(
        balance.orchard_spendable,
        u64::from(tip - 1) * POST_NU6_MINER_REWARD
    );
    assert_eq!(balance.ironwood_spendable, 0, "NU6.2 chain: {balance:?}");

    // send: faucet pays the recipient wallet's unified address.
    let recipient: ZingoCli = net.launch_wallet(ZingoCliConfig::recipient).await.unwrap();
    let recipient_address = recipient.default_address().await.unwrap();
    let send_txid = faucet.send(&recipient_address, SEND_VALUE).await.unwrap();
    assert_eq!(
        send_txid.len(),
        64,
        "txid should be 32 hex bytes: {send_txid}"
    );
    mine(&net, &mut tip, MIN_CONFIRMATIONS).await;
    let recipient_balance = sync_to_height(&recipient, tip).await;
    assert_eq!(
        recipient_balance.total, SEND_VALUE,
        "recipient should hold exactly the sent value: {recipient_balance:?}"
    );

    // shield: fund the faucet's own transparent receiver — non-coinbase
    // funds, so transparent-coinbase maturity
    // (zcash_protocol::consensus::COINBASE_MATURITY_BLOCKS) never
    // applies — then shield it.
    let faucet_transparent = faucet
        .address(zcash_local_net::wallet::AddressReceiver::Transparent)
        .await
        .unwrap();
    faucet
        .send(&faucet_transparent, SHIELD_FUNDING_VALUE)
        .await
        .unwrap();
    mine(&net, &mut tip, MIN_CONFIRMATIONS).await;
    let funded = sync_to_height(&faucet, tip).await;
    assert_eq!(
        funded.transparent_spendable, SHIELD_FUNDING_VALUE,
        "transparent receiver should be funded: {funded:?}"
    );
    let shield_txid = faucet.shield().await.unwrap();
    assert_eq!(
        shield_txid.len(),
        64,
        "txid should be 32 hex bytes: {shield_txid}"
    );
    mine(&net, &mut tip, MIN_CONFIRMATIONS).await;
    let shielded = sync_to_height(&faucet, tip).await;
    assert_eq!(
        shielded.transparent_spendable, 0,
        "shield should sweep the transparent funds: {shielded:?}"
    );
    assert!(
        shielded.orchard_spendable > funded.orchard_spendable,
        "shielded funds should land in orchard (the NU6.2 preferred pool): \
         before {funded:?}, after {shielded:?}"
    );
    assert_eq!(shielded.ironwood_spendable, 0, "NU6.2 chain: {shielded:?}");

    // rescan: wipe and rebuild from the birthday; the balance must
    // persist through it.
    recipient.rescan().await.unwrap();
    let rebuilt = sync_to_height(&recipient, tip).await;
    assert_eq!(
        rebuilt.total, recipient_balance.total,
        "recipient balance must survive a rescan: before {recipient_balance:?}, after {rebuilt:?}"
    );
}
