// use byteorder::WriteBytesExt;
use std::collections::{HashMap, HashSet};
use std::io::{self, Read, Write};

use append_only_vec::AppendOnlyVec;
use bip0039::Mnemonic;

use orchard::keys::Scope;
use zcash_address::unified::Ufvk;
use zcash_client_backend::address::UnifiedAddress;
// use zcash_encoding::Vector;
use zcash_primitives::legacy::TransparentAddress;
use zingoconfig::ZingoConfig;

#[cfg(feature = "ledger-support")]
use super::ledger::LedgerWalletCapability;

use super::unified::{Capability, ReceiverSelection, WalletCapability};
use crate::wallet::traits::ReadableWriteable;

#[derive(Debug)]
pub enum Keystore {
    InMemory(super::unified::WalletCapability),
    #[cfg(feature = "ledger-support")]
    Ledger(super::ledger::LedgerWalletCapability),
}

impl From<WalletCapability> for Keystore {
    fn from(value: WalletCapability) -> Self {
        Self::InMemory(value)
    }
}

#[cfg(feature = "ledger-support")]
impl From<LedgerWalletCapability> for Keystore {
    fn from(value: LedgerWalletCapability) -> Self {
        Self::Ledger(value)
    }
}

impl Keystore {
    pub(crate) fn get_ua_from_contained_transparent_receiver(
        &self,
        receiver: &TransparentAddress,
    ) -> Option<UnifiedAddress> {
        match self {
            Keystore::InMemory(wc) => wc.get_ua_from_contained_transparent_receiver(receiver),
            #[cfg(feature = "ledger-support")]
            Keystore::Ledger(wc) => wc.get_ua_from_contained_transparent_receiver(receiver),
        }
    }

    pub fn addresses(&self) -> &AppendOnlyVec<UnifiedAddress> {
        match self {
            Keystore::InMemory(wc) => wc.addresses(),
            #[cfg(feature = "ledger-support")]
            Keystore::Ledger(wc) => wc.addresses(),
        }
    }

    pub fn transparent_child_keys(
        &self,
    ) -> Result<&AppendOnlyVec<(usize, secp256k1::SecretKey)>, String> {
        match self {
            Keystore::InMemory(wc) => wc.transparent_child_keys(),
            #[cfg(feature = "ledger-support")]
            Keystore::Ledger(wc) => wc.transparent_child_keys(),
        }
    }

    pub(crate) fn ufvk(&self) -> Result<Ufvk, zcash_address::unified::ParseError> {
        match self {
            Keystore::InMemory(wc) => wc.ufvk(),
            #[cfg(feature = "ledger-support")]
            Keystore::Ledger(wc) => wc.ufvk(),
        }
    }

    pub fn new_address(
        &self,
        desired_receivers: ReceiverSelection,
        config: &ZingoConfig,
    ) -> Result<UnifiedAddress, String> {
        match self {
            Keystore::InMemory(wc) => wc.new_address(desired_receivers),
            #[cfg(feature = "ledger-support")]
            Keystore::Ledger(wc) => wc.new_address(desired_receivers, config),
        }
    }

    pub fn get_taddr_to_secretkey_map(
        &self,
        config: &ZingoConfig,
    ) -> Result<HashMap<String, secp256k1::SecretKey>, String> {
        match self {
            Keystore::InMemory(wc) => wc.get_taddr_to_secretkey_map(config),
            #[cfg(feature = "ledger-support")]
            Keystore::Ledger(wc) => wc.get_taddr_to_secretkey_map(config),
        }
    }

    pub fn new_from_seed(config: &ZingoConfig, seed: &[u8; 64], position: u32) -> Self {
        WalletCapability::new_from_seed(config, seed, position).into()
    }

    pub fn new_from_phrase(
        config: &ZingoConfig,
        seed_phrase: &Mnemonic,
        position: u32,
    ) -> Result<Self, String> {
        WalletCapability::new_from_phrase(config, seed_phrase, position).map(Self::from)
    }

