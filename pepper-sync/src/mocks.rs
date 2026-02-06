use crate::{
    config::{self, TransparentAddressDiscovery},
    wallet::{
        NullifierMap, OutputId, ScanTarget, ShardTrees, SyncState, WalletBlock, WalletTransaction,
        traits::{
            SyncBlocks, SyncNullifiers, SyncOutPoints, SyncShardTrees, SyncTransactions, SyncWallet,
        },
    },
};
use std::collections::{BTreeMap, HashMap};
use zcash_protocol::{TxId, consensus::BlockHeight};

#[derive(Debug, thiserror::Error)]
pub(super) enum MockWalletError {}
#[derive(Debug)]
pub(super) struct MockWallet {
    birthday: BlockHeight,
    sync_state: SyncState,
    wallet_blocks: BTreeMap<BlockHeight, WalletBlock>,
    wallet_transactions: HashMap<TxId, WalletTransaction>,
    nullifier_map: NullifierMap,
    outpoint_map: BTreeMap<OutputId, ScanTarget>,
    shard_trees: ShardTrees,
}
impl Default for MockWalletBuilder {
    fn default() -> Self {
        MockWalletBuilder {
            birthday: BlockHeight::from_u32(0),
            sync_state: SyncState::new(),
            wallet_blocks: BTreeMap::new(),
            wallet_transactions: HashMap::new(),
            nullifier_map: NullifierMap::new(),
            outpoint_map: BTreeMap::new(),
            shard_trees: ShardTrees::new(),
        }
    }
}

pub(super) struct MockWalletBuilder {
    birthday: BlockHeight,
    sync_state: SyncState,
    wallet_blocks: BTreeMap<BlockHeight, WalletBlock>,
    wallet_transactions: HashMap<TxId, WalletTransaction>,
    nullifier_map: NullifierMap,
    outpoint_map: BTreeMap<OutputId, ScanTarget>,
    shard_trees: ShardTrees,
}

impl MockWalletBuilder {
    pub(crate) fn birthday(mut self, birthday: BlockHeight) -> Self {
        self.birthday = birthday;
        self
    }

    pub(crate) fn sync_state(mut self, sync_state: SyncState) -> Self {
        self.sync_state = sync_state;
        self
    }

    pub(crate) fn wallet_blocks(
        mut self,
        wallet_blocks: BTreeMap<BlockHeight, WalletBlock>,
    ) -> Self {
        self.wallet_blocks = wallet_blocks;
        self
    }

    pub(crate) fn wallet_transactions(
        mut self,
        wallet_transactions: HashMap<TxId, WalletTransaction>,
    ) -> Self {
        self.wallet_transactions = wallet_transactions;
        self
    }

    pub(crate) fn nullifier_map(mut self, nullifier_map: NullifierMap) -> Self {
        self.nullifier_map = nullifier_map;
        self
    }

    pub(crate) fn outpoint_map(mut self, outpoint_map: BTreeMap<OutputId, ScanTarget>) -> Self {
        self.outpoint_map = outpoint_map;
        self
    }

    pub(crate) fn shard_trees(mut self, shard_trees: ShardTrees) -> Self {
        self.shard_trees = shard_trees;
        self
    }
    pub(crate) fn new() -> MockWalletBuilder {
        MockWalletBuilder::default()
    }
    pub(crate) fn create_mock_wallet(self) -> MockWallet {
        MockWallet {
            birthday: self.birthday,
            sync_state: self.sync_state,
            wallet_blocks: self.wallet_blocks,
            wallet_transactions: self.wallet_transactions,
            nullifier_map: self.nullifier_map,
            outpoint_map: self.outpoint_map,
            shard_trees: self.shard_trees,
        }
    }
}
impl SyncWallet for MockWallet {
    type Error = MockWalletError;

    fn get_birthday(&self) -> Result<BlockHeight, Self::Error> {
        Ok(self.birthday)
    }

    fn get_sync_state(&self) -> Result<&crate::wallet::SyncState, Self::Error> {
        Ok(&self.sync_state)
    }

    fn get_sync_state_mut(&mut self) -> Result<&mut crate::wallet::SyncState, Self::Error> {
        todo!()
    }

