#![forbid(unsafe_code)]

use std::collections::{BTreeMap, HashMap};
use std::io::{self, Read, Write};
use std::num::NonZeroU32;

use bip0039::Mnemonic;
use zcash_keys::address::UnifiedAddress;
use zcash_primitives::transaction::TxId;
use zcash_protocol::consensus::BlockHeight;

use zingolib_common::price::PriceList;
use zingolib_common::{
    chain::ChainType,
    config::SyncConfig,
    keys::{UnifiedAddressId, UnifiedKeyStore, transparent::TransparentAddressId},
    sync::SyncState,
    wallet::{NullifierMap, OutputId, ScanTarget, ShardTrees, WalletBlock, WalletTransaction},
};

mod file;
#[cfg(feature = "legacy")]
mod legacy;
pub mod strings;

pub use file::{
    ChainMismatch, MAX_RECOVERABLE_BIRTHDAY, MAX_RECOVERABLE_VERSION, read_chain,
    read_recovery_info, serialized_version,
};

/// Wallet settings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WalletSettings {
    /// Sync configuration.
    pub sync_config: zingolib_common::config::SyncConfig,
    /// Minimum confirmations.
    pub min_confirmations: NonZeroU32,
}

impl Default for WalletSettings {
    fn default() -> Self {
        Self {
            sync_config: SyncConfig::default(),
            min_confirmations: NonZeroU32::try_from(3).expect("hard-coded non-zero integer"),
        }
    }
}

/// Provides necessary information to recover the wallet without the wallet file.
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct RecoveryInfo {
    /// 24-word mnemonic phrase.
    pub seed_phrase: String,
    /// Block height wallet was created.
    pub birthday: u64,
    /// Number of accounts in use.
    pub no_of_accounts: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub birthday_adjusted_from: Option<u64>,
}

impl std::fmt::Display for RecoveryInfo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Wallet backup info:
{{
    seed phrase: {}
    birthday: {}
    no_of_accounts: {}
}}",
            self.seed_phrase, self.birthday, self.no_of_accounts,
        )
    }
}

#[derive(Debug)]
pub struct WalletFile<M>(WalletFileParts<M>);

impl<M> WalletFile<M> {
    pub fn read_version(&self) -> u64 {
        self.0.read_version
    }
    pub fn chain_type(&self) -> ChainType {
        self.0.chain_type
    }
    pub fn mnemonic(&self) -> Option<&Mnemonic> {
        self.0.mnemonic.as_ref()
    }
    pub fn birthday(&self) -> BlockHeight {
        self.0.birthday
    }
    pub fn unified_key_store(&self) -> &BTreeMap<zip32::AccountId, UnifiedKeyStore> {
        &self.0.unified_key_store
    }
    pub fn unified_addresses(&self) -> &BTreeMap<UnifiedAddressId, UnifiedAddress> {
        &self.0.unified_addresses
    }
    pub fn transparent_addresses(&self) -> &BTreeMap<TransparentAddressId, String> {
        &self.0.transparent_addresses
    }
    pub fn wallet_blocks(&self) -> &BTreeMap<BlockHeight, WalletBlock> {
        &self.0.wallet_blocks
    }
    pub fn wallet_transactions(&self) -> &HashMap<TxId, WalletTransaction> {
        &self.0.wallet_transactions
    }
    pub fn nullifier_map(&self) -> &NullifierMap {
        &self.0.nullifier_map
    }
    pub fn outpoint_map(&self) -> &BTreeMap<OutputId, ScanTarget> {
        &self.0.outpoint_map
    }
    pub fn shard_trees(&self) -> &ShardTrees {
        &self.0.shard_trees
    }
    pub fn sync_state(&self) -> &SyncState {
        &self.0.sync_state
    }
    pub fn wallet_settings(&self) -> &WalletSettings {
        &self.0.wallet_settings
    }
    pub fn price_list(&self) -> &PriceList {
        &self.0.price_list
    }
    pub fn migration(&self) -> Option<&M> {
        self.0.migration.as_ref()
    }

    pub fn into_parts(self) -> WalletFileParts<M> {
        self.0
    }
}

#[derive(Debug)]
pub struct WalletFileParts<M> {
    pub read_version: u64,
    pub chain_type: ChainType,
    pub mnemonic: Option<Mnemonic>,
    pub birthday: BlockHeight,
    pub unified_key_store: BTreeMap<zip32::AccountId, UnifiedKeyStore>,
    pub unified_addresses: BTreeMap<UnifiedAddressId, UnifiedAddress>,
    pub transparent_addresses: BTreeMap<TransparentAddressId, String>,
    pub wallet_blocks: BTreeMap<BlockHeight, WalletBlock>,
    pub wallet_transactions: HashMap<TxId, WalletTransaction>,
    pub nullifier_map: NullifierMap,
    pub outpoint_map: BTreeMap<OutputId, ScanTarget>,
    pub shard_trees: ShardTrees,
    pub sync_state: SyncState,
    pub wallet_settings: WalletSettings,
    pub price_list: PriceList,
    pub migration: Option<M>,
}

