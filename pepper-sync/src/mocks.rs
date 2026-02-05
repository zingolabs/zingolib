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
pub(crate) enum MockWalletError {}
#[derive(Debug)]
pub(crate) struct MockWallet {
    birthday: BlockHeight,
    sync_state: SyncState,
    wallet_blocks: BTreeMap<BlockHeight, WalletBlock>,
    wallet_transactions: HashMap<TxId, WalletTransaction>,
    nullifier_map: NullifierMap,
    outpoint_map: BTreeMap<OutputId, ScanTarget>,
    shard_trees: ShardTrees,
}
impl MockWallet {
    pub(crate) fn new(birthday: BlockHeight) -> MockWallet {
        MockWallet {
            birthday,
            sync_state: SyncState::new(),
            wallet_blocks: BTreeMap::new(),
            wallet_transactions: HashMap::new(),
            nullifier_map: NullifierMap::new(),
            outpoint_map: BTreeMap::new(),
            shard_trees: ShardTrees::new(),
        }
    }
}
impl SyncWallet for MockWallet {
    type Error = MockWalletError;

    fn get_birthday(&self) -> Result<BlockHeight, Self::Error> {
        Ok(self.birthday)
    }

    fn get_sync_state(&self) -> Result<&crate::wallet::SyncState, Self::Error> {
        todo!()
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
