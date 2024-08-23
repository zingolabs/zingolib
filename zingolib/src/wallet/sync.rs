//! Trait implmentations for sync interface

use std::{
    collections::{BTreeMap, HashMap},
    sync::atomic,
};

use zcash_keys::keys::{UnifiedFullViewingKey, UnifiedSpendingKey};
use zcash_primitives::consensus::BlockHeight;
use zingo_sync::{
    primitives::{NullifierMap, SyncState, WalletBlock},
    traits::{SyncBlocks, SyncNullifiers, SyncShardTrees, SyncTransactions, SyncWallet},
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
        Ok(self.sync_state())
    }

    fn get_sync_state_mut(&mut self) -> Result<&mut SyncState, Self::Error> {
        Ok(self.sync_state_mut())
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
}

impl SyncBlocks for LightWallet {
    fn get_wallet_block(&self, block_height: BlockHeight) -> Result<WalletBlock, Self::Error> {
        self.wallet_blocks.get(&block_height).cloned().ok_or(())
    }

    fn get_wallet_blocks_mut(
        &mut self,
    ) -> Result<&mut BTreeMap<BlockHeight, WalletBlock>, Self::Error> {
        Ok(self.wallet_blocks_mut())
    }
}

impl SyncTransactions for LightWallet {
    fn get_wallet_transactions_mut(
        &mut self,
    ) -> Result<
        &mut HashMap<
            zcash_primitives::transaction::TxId,
            zingo_sync::primitives::WalletTransaction,
        >,
        Self::Error,
    > {
        Ok(self.wallet_transactions_mut())
    }
}

impl SyncNullifiers for LightWallet {
    fn get_nullifiers_mut(&mut self) -> Result<&mut NullifierMap, ()> {
        Ok(self.nullifier_map_mut())
    }
}

impl SyncShardTrees for LightWallet {
    fn get_shard_trees_mut(&mut self) -> Result<&mut ShardTrees, Self::Error> {
        Ok(self.shard_trees_mut())
    }
}
