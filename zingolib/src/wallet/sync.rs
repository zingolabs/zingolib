//! Trait implmentations for sync interface

use std::{
    collections::{BTreeMap, HashMap},
    sync::atomic,
};

use zcash_keys::keys::{UnifiedFullViewingKey, UnifiedSpendingKey};
use zcash_primitives::consensus::BlockHeight;
use zingo_sync::{
    keys::transparent::TransparentAddressId,
    primitives::{NullifierMap, OutPointMap, SyncState, WalletBlock},
    traits::{
        SyncBlocks, SyncNullifiers, SyncOutPoints, SyncShardTrees, SyncTransactions, SyncWallet,
    },
    witness::ShardTrees,
};
use zip32::AccountId;

use crate::wallet::LightWallet;

impl SyncWallet for LightWallet {
    type Error = ();

    fn get_birthday(&self) -> Result<BlockHeight, Self::Error> {
        let birthday = self.birthday.load(atomic::Ordering::Relaxed);
        Ok(BlockHeight::from_u32(birthday as u32))
    }

    fn get_sync_state(&self) -> Result<&SyncState, Self::Error> {
        Ok(&self.sync_state)
    }

    fn get_sync_state_mut(&mut self) -> Result<&mut SyncState, Self::Error> {
        Ok(&mut self.sync_state)
    }

    fn get_unified_full_viewing_keys(
        &self,
    ) -> Result<HashMap<AccountId, UnifiedFullViewingKey>, Self::Error> {
        let account_id = AccountId::try_from(0).unwrap();
        let seed = self
            .mnemonic()
            .map(|(mmemonic, _)| mmemonic)
            .unwrap()
            .to_seed("");
        let usk = UnifiedSpendingKey::from_seed(
            &self.transaction_context.config.chain,
            &seed,
            account_id,
        )
        .unwrap();
        let ufvk = usk.to_unified_full_viewing_key();
        let mut ufvk_map = HashMap::new();
        ufvk_map.insert(account_id, ufvk);

        Ok(ufvk_map)
    }

    fn get_transparent_addresses(
        &self,
    ) -> Result<&BTreeMap<TransparentAddressId, String>, Self::Error> {
        Ok(&self.transparent_addresses)
    }

    fn get_transparent_addresses_mut(
        &mut self,
    ) -> Result<&mut BTreeMap<TransparentAddressId, String>, Self::Error> {
        Ok(&mut self.transparent_addresses)
    }
}

impl SyncBlocks for LightWallet {
    fn get_wallet_block(&self, block_height: BlockHeight) -> Result<WalletBlock, Self::Error> {
        self.wallet_blocks.get(&block_height).cloned().ok_or(())
    }

    fn get_wallet_blocks_mut(
        &mut self,
    ) -> Result<&mut BTreeMap<BlockHeight, WalletBlock>, Self::Error> {
        Ok(&mut self.wallet_blocks)
    }
}

impl SyncTransactions for LightWallet {
    fn get_wallet_transactions(
        &self,
    ) -> Result<
        &HashMap<zcash_primitives::transaction::TxId, zingo_sync::primitives::WalletTransaction>,
        Self::Error,
    > {
        Ok(&self.wallet_transactions)
    }

    fn get_wallet_transactions_mut(
        &mut self,
    ) -> Result<
        &mut HashMap<
            zcash_primitives::transaction::TxId,
            zingo_sync::primitives::WalletTransaction,
        >,
        Self::Error,
    > {
        Ok(&mut self.wallet_transactions)
    }
}

impl SyncNullifiers for LightWallet {
    fn get_nullifiers(&self) -> Result<&NullifierMap, Self::Error> {
        Ok(&self.nullifier_map)
    }

    fn get_nullifiers_mut(&mut self) -> Result<&mut NullifierMap, ()> {
        Ok(&mut self.nullifier_map)
    }
}

impl SyncOutPoints for LightWallet {
    fn get_outpoints(&self) -> Result<&OutPointMap, Self::Error> {
        Ok(&self.outpoint_map)
    }

    fn get_outpoints_mut(&mut self) -> Result<&mut OutPointMap, Self::Error> {
        Ok(&mut self.outpoint_map)
    }
}

impl SyncShardTrees for LightWallet {
    fn get_shard_trees_mut(&mut self) -> Result<&mut ShardTrees, Self::Error> {
        Ok(&mut self.shard_trees)
    }

    fn get_shard_trees(&self) -> Result<&ShardTrees, Self::Error> {
        Ok(&self.shard_trees)
    }
}
