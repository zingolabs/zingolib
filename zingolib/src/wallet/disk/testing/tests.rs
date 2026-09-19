use bip0039::Mnemonic;

use zcash_keys::keys::Era;
use zcash_protocol::{PoolType, ShieldedPool};

use crate::{
    config::ClientConfig,
    lightclient::LightClient,
    wallet::{
        disk::testing::{
            assert_wallet_capability_matches_seed,
            examples::{
                AbandonAbandonVersion, AbsurdAmountVersion, ChimneyBetterVersion,
                HospitalMuseumVersion, HotelHumorVersion, MainnetSeedVersion, MobileShuffleVersion,
                NetworkSeedVersion, RegtestSeedVersion, TestnetSeedVersion, VillageTargetVersion,
            },
        },
        keys::unified::UnifiedKeyStore,
    },
};

// moving toward completeness: each of these tests should assert everything known about the LightWallet without network.

impl NetworkSeedVersion {
    /// this is enough data to restore wallet from! thus, it is the bronze test for backward compatibility
    async fn load_example_wallet_with_verification(&self) -> LightClient {
        let client = self.load_example_wallet().await;
        let wallet = client.wallet().read().await;

        assert_wallet_capability_matches_seed(&wallet, self.example_wallet_seed()).await;
        for pool in [
            PoolType::Transparent,
            PoolType::Shielded(ShieldedPool::Orchard),
        ] {
            assert_eq!(wallet.get_address(pool), self.example_wallet_address(pool));
        }
        drop(wallet);

        client
    }
}

#[tokio::test]
async fn verify_example_wallet_regtest_aaaaaaaaaaaaaaaaaaaaaaaa_v26() {
    NetworkSeedVersion::Regtest(RegtestSeedVersion::AbandonAbandon(
        AbandonAbandonVersion::V26,
    ))
    .load_example_wallet_with_verification()
    .await;
}
#[tokio::test]
async fn verify_example_wallet_regtest_aadaalacaadaalacaadaalac_orch_and_sapl() {
    NetworkSeedVersion::Regtest(RegtestSeedVersion::AbsurdAmount(
        AbsurdAmountVersion::OrchAndSapl,
    ))
    .load_example_wallet_with_verification()
    .await;
}
#[tokio::test]
async fn verify_example_wallet_regtest_aadaalacaadaalacaadaalac_orch_only() {
    NetworkSeedVersion::Regtest(RegtestSeedVersion::AbsurdAmount(
        AbsurdAmountVersion::OrchOnly,
    ))
    .load_example_wallet_with_verification()
    .await;
}
#[tokio::test]
async fn verify_example_wallet_regtest_hmvasmuvwmssvichcarbpoct_v27() {
    NetworkSeedVersion::Regtest(RegtestSeedVersion::HospitalMuseum(
        HospitalMuseumVersion::V27,
    ))
    .load_example_wallet_with_verification()
    .await;
}
/// unlike other, more basic tests, this test also checks number of addresses and balance
#[ignore = "FIXME pepper sync needs unified address discovery"]
#[tokio::test]
async fn verify_example_wallet_testnet_cbbhrwiilgbrababsshsmtpr_v26() {
    let client =
        NetworkSeedVersion::Testnet(TestnetSeedVersion::ChimneyBetter(ChimneyBetterVersion::V26))
            .load_example_wallet_with_verification()
            .await;

    loaded_wallet_assert(
        client,
        zingo_test_vectors::seeds::CHIMNEY_BETTER_SEED.to_string(),
        0,
        3,
    )
    .await;
}
/// unlike other, more basic tests, this test also checks number of addresses and balance
#[ignore = "test proves note has no index bug is a breaker"]
#[tokio::test]
async fn verify_example_wallet_testnet_cbbhrwiilgbrababsshsmtpr_v27() {
    let wallet =
        NetworkSeedVersion::Testnet(TestnetSeedVersion::ChimneyBetter(ChimneyBetterVersion::V27))
            .load_example_wallet_with_verification()
            .await;

    loaded_wallet_assert(
        wallet,
        zingo_test_vectors::seeds::CHIMNEY_BETTER_SEED.to_string(),
        10177826,
        1,
    )
    .await;
}
#[tokio::test]
async fn verify_example_wallet_testnet_cbbhrwiilgbrababsshsmtpr_v28() {
    NetworkSeedVersion::Testnet(TestnetSeedVersion::ChimneyBetter(ChimneyBetterVersion::V28))
        .load_example_wallet_with_verification()
        .await;
}
#[tokio::test]
async fn verify_example_wallet_testnet_cbbhrwiilgbrababsshsmtpr_g2f3830058() {
    NetworkSeedVersion::Testnet(TestnetSeedVersion::ChimneyBetter(
        ChimneyBetterVersion::Latest,
    ))
    .load_example_wallet_with_verification()
    .await;
}
#[tokio::test]
async fn verify_example_wallet_testnet_mskmgdbhotbpetcjwcspgopp_gab72a38b() {
    NetworkSeedVersion::Testnet(TestnetSeedVersion::MobileShuffle(
        MobileShuffleVersion::Gab72a38b,
    ))
    .load_example_wallet_with_verification()
    .await;
}
#[tokio::test]
async fn verify_example_wallet_testnet_mskmgdbhotbpetcjwcspgopp_g93738061a() {
    NetworkSeedVersion::Testnet(TestnetSeedVersion::MobileShuffle(
        MobileShuffleVersion::G93738061a,
    ))
    .load_example_wallet_with_verification()
    .await;
}
#[tokio::test]
async fn verify_example_wallet_testnet_mskmgdbhotbpetcjwcspgopp_ga74fed621() {
    NetworkSeedVersion::Testnet(TestnetSeedVersion::MobileShuffle(
        MobileShuffleVersion::Latest,
    ))
    .load_example_wallet_with_verification()
    .await;
}
#[tokio::test]
async fn verify_example_wallet_testnet_glorygoddess() {
    NetworkSeedVersion::Testnet(TestnetSeedVersion::GloryGoddess)
        .load_example_wallet_with_verification()
        .await;
}
#[tokio::test]
async fn verify_example_wallet_mainnet_vtfcorfbcbpctcfupmegmwbp_v28() {
    NetworkSeedVersion::Mainnet(MainnetSeedVersion::VillageTarget(VillageTargetVersion::V28))
        .load_example_wallet_with_verification()
        .await;
}
#[tokio::test]
async fn verify_example_wallet_mainnet_hhcclaltpcckcsslpcnetblr_gf0aaf9347() {
    NetworkSeedVersion::Mainnet(MainnetSeedVersion::HotelHumor(
        HotelHumorVersion::Gf0aaf9347,
    ))
    .load_example_wallet_with_verification()
    .await;
}
#[tokio::test]
async fn verify_example_wallet_mainnet_hhcclaltpcckcsslpcnetblr_latest() {
    NetworkSeedVersion::Mainnet(MainnetSeedVersion::HotelHumor(HotelHumorVersion::Latest))
        .load_example_wallet_with_verification()
        .await;
}

