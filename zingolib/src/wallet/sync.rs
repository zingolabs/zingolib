//! Trait implmentations for sync interface

use std::collections::HashMap;

use zcash_keys::keys::{UnifiedFullViewingKey, UnifiedSpendingKey};
use zingo_sync::interface::{SyncCompactBlocks, SyncWallet};
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
    async fn get_wallet_compact_block(
        &self,
        block_height: zcash_primitives::consensus::BlockHeight,
    ) -> Result<zingo_sync::primitives::WalletCompactBlock, Self::Error> {
        self.compact_blocks
            .read()
            .await
            .get(&block_height)
            .cloned()
            .ok_or(())
    }

    async fn store_wallet_compact_blocks(
        &self,
        wallet_compact_blocks: HashMap<
            zcash_primitives::consensus::BlockHeight,
            zingo_sync::primitives::WalletCompactBlock,
        >,
    ) -> Result<(), Self::Error> {
        self.compact_blocks
            .write()
            .await
            .extend(wallet_compact_blocks);

        Ok(())
    }
}