    fn get_unified_full_viewing_keys(
        &self,
    ) -> Result<
        std::collections::HashMap<zip32::AccountId, zcash_keys::keys::UnifiedFullViewingKey>,
        Self::Error,
    > {
        todo!()
    }

    fn add_orchard_address(
        &mut self,
        account_id: zip32::AccountId,
        address: orchard::Address,
        diversifier_index: zip32::DiversifierIndex,
    ) -> Result<(), Self::Error> {
        todo!()
    }

    fn add_sapling_address(
        &mut self,
        account_id: zip32::AccountId,
        address: sapling_crypto::PaymentAddress,
        diversifier_index: zip32::DiversifierIndex,
    ) -> Result<(), Self::Error> {
        todo!()
    }

    fn get_transparent_addresses(
        &self,
    ) -> Result<
        &std::collections::BTreeMap<crate::keys::transparent::TransparentAddressId, String>,
        Self::Error,
    > {
        todo!()
    }

    fn get_transparent_addresses_mut(
        &mut self,
    ) -> Result<
        &mut std::collections::BTreeMap<crate::keys::transparent::TransparentAddressId, String>,
        Self::Error,
    > {
        todo!()
    }

    fn set_save_flag(&mut self) -> Result<(), Self::Error> {
        Ok(())
    }
}
impl SyncBlocks for MockWallet {
    fn get_wallet_block(
        &self,
        block_height: BlockHeight,
    ) -> Result<crate::wallet::WalletBlock, Self::Error> {
        todo!()
    }

    fn get_wallet_blocks_mut(
        &mut self,
    ) -> Result<&mut std::collections::BTreeMap<BlockHeight, crate::wallet::WalletBlock>, Self::Error>
    {
        todo!()
    }
}
impl SyncTransactions for MockWallet {
    fn get_wallet_transactions(
        &self,
    ) -> Result<
        &std::collections::HashMap<zcash_protocol::TxId, crate::wallet::WalletTransaction>,
        Self::Error,
    > {
        todo!()
    }

    fn get_wallet_transactions_mut(
        &mut self,
    ) -> Result<
        &mut std::collections::HashMap<zcash_protocol::TxId, crate::wallet::WalletTransaction>,
        Self::Error,
    > {
        todo!()
    }
}
impl SyncNullifiers for MockWallet {
    fn get_nullifiers(&self) -> Result<&crate::wallet::NullifierMap, Self::Error> {
        todo!()
    }

    fn get_nullifiers_mut(&mut self) -> Result<&mut crate::wallet::NullifierMap, Self::Error> {
        todo!()
    }
}
impl SyncOutPoints for MockWallet {
    fn get_outpoints(
        &self,
    ) -> Result<
        &std::collections::BTreeMap<crate::wallet::OutputId, crate::wallet::ScanTarget>,
        Self::Error,
    > {
        todo!()
    }

    fn get_outpoints_mut(
        &mut self,
    ) -> Result<
        &mut std::collections::BTreeMap<crate::wallet::OutputId, crate::wallet::ScanTarget>,
        Self::Error,
    > {
        todo!()
    }
}
impl SyncShardTrees for MockWallet {
    fn get_shard_trees(&self) -> Result<&crate::wallet::ShardTrees, Self::Error> {
        todo!()
    }

    fn get_shard_trees_mut(&mut self) -> Result<&mut crate::wallet::ShardTrees, Self::Error> {
        todo!()
    }
}
//libuse zingolib::config::zingoconfigbuilder;
//use zingolib::wallet::{lightwallet, walletbase::freshentropy, walletsettings};
/*
fn create_bday_confirm_wallet(bday: blockheight, min_confirmations: nonzerou32) -> lightwallet {
    let chain = zingoconfigbuilder::default().create().chain;
    let sync_config = config::syncconfig {
        transparent_address_discovery: transparentaddressdiscovery::minimal(),
        performance_level: config::performancelevel::low,
    };
    let entropy = freshentropy {
        no_of_accounts: nonzerou32::try_from(1).expect("1 is non-zero u32"),
    };
    let wallet_settings = walletsettings {
        sync_config,
        min_confirmations,
    };
    todo!()
}
*/