async fn loaded_wallet_assert(
    mut lightclient: LightClient,
    expected_seed_phrase: String,
    expected_balance: u64,
    expected_num_addresses: usize,
) {
    {
        let wallet = lightclient.wallet().read().await;
        assert_wallet_capability_matches_seed(&wallet, expected_seed_phrase).await;

        assert_eq!(wallet.unified_addresses.len(), expected_num_addresses);
        for addr in wallet.unified_addresses.values() {
            assert!(addr.orchard().is_some());
            assert!(addr.sapling().is_some());
            assert!(addr.transparent().is_some());
        }

        let balance = lightclient
            .account_balance(zip32::AccountId::ZERO)
            .await
            .unwrap();
        assert_eq!(
            balance.total_orchard_balance,
            Some(expected_balance.try_into().unwrap())
        );
    }
    if expected_balance > 0 {
        let sapling_address = crate::get_base_address_macro!(lightclient, "sapling");
        crate::testutils::lightclient::from_inputs::quick_send(
            &mut lightclient,
            vec![(&sapling_address, 11011, None)],
        )
        .await
        .unwrap();
        lightclient.sync_and_await().await.unwrap();
        let transparent_address = crate::get_base_address_macro!(lightclient, "transparent");
        crate::testutils::lightclient::from_inputs::quick_send(
            &mut lightclient,
            vec![(&transparent_address, 28000, None)],
        )
        .await
        .unwrap();
    }
}

// todo: proptest enum
#[tokio::test]
async fn reload_wallet_from_file() {
    use crate::wallet::{LightWallet, WalletConfig};
    use zingo_test_vectors::seeds::CHIMNEY_BETTER_SEED;

    let mut mid_client =
        NetworkSeedVersion::Testnet(TestnetSeedVersion::ChimneyBetter(ChimneyBetterVersion::V28))
            .load_example_wallet_with_verification()
            .await;
    let mid_client_network = mid_client.chain_type();

    mid_client.save_task().await;
    mid_client.wait_for_save().await;
    mid_client.shutdown_save_task().await.unwrap();

    let config = ClientConfig::builder()
        .set_indexer_uri(
            mid_client
                .indexer_uri()
                .expect("test client has an indexer"),
        )
        .set_chain_type(mid_client_network)
        .set_wallet_dir(mid_client.wallet_dir().unwrap())
        .set_wallet_config(WalletConfig::Read)
        .build()
        .unwrap();
    let loaded_client = LightClient::new(config, true).await.unwrap();
    let loaded_wallet = loaded_client.wallet().read().await;

    let expected_mnemonic = Mnemonic::from_phrase(CHIMNEY_BETTER_SEED.to_string()).unwrap();

    let expected_keys = UnifiedKeyStore::new_from_mnemonic(
        mid_client_network,
        &expected_mnemonic,
        zip32::AccountId::ZERO,
    )
    .unwrap();

    let UnifiedKeyStore::Spend(usk) = &loaded_wallet
        .unified_key_store
        .get(&zip32::AccountId::ZERO)
        .unwrap()
    else {
        panic!("should be spending key!")
    };
    let UnifiedKeyStore::Spend(expected_usk) = &expected_keys else {
        panic!("should be spending key!")
    };

    assert_eq!(
        usk.to_bytes(Era::Orchard),
        expected_usk.to_bytes(Era::Orchard)
    );
    assert_eq!(usk.orchard().to_bytes(), expected_usk.orchard().to_bytes());
    assert_eq!(usk.sapling().to_bytes(), expected_usk.sapling().to_bytes());
    assert_eq!(
        usk.transparent().to_bytes(),
        expected_usk.transparent().to_bytes()
    );

    // TODO: there were 3 UAs associated with this wallet, we reset to 1 to ensure index is upheld correctly and
    // should thoroughly test UA discovery when syncing which should find these UAs again
    assert_eq!(loaded_wallet.unified_addresses.len(), 1);
    for addr in loaded_wallet.unified_addresses.values() {
        assert!(addr.orchard().is_some());
        assert!(addr.sapling().is_none());
        assert!(addr.transparent().is_none());
    }

    let ufvk = usk.to_unified_full_viewing_key();
    let chain_type = loaded_client.chain_type();
    let ufvk_string = ufvk.encode(&chain_type);
    let wallet_config = WalletConfig::Ufvk {
        ufvk: ufvk_string.clone(),
        birthday: loaded_client.birthday(),
        wallet_settings: loaded_wallet.wallet_settings.clone(),
    };
    let view_wallet = LightWallet::new(chain_type, wallet_config).unwrap();
    let UnifiedKeyStore::View(v_ufvk) = &view_wallet
        .unified_key_store
        .get(&zip32::AccountId::ZERO)
        .unwrap()
    else {
        panic!("should be viewing key!");
    };
    let v_ufvk_string = v_ufvk.encode(&view_wallet.chain_type);
    assert_eq!(ufvk_string, v_ufvk_string);

    // NOTE: removed balance check as need to sync to restore transaction data.
}