    /// Creates a new `WalletCapability` from a unified spending key.
    pub fn new_from_usk(usk: &[u8]) -> Result<Self, String> {
        WalletCapability::new_from_usk(usk).map(Self::from)
    }

    pub fn new_from_ufvk(config: &ZingoConfig, ufvk_encoded: String) -> Result<Self, String> {
        WalletCapability::new_from_ufvk(config, ufvk_encoded).map(Self::from)
    }

    pub(crate) fn get_all_taddrs(&self, config: &ZingoConfig) -> HashSet<String> {
        match self {
            Keystore::InMemory(wc) => wc.get_all_taddrs(config),
            #[cfg(feature = "ledger-support")]
            Keystore::Ledger(wc) => wc.get_all_taddrs(config),
        }
    }

    pub fn first_sapling_address(&self) -> sapling_crypto::PaymentAddress {
        match self {
            Keystore::InMemory(wc) => wc.first_sapling_address(),
            #[cfg(feature = "ledger-support")]
            Keystore::Ledger(wc) => wc.first_sapling_address(),
        }
    }

    /// Returns a selection of pools where the wallet can spend funds.
    pub fn can_spend_from_all_pools(&self) -> bool {
        match self {
            Keystore::InMemory(wc) => wc.can_spend_from_all_pools(),
            #[cfg(feature = "ledger-support")]
            Keystore::Ledger(wc) => wc.can_spend_from_all_pools(),
        }
    }

    pub fn get_trees_witness_trees(&self) -> Option<crate::wallet::data::WitnessTrees> {
        if self.can_spend_from_all_pools() {
            Some(crate::wallet::data::WitnessTrees::default())
        } else {
            None
        }
    }

    /// Returns a selection of pools where the wallet can view funds.
    pub fn can_view(&self) -> ReceiverSelection {
        match self {
            Keystore::InMemory(wc) => wc.can_view(),
            #[cfg(feature = "ledger-support")]
            Keystore::Ledger(wc) => wc.can_view(),
        }
    }

    // Checks if there is spendable Orchard balance capability.
    pub fn can_spend_orchard(&self) -> bool {
        match self {
            Keystore::InMemory(wc) => matches!(wc.orchard, Capability::Spend(_)),
            #[cfg(feature = "ledger-support")]
            Keystore::Ledger(_) => false, // Adjust based on Ledger's Orchard support
        }
    }

    // Checks if there is spendable Sapling balance capability.
    pub fn can_spend_sapling(&self) -> bool {
        match self {
            Keystore::InMemory(wc) => matches!(wc.sapling, Capability::Spend(_)),
            #[cfg(feature = "ledger-support")]
            Keystore::Ledger(wc) => matches!(wc.sapling, Capability::Spend(_)),
        }
    }

    // Checks if there is capability to view Transparent balances.
    pub fn can_view_transparent(&self) -> bool {
        match self {
            Keystore::InMemory(wc) => wc.transparent.can_view(),
            #[cfg(feature = "ledger-support")]
            Keystore::Ledger(wc) => wc.transparent.can_view(),
        }
    }
}

impl Keystore {
    #[cfg(feature = "ledger-support")]
    pub fn new_ledger() -> Result<Self, io::Error> {
        let lwc = LedgerWalletCapability::new()?;
        Ok(Self::Ledger(lwc))
    }

    pub fn wc(&self) -> Option<&WalletCapability> {
        match self {
            Self::InMemory(wc) => Some(wc),
            #[cfg(feature = "ledger-support")]
            _ => None,
        }
    }

    #[cfg(feature = "ledger-support")]
    pub fn ledger_wc(&self) -> Option<&LedgerWalletCapability> {
        let Self::Ledger(wc) = self else { return None };
        Some(wc)
    }

    // Method to get the kind string for the transparent capability
    pub fn transparent_kind_str(&self) -> &str {
        match self {
            Keystore::InMemory(wc) => wc.transparent.kind_str(),
            #[cfg(feature = "ledger-support")]
            Keystore::Ledger(wc) => wc.transparent.kind_str(),
        }
    }