#[derive(Debug)]
pub struct WalletFileRef<'a, M> {
    pub chain_type: ChainType,
    pub mnemonic: Option<&'a Mnemonic>,
    pub birthday: BlockHeight,
    pub unified_key_store: &'a BTreeMap<zip32::AccountId, UnifiedKeyStore>,
    pub unified_addresses: &'a BTreeMap<UnifiedAddressId, UnifiedAddress>,
    pub transparent_addresses: &'a BTreeMap<TransparentAddressId, String>,
    pub wallet_blocks: &'a BTreeMap<BlockHeight, WalletBlock>,
    pub wallet_transactions: &'a HashMap<TxId, WalletTransaction>,
    pub nullifier_map: &'a NullifierMap,
    pub outpoint_map: &'a BTreeMap<OutputId, ScanTarget>,
    pub shard_trees: &'a mut ShardTrees,
    pub sync_state: &'a mut SyncState,
    pub wallet_settings: &'a mut WalletSettings,
    pub price_list: &'a PriceList,
    pub migration: Option<&'a M>,
}

pub trait MigrationSection: Sized {
    fn read<R: Read>(reader: R, chain: ChainType) -> io::Result<Self>;

    fn write<W: Write>(&self, writer: W) -> io::Result<()>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawMigration(pub Vec<u8>);

impl MigrationSection for RawMigration {
    fn read<R: Read>(mut reader: R, _chain: ChainType) -> io::Result<Self> {
        let mut section = Vec::new();
        reader.read_to_end(&mut section)?;
        Ok(Self(section))
    }

    fn write<W: Write>(&self, mut writer: W) -> io::Result<()> {
        writer.write_all(&self.0)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, HashMap};

    use super::{MigrationSection, RawMigration, WalletFile, WalletFileRef, WalletSettings};
    use zcash_protocol::consensus::BlockHeight;
    use zingolib_common::chain::ChainType;
    use zingolib_common::price::PriceList;
    use zingolib_common::shardtree_ext::{
        DOWNSTREAM_BOUNDARY_PATH, assert_raw_boolean_calls_confined,
    };
    use zingolib_common::sync::SyncState;
    use zingolib_common::wallet::{NullifierMap, ShardTrees};

    #[test]
    fn accessors_read_what_into_parts_returns() {
        const BIRTHDAY: u32 = 1_000_000;
        const SECTION: [u8; 3] = [7, 8, 9];
        let chain_type = ChainType::Mainnet;

        let mut bytes = Vec::new();
        WalletFileRef {
            chain_type,
            mnemonic: None,
            birthday: BlockHeight::from_u32(BIRTHDAY),
            unified_key_store: &BTreeMap::new(),
            unified_addresses: &BTreeMap::new(),
            transparent_addresses: &BTreeMap::new(),
            wallet_blocks: &BTreeMap::new(),
            wallet_transactions: &HashMap::new(),
            nullifier_map: &NullifierMap::new(),
            outpoint_map: &BTreeMap::new(),
            shard_trees: &mut ShardTrees::new(),
            sync_state: &mut SyncState::new(),
            wallet_settings: &mut WalletSettings::default(),
            price_list: &PriceList::new(),
            migration: Some(&RawMigration(SECTION.to_vec())),
        }
        .write(&mut bytes, &chain_type)
        .expect("writing into an in-memory Vec<u8> never returns an io::Error");
        let read = || {
            WalletFile::<RawMigration>::read(bytes.as_slice(), chain_type)
                .expect("the bytes were written by the writer of this crate")
        };

        let file = read();
        let parts = read().into_parts();

        assert_eq!(file.read_version(), parts.read_version);
        assert_eq!(file.chain_type(), parts.chain_type);
        assert_eq!(file.mnemonic(), parts.mnemonic.as_ref());
        assert_eq!(file.birthday(), parts.birthday);
        assert_eq!(file.wallet_settings(), &parts.wallet_settings);
        assert_eq!(file.migration(), parts.migration.as_ref());
        assert_eq!(parts.migration, Some(RawMigration(SECTION.to_vec())));
    }

    #[test]
    fn raw_migration_round_trips_every_byte() {
        const SECTION_LENGTH: u8 = 32;
        let section = RawMigration((0..SECTION_LENGTH).collect());

        let mut written = Vec::new();
        section
            .write(&mut written)
            .expect("writing into an in-memory Vec<u8> never returns an io::Error");
        let read_back = RawMigration::read(written.as_slice(), ChainType::Mainnet)
            .expect("reading the rest of an in-memory slice never fails");

        assert_eq!(read_back, section);
    }

    #[test]
    fn raw_boolean_calls_route_through_the_wrapper() {
        assert_raw_boolean_calls_confined(
            &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src"),
            None,
            DOWNSTREAM_BOUNDARY_PATH,
        );
    }
}
