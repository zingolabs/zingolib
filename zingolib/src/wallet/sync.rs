//! Trait implmentations for sync interface

use std::collections::{BTreeMap, HashMap};

use zcash_keys::keys::{UnifiedFullViewingKey, UnifiedSpendingKey};
use zcash_primitives::consensus::BlockHeight;
use zingo_sync::{
    interface::{SyncCompactBlocks, SyncNullifiers, SyncWallet},
    primitives::{NullifierMap, WalletCompactBlock},
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

impl SyncCompactBlocks for LightWallet {
    fn get_wallet_compact_block(
        &self,
        block_height: BlockHeight,
    ) -> Result<WalletCompactBlock, Self::Error> {
        self.compact_blocks.get(&block_height).cloned().ok_or(())
    }

    fn append_wallet_compact_blocks(
        &mut self,
        mut wallet_compact_blocks: BTreeMap<BlockHeight, WalletCompactBlock>,
    ) -> Result<(), Self::Error> {
        self.compact_blocks.append(&mut wallet_compact_blocks);

        Ok(())
    }
}

impl SyncNullifiers for LightWallet {
    fn append_nullifiers(&mut self, mut nullifier_map: NullifierMap) -> Result<(), ()> {
        self.nullifier_map
            .sapling_mut()
            .append(nullifier_map.sapling_mut());
        self.nullifier_map
            .orchard_mut()
            .append(nullifier_map.orchard_mut());

        Ok(())
    }
}