    // Similarly for sapling and orchard capabilities
    pub fn sapling_kind_str(&self) -> &str {
        match self {
            Keystore::InMemory(wc) => wc.sapling.kind_str(),
            #[cfg(feature = "ledger-support")]
            Keystore::Ledger(wc) => wc.sapling.kind_str(),
        }
    }

    pub fn orchard_kind_str(&self) -> &str {
        match self {
            Keystore::InMemory(wc) => wc.orchard.kind_str(),
            #[cfg(feature = "ledger-support")]
            Keystore::Ledger(_) => "Unsupported by ledger application",
        }
    }

    pub fn transparent_capability(
        &self,
    ) -> &Capability<
        super::extended_transparent::ExtendedPubKey,
        super::extended_transparent::ExtendedPrivKey,
    > {
        match self {
            Self::InMemory(wc) => &wc.transparent,
            #[cfg(feature = "ledger-support")]
            Self::Ledger(wc) => &wc.transparent,
        }
    }
}

// This implementation will always use/return an InMemory Keystore.
// the reason is that keys in ledger devices never lived out of the device, never,
// even worse if keys or addresses are store into a file.
// at least this would be our take for now.
impl ReadableWriteable<()> for Keystore {
    const VERSION: u8 = 2;

    fn read<R: Read>(reader: R, _input: ()) -> io::Result<Self> {
        // TODO: Need to figureout what variant to call,
        // we assume we could have two wallet.dat files?
        // have a sort of pattern in data file name?
        let wc = WalletCapability::read(reader, _input)?;
        Ok(Self::InMemory(wc))
    }

    fn write<W: Write>(&self, writer: W) -> io::Result<()> {
        match self {
            Self::InMemory(wc) => wc.write(writer),
            #[cfg(feature = "ledger-support")]
            Self::Ledger(lwc) => lwc.write(writer),
        }
    }
}

impl TryFrom<&Keystore> for super::extended_transparent::ExtendedPrivKey {
    type Error = String;
    fn try_from(wc: &Keystore) -> Result<Self, String> {
        match wc {
            Keystore::InMemory(wc) => wc.try_into(),

            #[cfg(feature = "ledger-support")]
            Keystore::Ledger(lwc) => lwc.try_into(),
        }
    }
}

impl TryFrom<&Keystore> for sapling_crypto::zip32::ExtendedSpendingKey {
    type Error = String;
    fn try_from(wc: &Keystore) -> Result<Self, String> {
        match wc {
            Keystore::InMemory(wc) => wc.try_into(),

            #[cfg(feature = "ledger-support")]
            Keystore::Ledger(lwc) => lwc.try_into(),
        }
    }
}

impl TryFrom<&Keystore> for orchard::keys::SpendingKey {
    type Error = String;
    fn try_from(wc: &Keystore) -> Result<Self, String> {
        match wc {
            Keystore::InMemory(wc) => wc.try_into(),

            #[cfg(feature = "ledger-support")]
            Keystore::Ledger(_) => {
                Err("Ledger device does not support Orchard protocol.".to_string())
            }
        }
    }
}

impl TryFrom<&Keystore> for super::extended_transparent::ExtendedPubKey {
    type Error = String;
    fn try_from(wc: &Keystore) -> Result<Self, String> {
        match wc {
            Keystore::InMemory(wc) => wc.try_into(),

            #[cfg(feature = "ledger-support")]
            Keystore::Ledger(lwc) => lwc.try_into(),
        }
    }
}

impl TryFrom<&Keystore> for orchard::keys::FullViewingKey {
    type Error = String;
    fn try_from(wc: &Keystore) -> Result<Self, String> {
        match wc {
            Keystore::InMemory(wc) => wc.try_into(),

            #[cfg(feature = "ledger-support")]
            Keystore::Ledger(_) => {
                Err("Ledger device does not support Orchard protocol.".to_string())
            }
        }
    }
}