/// A pre-42 wallet loads with no migration section, and a populated
/// [`crate::wallet::migration::MigrationState`] survives a full wallet
/// write/read round trip at the current serialized version.
#[tokio::test]
async fn wallet_round_trips_migration_state_at_current_version() {
    use crate::wallet::LightWallet;
    use crate::wallet::migration::{
        BoundNote, MigrationParams, MigrationPhase, MigrationState, PlanCommitment,
        SigningStrategy, TransferId, TransferRecord,
    };
    use pepper_sync::wallet::OutputId;
    use zcash_primitives::transaction::TxId;

    let client = NetworkSeedVersion::Regtest(RegtestSeedVersion::AbandonAbandon(
        AbandonAbandonVersion::V26,
    ))
    .load_example_wallet()
    .await;
    let mut wallet = client.wallet().write().await;
    assert!(wallet.migration.is_none(), "pre-42 wallet has no migration");

    let params = MigrationParams::provisional(wallet.chain_type());
    let mut part = TransferRecord::new(
        TransferId(0),
        100_000_000,
        BoundNote {
            output_id: OutputId::new(TxId::from_bytes([3; 32]), 1),
            nullifier: [4; 32],
            commitment: [5; 32],
        },
    );
    part.assign(12).unwrap();
    let state = MigrationState {
        commitment: PlanCommitment {
            params_hash: params.params_hash(),
            plan_hash: [6; 32],
            committed_at: 1_782_000_000,
        },
        params,
        strategy: SigningStrategy::LazyAtBoundary,
        mode: crate::wallet::migration::MigrationMode::Scheduled,
        account: zip32::AccountId::ZERO,
        phase: MigrationPhase::Scheduled,
        transfers: vec![part],
    };
    wallet.migration = Some(state.clone());
    wallet.save_required = true;

    let bytes = wallet.save().unwrap().expect("save required");
    let recovered = LightWallet::read(bytes.as_slice(), wallet.chain_type()).unwrap();
    assert_eq!(
        recovered.current_version(),
        LightWallet::serialized_version()
    );
    assert_eq!(recovered.migration, Some(state));
}

/// What the orphaned-layout tests below assert against: the wallet's chain
/// type, its recovery info, the serialized tail (price list plus optional
/// migration section), and the whole file saved at the current version.
///
/// The tail is returned in full, not merely as a length, because asserting
/// on it is what makes these tests meaningful. Recovery info comes from the
/// file *prefix* and so survives even a badly misparsed tail. Only comparing
/// the tail proves the disambiguating reader chose the right layout.
struct CurrentVersionWallet {
    chain_type: crate::config::ChainType,
    recovery_info: crate::wallet::RecoveryInfo,
    tail: Vec<u8>,
    bytes: Vec<u8>,
}

impl CurrentVersionWallet {
    /// Offset at which the pre-release layout's `allow_v6_transactions`
    /// byte sits, immediately before the price list.
    fn tail_offset(&self) -> usize {
        self.bytes.len() - self.tail.len()
    }

    /// Serializes the tail of `wallet` for comparison against this
    /// expectation. `PriceList` has no `PartialEq`, so the round trip is
    /// checked through the encoding, which is the property under test.
    fn assert_tail_matches(&self, wallet: &crate::wallet::LightWallet, context: &str) {
        use zcash_encoding::Optional;

        let mut recovered_tail = Vec::new();
        wallet.price_list.write(&mut recovered_tail).unwrap();
        Optional::write(
            &mut recovered_tail,
            wallet.migration.as_ref(),
            crate::wallet::migration::store::write,
        )
        .unwrap();

        assert_eq!(
            recovered_tail, self.tail,
            "{context}: the price list and migration section must survive the read"
        );
    }
}

/// Saves an example wallet carrying a deliberately non-trivial tail: a price
/// list with a start time and a populated migration state. A tail of all
/// zeroes would be misparsed into an identical all-`None` value, so it could
/// not detect a reader that picked the wrong layout.
async fn current_version_wallet_bytes() -> CurrentVersionWallet {
    use crate::wallet::migration::{
        BoundNote, MigrationParams, MigrationPhase, MigrationState, PlanCommitment,
        SigningStrategy, TransferId, TransferRecord,
    };
    use pepper_sync::wallet::OutputId;
    use zcash_encoding::Optional;
    use zcash_primitives::transaction::TxId;

    let client = NetworkSeedVersion::Regtest(RegtestSeedVersion::AbandonAbandon(
        AbandonAbandonVersion::V26,
    ))
    .load_example_wallet()
    .await;
    let mut wallet = client.wallet().write().await;

    wallet.price_list.set_start_time(1_782_000_000);

    let params = MigrationParams::provisional(wallet.chain_type());
    let mut part = TransferRecord::new(
        TransferId(0),
        100_000_000,
        BoundNote {
            output_id: OutputId::new(TxId::from_bytes([3; 32]), 1),
            nullifier: [4; 32],
            commitment: [5; 32],
        },
    );
    part.assign(12).unwrap();
    wallet.migration = Some(MigrationState {
        commitment: PlanCommitment {
            params_hash: params.params_hash(),
            plan_hash: [6; 32],
            committed_at: 1_782_000_000,
        },
        params,
        strategy: SigningStrategy::LazyAtBoundary,
        mode: crate::wallet::migration::MigrationMode::Scheduled,
        account: zip32::AccountId::ZERO,
        phase: MigrationPhase::Scheduled,
        transfers: vec![part],
    });

    wallet.save_required = true;
    let bytes = wallet.save().unwrap().expect("save required");

    let mut tail = Vec::new();
    wallet.price_list.write(&mut tail).unwrap();
    Optional::write(&mut tail, wallet.migration.as_ref(), |w, migration| {
        crate::wallet::migration::store::write(w, migration)
    })
    .unwrap();

    CurrentVersionWallet {
        chain_type: wallet.chain_type(),
        recovery_info: wallet.recovery_info().unwrap(),
        tail,
        bytes,
    }
}

/// Pre-release ironwood builds briefly wrote version 43 with the final
/// version 42 layout. Those files must load as if they were version 42.
#[tokio::test]
async fn wallet_reads_retired_version_43_as_current() {
    use crate::wallet::LightWallet;

    let expected = current_version_wallet_bytes().await;
    let mut bytes = expected.bytes.clone();
    bytes[..8].copy_from_slice(&43u64.to_le_bytes());

    let recovered = LightWallet::read(bytes.as_slice(), expected.chain_type)
        .expect("retired version 43 must read as the final 42 layout");
    assert_eq!(recovered.recovery_info().unwrap(), expected.recovery_info);
    expected.assert_tail_matches(&recovered, "version 43");
}

/// Pre-release ironwood builds before the `allow_v6_transactions` removal
/// wrote version 42 with an extra bool between `min_confirmations` and the
/// price list. Both bool values must load under the disambiguating reader.
#[tokio::test]
async fn wallet_reads_pre_release_v42_with_allow_v6_byte() {
    use crate::wallet::LightWallet;

    let expected = current_version_wallet_bytes().await;

    for allow_v6_byte in [0u8, 1u8] {
        let mut pre_release = expected.bytes.clone();
        pre_release.insert(expected.tail_offset(), allow_v6_byte);

        let recovered = LightWallet::read(pre_release.as_slice(), expected.chain_type)
            .expect("pre-release v42 layout must read via tail disambiguation");
        assert_eq!(recovered.recovery_info().unwrap(), expected.recovery_info);
        expected.assert_tail_matches(
            &recovered,
            &format!("pre-release version 42 with allow_v6_transactions={allow_v6_byte}"),
        );
    }
}

