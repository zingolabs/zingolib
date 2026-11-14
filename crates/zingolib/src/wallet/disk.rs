//! This mod contains write and read functionality of impl `LightWallet`

use std::io::{self, Read, Write};

use zcash_protocol::consensus;

use zingolib_file_format::{MigrationSection, WalletFile, WalletFileParts, WalletFileRef};

use super::LightWallet;
use crate::config::ChainType;
use crate::wallet::migration::{MigrationState, store};

pub use zingolib_file_format::ChainMismatch;

impl MigrationSection for MigrationState {
    fn read<R: Read>(reader: R, chain: ChainType) -> io::Result<Self> {
        store::read(reader, chain)
    }

    fn write<W: Write>(&self, writer: W) -> io::Result<()> {
        store::write(writer, self)
    }
}

impl LightWallet {
    #[must_use]
    pub const fn serialized_version() -> u64 {
        zingolib_file_format::serialized_version()
    }

    /// Upper bound on the version word [`Self::read_recovery_info`] accepts:
    /// far above any version this project will reach, low enough that random
    /// or encrypted bytes cannot land in it.
    pub const MAX_RECOVERABLE_VERSION: u64 = zingolib_file_format::MAX_RECOVERABLE_VERSION;

    /// Upper bound on the birthday [`Self::read_recovery_info`] accepts: no
    /// real chain reaches this height for centuries, while fabricated
    /// prefixes decode to birthdays far above it.
    pub const MAX_RECOVERABLE_BIRTHDAY: u32 = zingolib_file_format::MAX_RECOVERABLE_BIRTHDAY;

    /// Serialize into `writer`
    pub fn write<W: Write>(
        &mut self,
        writer: W,
        consensus_parameters: &impl consensus::Parameters,
    ) -> io::Result<()> {
        self.file_ref().write(writer, consensus_parameters)
    }

    /// Deserialize into `reader`
    // TODO: update to return WalletError
    pub fn read<R: Read>(reader: R, chain_type: ChainType) -> io::Result<Self> {
        let mut wallet = Self::from_file(WalletFile::read(reader, chain_type)?);
        wallet.raise_birthday_to_sapling_activation();
        Ok(wallet)
    }

    /// The chain a wallet file was written for. Version 32 and later store
    /// it right after the version word; older files store it past the keys,
    /// so they are read whole under each chain until one parses.
    pub fn read_chain(bytes: &[u8]) -> io::Result<ChainType> {
        zingolib_file_format::read_chain(bytes)
    }

    /// Confirms the bytes parse as a complete wallet this build can read, by
    /// running the full [`WalletFile::read`] deserialization and discarding the
    /// result; on failure the error names the byte offset reached.
    pub fn validate<R: Read>(reader: R, chain_type: ChainType) -> io::Result<()> {
        WalletFile::<MigrationState>::validate(reader, chain_type)
    }

    /// Recovers the seed phrase, birthday, and account count from the stable
    /// prefix of a version 32+ wallet file, without parsing the rest of the
    /// file. This is the escape hatch when [`Self::read`] fails on a file
    /// written by an orphaned or unknown format revision: the recovered
    /// info suffices to restore the wallet from seed and rescan.
    ///
    /// Fails on legacy files (version below 32), whose seed is stored too
    /// deep in the file to reach without a full parse, and on view-only
    /// wallets, which store no seed.
    ///
    /// Rejects versions above [`Self::MAX_RECOVERABLE_VERSION`] and birthdays
    /// above [`Self::MAX_RECOVERABLE_BIRTHDAY`], so random or encrypted bytes
    /// cannot come back as a confident seed phrase.
    pub fn read_recovery_info<R: Read>(reader: R) -> io::Result<super::RecoveryInfo> {
        zingolib_file_format::read_recovery_info(reader)
    }

    fn from_file(file: WalletFile<MigrationState>) -> Self {
        let WalletFileParts {
            read_version,
            chain_type,
            mnemonic,
            birthday,
            unified_key_store,
            unified_addresses,
            transparent_addresses,
            wallet_blocks,
            wallet_transactions,
            nullifier_map,
            outpoint_map,
            shard_trees,
            sync_state,
            wallet_settings,
            price_list,
            migration,
        } = file.into_parts();
        Self {
            current_version: Self::serialized_version(),
            read_version,
            chain_type,
            mnemonic,
            birthday,
            birthday_adjusted_from: None,
            unified_key_store,
            unified_addresses,
            transparent_addresses,
            wallet_blocks,
            wallet_transactions,
            nullifier_map,
            outpoint_map,
            shard_trees,
            sync_state,
            wallet_settings,
            price_list,
            migration,
            send_proposal: None,
            output_locks: crate::wallet::locks::OutputLocks::default(),
            save_required: false,
        }
    }

    fn file_ref(&mut self) -> WalletFileRef<'_, MigrationState> {
        WalletFileRef {
            chain_type: self.chain_type,
            mnemonic: self.mnemonic.as_ref(),
            birthday: self.birthday,
            unified_key_store: &self.unified_key_store,
            unified_addresses: &self.unified_addresses,
            transparent_addresses: &self.transparent_addresses,
            wallet_blocks: &self.wallet_blocks,
            wallet_transactions: &self.wallet_transactions,
            nullifier_map: &self.nullifier_map,
            outpoint_map: &self.outpoint_map,
            shard_trees: &mut self.shard_trees,
            sync_state: &mut self.sync_state,
            wallet_settings: &mut self.wallet_settings,
            price_list: &self.price_list,
            migration: self.migration.as_ref(),
        }
    }
}

#[cfg(any(test, feature = "testutils"))]
pub mod testing;
