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
        BoundNote, ConsentBinding, MigrationParams, MigrationPhase, MigrationState, PartId,
        PartRecord, SigningStrategy,
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
    let mut part = PartRecord::new(
        PartId(0),
        100_000_000,
        BoundNote {
            output_id: OutputId::new(TxId::from_bytes([3; 32]), 1),
            nullifier: [4; 32],
            commitment: [5; 32],
        },
    );
    part.assign(12).unwrap();
    let state = MigrationState {
        consent: ConsentBinding {
            params_hash: params.params_hash(),
            plan_hash: [6; 32],
            consented_at: 1_782_000_000,
        },
        params,
        strategy: SigningStrategy::LazyAtBoundary,
        mode: crate::wallet::migration::MigrationMode::Scheduled,
        account: zip32::AccountId::ZERO,
        phase: MigrationPhase::PartsScheduled,
        parts: vec![part],
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
        BoundNote, ConsentBinding, MigrationParams, MigrationPhase, MigrationState, PartId,
        PartRecord, SigningStrategy,
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
    let mut part = PartRecord::new(
        PartId(0),
        100_000_000,
        BoundNote {
            output_id: OutputId::new(TxId::from_bytes([3; 32]), 1),
            nullifier: [4; 32],
            commitment: [5; 32],
        },
    );
    part.assign(12).unwrap();
    wallet.migration = Some(MigrationState {
        consent: ConsentBinding {
            params_hash: params.params_hash(),
            plan_hash: [6; 32],
            consented_at: 1_782_000_000,
        },
        params,
        strategy: SigningStrategy::LazyAtBoundary,
        mode: crate::wallet::migration::MigrationMode::Scheduled,
        account: zip32::AccountId::ZERO,
        phase: MigrationPhase::PartsScheduled,
        parts: vec![part],
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

    use crate::config::ChainType;
    use crate::wallet::LightWallet;

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