/// A file whose tail reads cleanly under *both* layouts is genuinely
/// ambiguous, and the reader must refuse it rather than silently substitute
/// one reading's price list and migration state for the other's.
///
/// The decision is exercised directly: constructing a byte string that
/// satisfies both parses is not possible for a migration-free tail (see the
/// parity argument on `resolve_v42_tail`) and impractical otherwise, so the
/// test supplies the four parse outcomes the caller can hand it.
#[test]
fn ambiguous_version_42_tail_is_refused() {
    use crate::wallet::LightWallet;
    use zingo_price::PriceList;

    let parsed = || Ok((PriceList::new(), None));
    let failed = || {
        Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "tail did not parse",
        ))
    };

    let ambiguous = LightWallet::resolve_v42_tail(parsed(), Some(parsed()));
    assert_eq!(
        ambiguous.unwrap_err().kind(),
        std::io::ErrorKind::InvalidData,
        "a tail parsing both ways must be refused, not guessed"
    );

    assert!(
        LightWallet::resolve_v42_tail(parsed(), Some(failed())).is_ok(),
        "the canonical layout must win when only it parses"
    );
    assert!(
        LightWallet::resolve_v42_tail(parsed(), None).is_ok(),
        "a leading byte that is no bool rules out the pre-release layout"
    );
    assert!(
        LightWallet::resolve_v42_tail(failed(), Some(parsed())).is_ok(),
        "the pre-release layout must be accepted when only it parses"
    );
    assert!(
        LightWallet::resolve_v42_tail(failed(), Some(failed())).is_err(),
        "a tail parsing neither way must fail"
    );
}

/// When the full parse fails (here simulated by a truncated file) the
/// prefix-only salvage reader must still recover seed, birthday, and
/// account count, so no format break can strand a wallet.
#[tokio::test]
async fn recovery_info_salvages_a_wallet_file_that_fails_to_read() {
    use crate::wallet::LightWallet;

    let expected = current_version_wallet_bytes().await;
    let mut bytes = expected.bytes.clone();
    bytes.truncate(bytes.len() - 10);

    assert!(
        LightWallet::read(bytes.as_slice(), expected.chain_type).is_err(),
        "truncated file must fail the full parse for this test to be meaningful"
    );
    let salvaged = LightWallet::read_recovery_info(bytes.as_slice())
        .expect("prefix salvage must survive a corrupt tail");
    assert_eq!(salvaged, expected.recovery_info);
}

/// Sweeps the local corpus of real wallet files in `data_wallets/` at the
/// workspace root: every file must either fully parse or yield recovery
/// info from the prefix salvage, so no corpus wallet is stranded.
///
/// The corpus holds live seed material, so it is gitignored and this test
/// never prints seeds. It reports only per-file outcomes. On machines
/// without the corpus (CI included) the sweep is an empty pass.
#[test]
fn data_wallets_corpus_parses_or_salvages() {
    use crate::config::ChainType;
    use crate::wallet::LightWallet;

    let corpus_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("zingolib lives one level below the workspace root")
        .join("data_wallets");
    if !corpus_dir.is_dir() {
        eprintln!("no data_wallets corpus on this machine; nothing to sweep");
        return;
    }

    let mut corpus_paths = std::fs::read_dir(&corpus_dir)
        .expect("corpus directory must be listable")
        .map(|entry| {
            entry
                .expect("corpus directory entries must be readable")
                .path()
        })
        .filter(|path| path.extension().is_some_and(|extension| extension == "dat"))
        .collect::<Vec<_>>();
    corpus_paths.sort();
    assert!(
        !corpus_paths.is_empty(),
        "data_wallets exists but holds no .dat files; nothing swept"
    );

    for path in corpus_paths {
        let file_name = path.file_name().unwrap().to_string_lossy().into_owned();
        let bytes = std::fs::read(&path).expect("corpus files must be readable");

        // Device wallets are mainnet or testnet; regtest wallets cannot
        // occur in the corpus because their activation heights are not
        // recoverable from the file alone.
        let full_parse = LightWallet::read(bytes.as_slice(), ChainType::Mainnet)
            .or_else(|_| LightWallet::read(bytes.as_slice(), ChainType::Testnet));

        match full_parse {
            Ok(wallet) => {
                eprintln!(
                    "{file_name}: parsed fully (stored version {})",
                    wallet.read_version()
                );
            }
            Err(read_error) => {
                let salvaged = LightWallet::read_recovery_info(bytes.as_slice()).unwrap_or_else(
                    |salvage_error| {
                        panic!(
                            "{file_name}: full parse failed ({read_error}) and prefix \
                                 salvage also failed ({salvage_error})"
                        )
                    },
                );
                assert!(
                    !salvaged.seed_phrase.is_empty(),
                    "{file_name}: salvage produced an empty seed phrase"
                );
                eprintln!(
                    "{file_name}: full parse failed ({read_error}); salvaged recovery info \
                     (birthday {}, {} accounts)",
                    salvaged.birthday, salvaged.no_of_accounts
                );
            }
        }
    }
}

mod validation {
    use proptest::prelude::*;

    use zingo_common_components::protocol::ActivationHeights;

    use pepper_sync::keys::transparent::TransparentScope;

    use crate::config::ChainType;
    use crate::wallet::{LightWallet, utils};

    use super::current_version_wallet_bytes;
    use super::{
        AbandonAbandonVersion, AbsurdAmountVersion, ChimneyBetterVersion, HospitalMuseumVersion,
        HotelHumorVersion, MainnetSeedVersion, MobileShuffleVersion, NetworkSeedVersion,
        RegtestSeedVersion, TestnetSeedVersion, VillageTargetVersion,
    };

