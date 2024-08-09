//! Trait implmentations for sync interface

use zingo_sync::interface::SyncWallet;

use crate::wallet::LightWallet;

impl SyncWallet for LightWallet {
    type Error = ();

    fn set_sync_state(&mut self) -> Result<&mut zingo_sync::SyncState, Self::Error> {
        Ok(&mut self.sync_state)
    }
}
