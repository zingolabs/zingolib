//! Trait implmentations for sync interface

use std::collections::{BTreeMap, HashMap};

use zcash_keys::keys::{UnifiedFullViewingKey, UnifiedSpendingKey};
use zcash_primitives::consensus::BlockHeight;
use zingo_sync::{
    interface::{SyncBlocks, SyncNullifiers, SyncShardTrees, SyncWallet},
    primitives::{NullifierMap, WalletBlock},
    witness::ShardTrees,
};
use zip32::AccountId;

use crate::wallet::LightWallet;

impl SyncWallet for LightWallet {
    type Error = ();

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

    fn append_wallet_blocks(
        &mut self,
        mut wallet_compact_blocks: BTreeMap<BlockHeight, WalletBlock>,
    ) -> Result<(), Self::Error> {
        self.wallet_blocks.append(&mut wallet_compact_blocks);

        Ok(())
    }
}

impl SyncNullifiers for LightWallet {
    fn append_nullifiers(&mut self, mut nullifier_map: NullifierMap) -> Result<(), Self::Error> {
        self.nullifier_map
            .sapling_mut()
            .append(nullifier_map.sapling_mut());
        self.nullifier_map
            .orchard_mut()
            .append(nullifier_map.orchard_mut());

        Ok(())
    }
}

impl SyncShardTrees for LightWallet {
    fn get_shard_trees_mut(&mut self) -> Result<&mut ShardTrees, Self::Error> {
        Ok(self.shard_trees_mut())
    }
}