    /// [`LightWallet::validate`] accepts every checked-in example wallet file.
    #[test]
    fn validate_accepts_every_example_wallet_fixture() {
        let regtest = ChainType::Regtest(ActivationHeights::default());
        let fixtures = [
            (
                NetworkSeedVersion::Regtest(RegtestSeedVersion::HospitalMuseum(
                    HospitalMuseumVersion::V27,
                )),
                regtest,
            ),
            (
                NetworkSeedVersion::Regtest(RegtestSeedVersion::HospitalMuseum(
                    HospitalMuseumVersion::V42Migration,
                )),
                NetworkSeedVersion::Regtest(RegtestSeedVersion::HospitalMuseum(
                    HospitalMuseumVersion::V42Migration,
                ))
                .chain_type(),
            ),
            (
                NetworkSeedVersion::Regtest(RegtestSeedVersion::AbandonAbandon(
                    AbandonAbandonVersion::V26,
                )),
                regtest,
            ),
            (
                NetworkSeedVersion::Regtest(RegtestSeedVersion::AbsurdAmount(
                    AbsurdAmountVersion::OrchAndSapl,
                )),
                regtest,
            ),
            (
                NetworkSeedVersion::Regtest(RegtestSeedVersion::AbsurdAmount(
                    AbsurdAmountVersion::OrchOnly,
                )),
                regtest,
            ),
            (
                NetworkSeedVersion::Testnet(TestnetSeedVersion::ChimneyBetter(
                    ChimneyBetterVersion::V26,
                )),
                ChainType::Testnet,
            ),
            (
                NetworkSeedVersion::Testnet(TestnetSeedVersion::ChimneyBetter(
                    ChimneyBetterVersion::V27,
                )),
                ChainType::Testnet,
            ),
            (
                NetworkSeedVersion::Testnet(TestnetSeedVersion::ChimneyBetter(
                    ChimneyBetterVersion::V28,
                )),
                ChainType::Testnet,
            ),
            (
                NetworkSeedVersion::Testnet(TestnetSeedVersion::ChimneyBetter(
                    ChimneyBetterVersion::Latest,
                )),
                ChainType::Testnet,
            ),
            (
                NetworkSeedVersion::Testnet(TestnetSeedVersion::MobileShuffle(
                    MobileShuffleVersion::Gab72a38b,
                )),
                ChainType::Testnet,
            ),
            (
                NetworkSeedVersion::Testnet(TestnetSeedVersion::MobileShuffle(
                    MobileShuffleVersion::G93738061a,
                )),
                ChainType::Testnet,
            ),
            (
                NetworkSeedVersion::Testnet(TestnetSeedVersion::MobileShuffle(
                    MobileShuffleVersion::Latest,
                )),
                ChainType::Testnet,
            ),
            (
                NetworkSeedVersion::Testnet(TestnetSeedVersion::GloryGoddess),
                ChainType::Testnet,
            ),
            (
                NetworkSeedVersion::Mainnet(MainnetSeedVersion::VillageTarget(
                    VillageTargetVersion::V28,
                )),
                ChainType::Mainnet,
            ),
            (
                NetworkSeedVersion::Mainnet(MainnetSeedVersion::HotelHumor(
                    HotelHumorVersion::Gf0aaf9347,
                )),
                ChainType::Mainnet,
            ),
            (
                NetworkSeedVersion::Mainnet(MainnetSeedVersion::HotelHumor(
                    HotelHumorVersion::Latest,
                )),
                ChainType::Mainnet,
            ),
        ];

        for (fixture, chain_type) in fixtures {
            let path = fixture.example_wallet_path();
            let bytes = std::fs::read(&path).expect("example wallet files are checked in");
            LightWallet::validate(bytes.as_slice(), chain_type)
                .unwrap_or_else(|error| panic!("{} must validate: {error}", path.display()));
        }
    }

    /// [`LightWallet::validate`] accepts the output of `write` untouched and
    /// rejects it truncated to every shorter length, a superset of every
    /// field boundary and every position one byte past one.
    #[tokio::test]
    async fn validate_accepts_write_output_and_rejects_every_truncation() {
        let expected = current_version_wallet_bytes().await;

        LightWallet::validate(expected.bytes.as_slice(), expected.chain_type)
            .expect("the untruncated output of write must validate");

        for length in 0..expected.bytes.len() {
            assert!(
                LightWallet::validate(&expected.bytes[..length], expected.chain_type).is_err(),
                "the file truncated to {length} of {} bytes must be rejected",
                expected.bytes.len()
            );
        }
    }

    /// [`LightWallet::read_recovery_info`] must
    /// still read the prefix of a file stamped with a future version.
    #[tokio::test]
    async fn recovery_info_salvages_versions_above_the_current_write_version() {
        let expected = current_version_wallet_bytes().await;

        for future_version in [
            LightWallet::serialized_version() + 1,
            LightWallet::serialized_version() + 2,
        ] {
            let mut bytes = expected.bytes.clone();
            bytes[..8].copy_from_slice(&future_version.to_le_bytes());

            let salvaged =
                LightWallet::read_recovery_info(bytes.as_slice()).unwrap_or_else(|error| {
                    panic!("version {future_version} must remain salvageable: {error}")
                });
            assert_eq!(salvaged, expected.recovery_info);
        }
    }

    /// Regression case: invalid seed phrase bytes must not recover to a seed phrase.
    #[test]
    fn recovery_info_rejects_forty_seven_space_bytes() {
        let error = LightWallet::read_recovery_info(vec![0x20; 47].as_slice())
            .expect_err("uniform filler bytes must not decode to a seed phrase");
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
        assert!(
            error
                .to_string()
                .contains(&0x2020202020202020u64.to_string()),
            "the error must name the rejected version: {error}"
        );
    }

    /// A well-formed recovery prefix is accepted, and the same prefix with an invalid birthday is rejected.
    #[test]
    fn recovery_info_rejects_an_implausible_birthday() {
        let prefix = |birthday: u32| {
            let mut bytes = LightWallet::serialized_version().to_le_bytes().to_vec();
            bytes.push(0);
            bytes.push(32);
            bytes.extend_from_slice(&[0x55; 32]);
            bytes.extend_from_slice(&birthday.to_le_bytes());
            bytes.push(1);
            bytes
        };

        let info = LightWallet::read_recovery_info(prefix(2_000_000).as_slice()).unwrap();
        assert_eq!(info.birthday, 2_000_000);

        let error = LightWallet::read_recovery_info(prefix(600_000_000).as_slice())
            .expect_err("a birthday in the hundreds of millions must be rejected");
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
    }

    /// The lowest u32 with the ZIP 32 hardened-derivation bit set, valid neither as an account id nor as a non-hardened child index.
    const FIRST_HARDENED_INDEX: u32 = 1 << 31;

    /// The chain tag byte the current wallet version stores for mainnet.
    const MAINNET_CHAIN_TAG: u8 = 0;

    /// The filler byte the crafted fixtures use as seed entropy.
    const SEED_ENTROPY_FILL: u8 = 0x55;

    /// The oldest wallet version that stores the unified key store as a vector of account entries.
    const FIRST_VECTORED_KEY_STORE_VERSION: u64 = 35;

