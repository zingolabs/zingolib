use std::io::{Cursor, Read};

use crate::{
    config::{ChainType, ClientConfig, WalletConfig},
    lightclient::{LightClient, error::LightClientError},
    testutils::default_test_wallet_settings,
};
use tempfile::TempDir;
use zingo_common_components::protocol::ActivationHeights;
use zingo_test_vectors::seeds::CHIMNEY_BETTER_SEED;

#[tokio::test]
async fn new_wallet_from_phrase() {
    let temp_dir = TempDir::new().unwrap();
    let config = ClientConfig::builder()
        .set_chain_type(ChainType::Regtest(ActivationHeights::default()))
        .set_wallet_dir(temp_dir.path().to_path_buf())
        .set_wallet_config(WalletConfig::MnemonicPhrase {
            mnemonic_phrase: CHIMNEY_BETTER_SEED.to_string(),
            no_of_accounts: 1.try_into().unwrap(),
            birthday: 1,
            wallet_settings: default_test_wallet_settings(),
        })
        .build()
        .unwrap();

    let mut lc = LightClient::new(config.clone(), false).await.unwrap();

    lc.save_task().await;
    lc.wait_for_save().await;

    let lc_file_exists_error = LightClient::new(config, false).await.unwrap_err();

    assert!(matches!(
        lc_file_exists_error,
        LightClientError::FileError(ref e) if e.kind() == std::io::ErrorKind::AlreadyExists
    ));

    // The first transparent address and unified address should be derived
    assert_eq!(
        "tmYd5GP6JxUxTUcz98NLPumEotvaMPaXytz".to_string(),
        lc.transparent_addresses_json().await[0]["encoded_address"]
    );
    assert_eq!(
        "uregtest15en5x5cnsc7ye3wfy0prnh3ut34ns9w40htunlh9htfl6k5p004ja5gprxfz8fygjeax07a8489wzjk8gsx65thcp6d3ku8umgaka6f0"
            .to_string(),
        lc.unified_addresses_json().await[0]["encoded_address"]
    );
}

/// Round-trips a wallet through `save()` and `from_bytes`, asserting the deserialized
/// `LightClient` exposes the same derived addresses as the source. Crucially, the
/// `from_bytes` config uses `WalletConfig::Read` with an empty wallet_dir, so no
/// file is written or read; if `from_bytes` ever regresses into touching disk this
/// assertion would still pass but the call would fail to construct.
#[tokio::test]
async fn from_bytes_roundtrip() {
    let temp_dir = TempDir::new().unwrap();
    let config = ClientConfig::builder()
        .set_chain_type(ChainType::Regtest(ActivationHeights::default()))
        .set_wallet_dir(temp_dir.path().to_path_buf())
        .set_wallet_config(WalletConfig::MnemonicPhrase {
            mnemonic_phrase: CHIMNEY_BETTER_SEED.to_string(),
            no_of_accounts: 1.try_into().unwrap(),
            birthday: 1,
            wallet_settings: default_test_wallet_settings(),
        })
        .build()
        .unwrap();

    // Source wallet → serialized bytes via the same in-memory `save()` that mobile
    // consumers use to ship the wallet across the FFI.
    let source = LightClient::new(config.clone(), false).await.unwrap();
    let bytes = source
        .wallet()
        .write()
        .await
        .save()
        .expect("save returned an error")
        .expect("nothing to save");

    // Reconstruct purely from bytes. Note the config carries no source file path,
    // confirming the constructor never touches the filesystem to load the wallet.
    let restored_config = ClientConfig::builder()
        .set_chain_type(ChainType::Regtest(ActivationHeights::default()))
        .set_wallet_dir(temp_dir.path().to_path_buf())
        .set_wallet_config(WalletConfig::Read)
        .build()
        .unwrap();
    let restored = LightClient::from_bytes(bytes, restored_config)
        .await
        .unwrap();

    assert_eq!(
        source.transparent_addresses_json().await[0]["encoded_address"],
        restored.transparent_addresses_json().await[0]["encoded_address"],
    );
    assert_eq!(
        source.unified_addresses_json().await[0]["encoded_address"],
        restored.unified_addresses_json().await[0]["encoded_address"],
    );
}

/// A reader that yields at most one byte per `read` call.
struct OneByteReader(Cursor<Vec<u8>>);