impl TryFrom<&Keystore> for sapling_crypto::zip32::DiversifiableFullViewingKey {
    type Error = String;
    fn try_from(wc: &Keystore) -> Result<Self, String> {
        match wc {
            Keystore::InMemory(wc) => wc.try_into(),

            #[cfg(feature = "ledger-support")]
            Keystore::Ledger(lwc) => lwc.try_into(),
        }
    }
}

impl TryFrom<&Keystore> for sapling_crypto::note_encryption::PreparedIncomingViewingKey {
    type Error = String;

    fn try_from(value: &Keystore) -> Result<Self, Self::Error> {
        match value {
            Keystore::InMemory(wc) => sapling_crypto::SaplingIvk::try_from(wc)
                .map(|k| sapling_crypto::note_encryption::PreparedIncomingViewingKey::new(&k)),

            #[cfg(feature = "ledger-support")]
            Keystore::Ledger(lwc) => sapling_crypto::SaplingIvk::try_from(lwc)
                .map(|k| sapling_crypto::note_encryption::PreparedIncomingViewingKey::new(&k)),
        }
    }
}

impl TryFrom<&Keystore> for orchard::keys::IncomingViewingKey {
    type Error = String;
    fn try_from(wc: &Keystore) -> Result<Self, String> {
        match wc {
            Keystore::InMemory(wc) => {
                let fvk: orchard::keys::FullViewingKey = wc.try_into()?;
                Ok(fvk.to_ivk(Scope::External))
            }
            #[cfg(feature = "ledger-support")]
            Keystore::Ledger(_) => {
                Err("Ledger device does not support Orchard protocol.".to_string())
            }
        }
    }
}

impl TryFrom<&Keystore> for orchard::keys::PreparedIncomingViewingKey {
    type Error = String;
    fn try_from(wc: &Keystore) -> Result<Self, String> {
        match wc {
            Keystore::InMemory(wc) => orchard::keys::IncomingViewingKey::try_from(wc)
                .map(|k| orchard::keys::PreparedIncomingViewingKey::new(&k)),

            #[cfg(feature = "ledger-support")]
            Keystore::Ledger(_) => {
                Err("Ledger device does not support Orchard protocol.".to_string())
            }
        }
    }
}

impl TryFrom<&Keystore> for sapling_crypto::SaplingIvk {
    type Error = String;
    fn try_from(wc: &Keystore) -> Result<Self, String> {
        match wc {
            Keystore::InMemory(wc) => {
                let fvk: sapling_crypto::zip32::DiversifiableFullViewingKey = wc.try_into()?;
                Ok(fvk.fvk().vk.ivk())
            }

            #[cfg(feature = "ledger-support")]
            Keystore::Ledger(lwc) => {
                let fvk: sapling_crypto::zip32::DiversifiableFullViewingKey = lwc.try_into()?;
                Ok(fvk.fvk().vk.ivk())
            }
        }
    }
}

impl TryFrom<&Keystore> for orchard::keys::OutgoingViewingKey {
    type Error = String;
    fn try_from(wc: &Keystore) -> Result<Self, String> {
        match wc {
            Keystore::InMemory(wc) => {
                let fvk: orchard::keys::FullViewingKey = wc.try_into()?;
                Ok(fvk.to_ovk(Scope::External))
            }
            #[cfg(feature = "ledger-support")]
            Keystore::Ledger(_) => {
                Err("Ledger device does not support Orchard protocol.".to_string())
            }
        }
    }
}

impl TryFrom<&Keystore> for sapling_crypto::keys::OutgoingViewingKey {
    type Error = String;
    fn try_from(wc: &Keystore) -> Result<Self, String> {
        match wc {
            Keystore::InMemory(wc) => {
                let fvk: sapling_crypto::zip32::DiversifiableFullViewingKey = wc.try_into()?;
                Ok(fvk.fvk().ovk)
            }

            #[cfg(feature = "ledger-support")]
            Keystore::Ledger(lwc) => {
                let fvk: sapling_crypto::zip32::DiversifiableFullViewingKey = lwc.try_into()?;
                Ok(fvk.fvk().ovk)
            }
        }
    }
}