    /// Builds a current-version wallet prefix through the birthday field, ready for a crafted tail.
    fn current_version_prefix_through_birthday() -> Vec<u8> {
        let mut bytes = LightWallet::serialized_version().to_le_bytes().to_vec();
        bytes.push(MAINNET_CHAIN_TAG);
        let seed_entropy = [SEED_ENTROPY_FILL; 32];
        bytes.push(seed_entropy.len() as u8);
        bytes.extend_from_slice(&seed_entropy);
        bytes.extend_from_slice(&0u32.to_le_bytes());
        bytes
    }

    /// [`LightWallet::validate`] returns an error, rather than panicking, on a key store entry whose account id has the hardened bit set.
    #[test]
    fn validate_rejects_a_hardened_key_store_account_id() {
        let mut bytes = current_version_prefix_through_birthday();
        bytes.push(1);
        bytes.extend_from_slice(&FIRST_HARDENED_INDEX.to_le_bytes());

        let error = LightWallet::validate(bytes.as_slice(), ChainType::Mainnet)
            .expect_err("a hardened account id must be rejected, not panicked on");
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
        assert!(
            error
                .to_string()
                .contains(&FIRST_HARDENED_INDEX.to_string()),
            "the error must name the rejected account id: {error}"
        );
    }

    /// [`LightWallet::validate`] returns an error, rather than panicking, on a unified address whose account id has the hardened bit set.
    #[test]
    fn validate_rejects_a_hardened_unified_address_account_id() {
        let mut bytes = current_version_prefix_through_birthday();
        bytes.push(0);
        bytes.push(1);
        bytes.extend_from_slice(&FIRST_HARDENED_INDEX.to_le_bytes());

        let error = LightWallet::validate(bytes.as_slice(), ChainType::Mainnet)
            .expect_err("a hardened account id must be rejected, not panicked on");
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
    }

    /// [`LightWallet::validate`] returns an error, rather than panicking, on a transparent address whose account id has the hardened bit set.
    #[test]
    fn validate_rejects_a_hardened_transparent_address_account_id() {
        let mut bytes = current_version_prefix_through_birthday();
        bytes.push(0);
        bytes.push(0);
        bytes.push(1);
        bytes.extend_from_slice(&FIRST_HARDENED_INDEX.to_le_bytes());

        let error = LightWallet::validate(bytes.as_slice(), ChainType::Mainnet)
            .expect_err("a hardened account id must be rejected, not panicked on");
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
    }

    /// [`LightWallet::validate`] returns an error, rather than panicking, on a transparent address index with the hardened bit set.
    #[test]
    fn validate_rejects_a_hardened_transparent_address_index() {
        let mut bytes = current_version_prefix_through_birthday();
        bytes.push(0);
        bytes.push(0);
        bytes.push(1);
        bytes.extend_from_slice(&0u32.to_le_bytes());
        bytes.push(TransparentScope::External as u8);
        bytes.extend_from_slice(&FIRST_HARDENED_INDEX.to_le_bytes());

        let error = LightWallet::validate(bytes.as_slice(), ChainType::Mainnet)
            .expect_err("a hardened address index must be rejected, not panicked on");
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
    }

    /// [`LightWallet::validate`] returns an error, rather than panicking, on a version 35 wallet whose key store vector holds no account 0.
    #[test]
    fn validate_rejects_an_account_zero_free_version_thirty_five_wallet() {
        let mut bytes = FIRST_VECTORED_KEY_STORE_VERSION.to_le_bytes().to_vec();
        utils::write_string(&mut bytes, &"main".to_string())
            .expect("writing to a vector cannot fail");
        bytes.push(0);
        bytes.extend_from_slice(&0u32.to_le_bytes());
        bytes.push(0);
        bytes.push(0);
        bytes.push(0);

        let error = LightWallet::validate(bytes.as_slice(), ChainType::Mainnet)
            .expect_err("a wallet with no account 0 key must be rejected, not panicked on");
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
    }

    proptest! {
        /// Any byte string whose version word falls outside the accepted
        /// range is rejected by both functions.
        #[test]
        fn out_of_range_version_words_are_rejected(
            bytes in proptest::collection::vec(any::<u8>(), 8..512)
        ) {
            let version = u64::from_le_bytes(bytes[..8].try_into().unwrap());

            if version > 43 {
                prop_assert!(
                    LightWallet::validate(bytes.as_slice(), ChainType::Mainnet).is_err()
                );
            }
            if !(32..=LightWallet::MAX_RECOVERABLE_VERSION).contains(&version) {
                prop_assert!(LightWallet::read_recovery_info(bytes.as_slice()).is_err());
            }
        }
    }
}

mod version_forty {
    use bip0039::Mnemonic;

    use crate::config::ChainType;
    use crate::wallet::{LightWallet, utils};

    fn dev_v40_prefix() -> Vec<u8> {
        let mut bytes = 40u64.to_le_bytes().to_vec();
        bytes.push(0);
        bytes.push(32);
        bytes.extend_from_slice(&[0x55; 32]);
        bytes
    }

    #[test]
    fn dev_v40_truncated_file_errs_instead_of_aborting() {
        let error = LightWallet::read(dev_v40_prefix().as_slice(), ChainType::Mainnet)
            .expect_err("the prefix carries no body");
        assert_eq!(error.kind(), std::io::ErrorKind::UnexpectedEof);
    }

    #[test]
    fn dev_v40_chain_tag_mismatch_is_reported() {
        let mut bytes = 40u64.to_le_bytes().to_vec();
        bytes.push(1);
        let error = LightWallet::read(bytes.as_slice(), ChainType::Mainnet)
            .expect_err("a testnet tag must refuse a mainnet load");
        assert!(
            error
                .to_string()
                .contains("wallet chain name testnet doesn't match expected mainnet"),
            "{error}"
        );
    }

    #[test]
    fn stable_v40_chain_string_reads_as_before() {
        let mut bytes = 40u64.to_le_bytes().to_vec();
        utils::write_string(&mut bytes, &"test".to_string()).unwrap();
        let error = LightWallet::read(bytes.as_slice(), ChainType::Mainnet)
            .expect_err("a testnet name must refuse a mainnet load");
        assert!(
            error
                .to_string()
                .contains("wallet chain name testnet doesn't match expected mainnet"),
            "{error}"
        );
    }