impl Read for OneByteReader {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let len = buf.len().min(1);
        self.0.read(&mut buf[..len])
    }
}

/// Feeds one byte per call to `from_reader` and checks the wallet matches `from_bytes`.
#[tokio::test]
async fn from_reader_with_one_byte_reads_matches_from_bytes() {
    let temp_dir = TempDir::new().unwrap();
    let config = ClientConfig::builder()
        .set_chain_type(ChainType::Regtest(ActivationHeights::default()))
        .set_wallet_dir(temp_dir.path().to_path_buf())
        .set_wallet_config(WalletConfig::MnemonicPhrase {
            mnemonic_phrase: CHIMNEY_BETTER_SEED.to_string(),
            no_of_accounts: 1.try_into().unwrap(),
            birthday: 1,
            wallet_settings: default_test_wallet_settings(),
        })
        .build()
        .unwrap();

    let source = LightClient::new(config, false).await.unwrap();
    let bytes = source
        .wallet()
        .write()
        .await
        .save()
        .expect("save returned an error")
        .expect("nothing to save");

    let read_config = || {
        ClientConfig::builder()
            .set_chain_type(ChainType::Regtest(ActivationHeights::default()))
            .set_wallet_dir(temp_dir.path().to_path_buf())
            .set_wallet_config(WalletConfig::Read)
            .build()
            .unwrap()
    };
    let from_bytes = LightClient::from_bytes(bytes.clone(), read_config())
        .await
        .unwrap();
    let from_reader = LightClient::from_reader(OneByteReader(Cursor::new(bytes)), read_config())
        .await
        .unwrap();

    assert_eq!(
        from_reader.mnemonic_phrase().as_deref(),
        Some(CHIMNEY_BETTER_SEED)
    );
    assert_eq!(from_bytes.mnemonic_phrase(), from_reader.mnemonic_phrase());
    assert_eq!(from_bytes.birthday(), from_reader.birthday());
    assert_eq!(
        from_bytes.transparent_addresses_json().await,
        from_reader.transparent_addresses_json().await,
    );
    assert_eq!(
        from_bytes.unified_addresses_json().await,
        from_reader.unified_addresses_json().await,
    );
    let chain_type = from_bytes.chain_type();
    let mut bytes_serialized = Vec::new();
    from_bytes
        .wallet()
        .write()
        .await
        .write(&mut bytes_serialized, &chain_type)
        .unwrap();
    let mut reader_serialized = Vec::new();
    from_reader
        .wallet()
        .write()
        .await
        .write(&mut reader_serialized, &chain_type)
        .unwrap();
    assert_eq!(bytes_serialized, reader_serialized);
}

/// The `info` data/error channel contract (zingolabs/zingolib#2446).
///
/// Failure must travel on the error channel. The data channel carries
/// only typed data. Downstream FFIs must never have to inspect a
/// returned value's content to learn whether the call succeeded.
mod info_contract {
    use crate::lightclient::LightClient;
    use crate::testutils::synthetic_wallet::SyntheticWalletBuilder;

    /// The test client is Indexerless, so the info request must fail,
    /// and that failure must not surface as prose in the data channel.
    ///
    /// This began as the red TDD test for the migration: `do_info`
    /// returned `String`, and the connection failure arrived as a
    /// `Status {..}` Debug string indistinguishable by type from
    /// data. It originally pinned a typed `IndexerError` from a
    /// never-listening lazy endpoint. The offline-mode work replaced
    /// that placeholder with a genuinely Indexerless client, whose
    /// typed failure is `Offline`. The migration ended with `do_info`
    /// deleted outright: `info()` returns a typed `ServerInfo`, and
    /// rendering happens only at presentation boundaries.
    #[tokio::test]
    async fn info_failure_stays_out_of_the_data_channel() {
        let wallet =
            SyntheticWalletBuilder::new(zingo_test_vectors::seeds::HOSPITAL_MUSEUM_SEED).build();
        let mut client = LightClient::new_for_test(wallet).await;

        let error = client
            .info()
            .await
            .expect_err("an Indexerless client cannot serve info");
        assert!(
            matches!(error, crate::lightclient::error::LightClientError::Offline),
            "the failure must be typed, not prose: {error}"
        );
    }
}
