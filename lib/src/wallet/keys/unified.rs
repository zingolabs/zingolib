use std::sync::Arc;

use bip0039::Mnemonic;
use orchard::keys::Scope;
use tokio::sync::{Mutex, RwLock};
use zcash_client_backend::address::UnifiedAddress;
use zcash_primitives::zip32::DiversifierIndex;
use zingoconfig::ZingoConfig;

use super::{extended_transparent::KeyIndex, Keys};

#[derive(Clone, Debug)]
pub struct UnifiedSpendAuthority {
    orchard_key: orchard::keys::SpendingKey,
    sapling_key: zcash_primitives::zip32::ExtendedSpendingKey,
    transparent_parent_key: super::extended_transparent::ExtendedPrivKey,
    transparent_child_keys: Vec<secp256k1::SecretKey>,

    addresses: Vec<UnifiedAddress>,
    // Not all diversifier indexes produce valid sapling addresses.
    // Because of this, the index isn't necessarily equal to addresses.len()
    next_sapling_diversifier_index: DiversifierIndex,
}

#[derive(Debug, Clone, Copy)]
#[non_exhaustive]
pub struct ReceiverSelection {
    orchard: bool,
    sapling: bool,
    transparent: bool,
}

impl UnifiedSpendAuthority {
    pub async fn new_address(
        &mut self,
        desired_receivers: ReceiverSelection,
    ) -> Result<UnifiedAddress, String> {
        let orchard_receiver = if desired_receivers.orchard {
            Some(
                orchard::keys::FullViewingKey::from(&self.orchard_key)
                    .address_at(self.addresses.len(), Scope::External),
            )
        } else {
            None
        };
        let sapling_reciever = if desired_receivers.sapling {
            let (mut new_index, address) =
                zcash_primitives::zip32::ExtendedFullViewingKey::from(&self.sapling_key)
                    .find_address(self.next_sapling_diversifier_index)
                    .expect("Diversifier index overflow");
            new_index.increment().expect("Diversifier index overflow");
            self.next_sapling_diversifier_index = new_index;
            Some(address)
        } else {
            None
        };
        let transparent_receiver = if desired_receivers.transparent {
            let key_index =
                KeyIndex::hardened_from_normalize_index(self.addresses.len() as u32).unwrap();
            let new_key = self
                .transparent_parent_key
                .derive_private_key(key_index)
                .expect("Private key derevation failed")
                .private_key;
            let secp = secp256k1::Secp256k1::new();
            let address = secp256k1::PublicKey::from_secret_key(&secp, &new_key);
            self.transparent_child_keys.push(new_key);
            Some(address)
        } else {
            None
        };

        UnifiedAddress::from_receivers(
            orchard_receiver,
            sapling_reciever,
            transparent_receiver
                .as_ref()
                // This is deprecated. Not sure what the alternative is,
                // other than implementing it ourselves.
                .map(zcash_primitives::legacy::keys::pubkey_to_address),
        )
        .ok_or(format!(
            "Invalid receivers requested! At least one of sapling or orchard required"
        ))
    }

    pub fn new_from_seed(config: &ZingoConfig, seed: &[u8; 64], position: u32) -> Self {
        let (sapling_key, _, _) = Keys::get_zaddr_from_bip39seed(config, seed, position);
        let transparent_parent_key =
            super::extended_transparent::ExtendedPrivKey::get_ext_taddr_from_bip39seed(
                config, seed, position,
            );

        let orchard_key =
            orchard::keys::SpendingKey::from_zip32_seed(seed, config.get_coin_type(), position)
                .unwrap();
        Self {
            sapling_key,
            orchard_key,
            transparent_parent_key,
            transparent_child_keys: vec![],
            addresses: vec![],
            next_sapling_diversifier_index: DiversifierIndex::new(),
        }
    }
}