    #[test]
    fn dev_v40_recovery_info_reaches_the_seed() {
        let mut bytes = dev_v40_prefix();
        bytes.extend_from_slice(&100u32.to_le_bytes());
        bytes.push(1);
        let info = LightWallet::read_recovery_info(bytes.as_slice()).unwrap();
        assert_eq!(info.birthday, 100);
        assert_eq!(info.no_of_accounts, 1);
        assert_eq!(
            info.seed_phrase,
            <Mnemonic>::from_entropy([0x55; 32].to_vec())
                .unwrap()
                .phrase()
        );
    }
}

mod v42_migration_fixture {
    use pepper_sync::wallet::{NoteInterface as _, OrchardNote, OutputId, OutputInterface as _};
    use zcash_protocol::consensus::BlockHeight;
    use zingo_status::confirmation_status::ConfirmationStatus;

    use super::*;
    use crate::lightclient::migrate::TransferProgress;
    use crate::wallet::LightWallet;
    use crate::wallet::migration::{
        MigrationMode, MigrationPhase, SigningStrategy, TransferId, TransferState,
    };

    const FUNDING_TXID: &str = "9db7e76c6dde10cc7b4aab0686e7ad5c5d3be96a5a52d1cd90b8556250a1df6a";
    const PART_ZERO_TXID: &str = "74e319016d4a3643a1758785f0ad087f0e1104bc0f51957acb75c96ca4142545";
    const PART_ONE_TXID: &str = "bf188cbe62051004bc39b7b2acb20478913d3b15e731fe9e0a756204de51e35f";
    const PARAMS_HASH: &str = "c4a51c771da9edb76b5199640f4c46c3a4e77a6e8fb15ec37865e13553a9cdac";
    const PLAN_HASH: &str = "f9e75428dfb8f3ec9550dd68b23a39ada98aa8c8e73cfab6e337aa1c4c5b42d8";
    const COMMITTED_AT: u64 = 1_788_990_210;
    const TIP: u32 = 460;
    const FUNDING_HEIGHT: u32 = 4;
    const PART_ZERO_CONFIRMED_AT: u32 = 356;
    const PART_ONE_BUILT_AT: u32 = 433;
    const PART_FEE: u64 = 20_000;
    const CANONICAL_EXPIRY: u32 = 69_120;

    fn fixture() -> NetworkSeedVersion {
        NetworkSeedVersion::Regtest(RegtestSeedVersion::HospitalMuseum(
            HospitalMuseumVersion::V42Migration,
        ))
    }

    fn funding_output(output_index: u32) -> OutputId {
        OutputId::new(
            crate::utils::conversion::txid_from_hex_encoded_str(FUNDING_TXID)
                .expect("the pinned funding txid decodes"),
            output_index,
        )
    }

    #[tokio::test]
    async fn the_loaded_migration_answers_the_new_client_api() {
        let client = fixture().load_example_wallet().await;
        let status = client
            .migration_status()
            .await
            .expect("the loaded migration reports its status");
        assert_eq!(status.phase, Some(MigrationPhase::Scheduled));
        let progress: Vec<(TransferId, TransferProgress, u32)> = status
            .transfers
            .iter()
            .map(|transfer| (transfer.id, transfer.progress, transfer.missed_windows))
            .collect();
        assert_eq!(
            progress,
            vec![
                (TransferId(0), TransferProgress::Confirmed, 0),
                (TransferId(1), TransferProgress::Broadcast, 0),
                (TransferId(2), TransferProgress::Pending, 0),
            ],
            "every pinned transfer reports its progress under the new status API"
        );
        assert_eq!(status.transfers_total, 3);
        assert_eq!(status.transfers_confirmed, 1);
        assert_eq!(status.value_total, 5_000_000 + 2_000_000 + 1_000_000);
        assert_eq!(status.value_migrated, 5_000_000);
        assert!(
            status
                .transfers
                .iter()
                .all(|transfer| transfer.window.is_some()),
            "every pinned transfer is scheduled into a window"
        );
    }

    #[tokio::test]
    async fn the_loaded_migration_reserves_the_funding_notes_of_its_pending_transfers() {
        let client = fixture().load_example_wallet().await;
        let wallet = client.wallet().read().await;
        let mut reserved = wallet.reserved_output_ids();
        reserved.sort();
        assert_eq!(
            reserved,
            vec![funding_output(1), funding_output(2)],
            "the broadcast and the assigned transfers keep their funding notes reserved; the \
             confirmed transfer's note is spent and no longer reserved"
        );
        assert_eq!(
            wallet.reserved_orchard_value(),
            (2_000_000 + PART_FEE) + (1_000_000 + PART_FEE)
        );
    }

    #[tokio::test]
    async fn writing_the_loaded_wallet_upgrades_its_migration_section_to_version_5() {
        let client = fixture().load_example_wallet().await;
        let mut wallet = client.wallet().write().await;
        let chain_type = wallet.chain_type();
        let loaded = wallet
            .migration()
            .expect("the fixture carries a migration")
            .clone();

        let mut section = Vec::new();
        crate::wallet::migration::store::write(&mut section, &loaded)
            .expect("the loaded state writes");
        assert_eq!(section[0], 5, "the section is rewritten at version 5");

        let mut bytes = Vec::new();
        wallet
            .write(&mut bytes, &chain_type)
            .expect("the loaded wallet writes");
        assert!(
            bytes.ends_with(&section),
            "the migration section is the wallet file's tail"
        );

        let recovered =
            LightWallet::read(bytes.as_slice(), chain_type).expect("the rewritten wallet reads");
        assert_eq!(
            recovered.current_version(),
            LightWallet::serialized_version()
        );
        assert_eq!(
            recovered.migration(),
            Some(&loaded),
            "the round trip after the upgrade preserves the loaded state"
        );
        assert_eq!(
            recovered.reserved_output_ids(),
            wallet.reserved_output_ids()
        );
    }

    #[tokio::test]
    async fn the_pinned_file_carries_a_version_4_section() {
        let client = fixture().load_example_wallet().await;
        let wallet = client.wallet().read().await;
        let loaded = wallet
            .migration()
            .expect("the fixture carries a migration")
            .clone();

        let section_with = |count: usize| {
            let mut truncated = loaded.clone();
            truncated.transfers = loaded.transfers()[..count].to_vec();
            let mut section = Vec::new();
            crate::wallet::migration::store::write(&mut section, &truncated)
                .expect("the truncated state writes");
            section
        };
        let mut as_version_4 = section_with(loaded.transfers().len());
        as_version_4[0] = 4;
        for count in (1..=loaded.transfers().len()).rev() {
            let transfer_end = section_with(count).len() - 1;
            assert_eq!(
                &as_version_4[transfer_end - 5..transfer_end],
                &[0, 0, 0, 0, 0],
                "a transfer read from version 4 writes an empty history"
            );
            as_version_4.drain(transfer_end - 5..transfer_end);
        }

        let pinned =
            std::fs::read(fixture().example_wallet_path()).expect("the pinned wallet file reads");
        assert!(
            pinned.ends_with(&as_version_4),
            "the pinned file ends with the version-4 encoding of the loaded state"
        );
    }

    struct PinnedPart {
        id: u32,
        denomination: u64,
        output_index: u32,
        bucket: u64,
        anchor_bucket: u64,
        target: u32,
        state: TransferState,
        txid: Option<&'static str>,
        expiry: Option<u32>,
        attempts: u8,
        witness_position: u64,
    }

    #[tokio::test]
    async fn verify_example_wallet_regtest_hmvasmuvwmssvichcarbpoct_v42_migration() {
        let pinned = [
            PinnedPart {
                id: 0,
                denomination: 5_000_000,
                output_index: 0,
                bucket: 2,
                anchor_bucket: 1,
                target: 355,
                state: TransferState::Confirmed {
                    height: BlockHeight::from_u32(PART_ZERO_CONFIRMED_AT),
                },
                txid: Some(PART_ZERO_TXID),
                expiry: Some(CANONICAL_EXPIRY),
                attempts: 1,
                witness_position: 0,
            },
            PinnedPart {
                id: 1,
                denomination: 2_000_000,
                output_index: 1,
                bucket: 3,
                anchor_bucket: 2,
                target: 460,
                state: TransferState::Broadcast,
                txid: Some(PART_ONE_TXID),
                expiry: Some(CANONICAL_EXPIRY),
                attempts: 1,
                witness_position: 1,
            },
            PinnedPart {
                id: 2,
                denomination: 1_000_000,
                output_index: 2,
                bucket: 4,
                anchor_bucket: 2,
                target: 581,
                state: TransferState::Assigned,
                txid: None,
                expiry: None,
                attempts: 0,
                witness_position: 2,
            },
        ];

        let fixture = fixture();
        let client = fixture.load_example_wallet().await;
        let wallet = client.wallet().read().await;

        assert_eq!(wallet.current_version(), LightWallet::serialized_version());
        assert_eq!(wallet.current_version(), 42);
        assert_wallet_capability_matches_seed(&wallet, fixture.example_wallet_seed()).await;
        assert_eq!(
            wallet.sync_state.last_known_chain_height(),
            Some(BlockHeight::from_u32(TIP))
        );

        let state = wallet.migration().expect("the fixture carries a migration");
        assert_eq!(state.mode(), MigrationMode::Scheduled);
        assert_eq!(*state.phase(), MigrationPhase::Scheduled);
        assert_eq!(state.strategy(), SigningStrategy::LazyAtBoundary);
        assert_eq!(state.account(), zip32::AccountId::ZERO);
        assert_eq!(state.params().k_max(), 1);
        assert_eq!(state.params().bucket_modulus(), 144);
        assert_eq!(state.params().transfer_fee(), PART_FEE);
        assert_eq!(hex::encode(state.commitment().params_hash), PARAMS_HASH);
        assert_eq!(
            state.commitment().params_hash,
            state.params().params_hash(),
            "the committed parameter hash is the hash of the stored parameters"
        );
        assert_eq!(hex::encode(state.commitment().plan_hash), PLAN_HASH);
        assert_eq!(state.commitment().committed_at, COMMITTED_AT);

        assert_eq!(state.transfers().len(), pinned.len());
        for (part, expected) in state.transfers().iter().zip(&pinned) {
            assert_eq!(part.id, TransferId(expected.id));
            assert_eq!(part.denomination, expected.denomination);
            assert_eq!(part.bucket_index, Some(expected.bucket));
            assert_eq!(part.anchor_bucket, Some(expected.anchor_bucket));
            assert_eq!(
                part.target_height,
                Some(BlockHeight::from_u32(expected.target))
            );
            assert_eq!(part.state, expected.state);
            assert_eq!(
                part.txid.map(|txid| txid.to_string()),
                expected.txid.map(str::to_string)
            );
            assert_eq!(
                part.expiry_height,
                expected.expiry.map(BlockHeight::from_u32)
            );
            assert_eq!(part.attempts, expected.attempts);
            assert_eq!(
                part.anchor_witness.as_ref().map(|witness| witness.position),
                Some(expected.witness_position)
            );
            assert!(
                part.previous_txids.is_empty(),
                "a version-4 section carries no discarded signatures"
            );
            assert_eq!(
                part.missed_windows, 0,
                "a version-4 section carries no missed-window count"
            );

            let bound = part.note.expect("every fixture part is bound");
            assert_eq!(bound.output_id.txid().to_string(), FUNDING_TXID);
            assert_eq!(bound.output_id.output_index(), expected.output_index);
            let note = wallet
                .wallet_transactions
                .values()
                .flat_map(OrchardNote::transaction_outputs)
                .find(|note| note.output_id() == bound.output_id)
                .expect("the bound note is a real wallet note");
            assert_eq!(note.value(), expected.denomination + PART_FEE);
            assert_eq!(
                note.nullifier()
                    .expect("scanned notes carry nullifiers")
                    .to_bytes(),
                bound.nullifier
            );
            assert_eq!(
                note.spending_transaction().map(|txid| txid.to_string()),
                expected.txid.map(str::to_string),
                "a sent part's note is spent by that part's transaction"
            );
        }

        let status_of = |txid: &str| {
            wallet
                .wallet_transactions
                .values()
                .find(|tx| tx.txid().to_string() == txid)
                .map(|tx| tx.status())
                .unwrap_or_else(|| panic!("transaction {txid} is in the wallet"))
        };
        assert_eq!(wallet.wallet_transactions.len(), 3);
        assert_eq!(
            status_of(FUNDING_TXID),
            ConfirmationStatus::Confirmed(BlockHeight::from_u32(FUNDING_HEIGHT))
        );
        assert_eq!(
            status_of(PART_ZERO_TXID),
            ConfirmationStatus::Confirmed(BlockHeight::from_u32(PART_ZERO_CONFIRMED_AT))
        );
        assert_eq!(
            status_of(PART_ONE_TXID),
            ConfirmationStatus::Calculated(BlockHeight::from_u32(PART_ONE_BUILT_AT))
        );
    }
}
