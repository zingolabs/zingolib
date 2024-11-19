//! Holds information related to the ledger 



use std::io;
use secp256k1::{PublicKey, Secp256k1, SecretKey};
use crate::wallet::{
    traits::ReadableWriteable,
    keys::capability::InternalCapability,
};

/// Holds ledger things
#[derive(Debug)]
pub struct LedgerKeys {
    ledger_id: PublicKey,
    _app: ZcashApp
}

//TODO! this is all mocked code
impl LedgerKeys {
    /// TODO! this is all mocked code
    pub fn new() -> LedgerKeys{
        // Create a new secp256k1 context
        let secp = Secp256k1::new();

        // Generate a secret key for testing purposes (not secure)
        let secret_key = SecretKey::from_slice(&[0x01; 32]).expect("32 bytes, within curve order");

        // Derive the corresponding public key
        let public_key = PublicKey::from_secret_key(&secp, &secret_key);
        LedgerKeys { ledger_id: public_key, _app: ZcashApp::new() }
    }
}

/// Placeholder for the real thing
#[derive(Debug)]
pub struct ZcashApp {}

impl ZcashApp {
    fn new() -> ZcashApp { ZcashApp {  }}
}

impl ReadableWriteable for LedgerKeys {
    const VERSION: u8 = 0; //not applicable

    fn read<R: std::io::Read>(mut reader: R, _input: ()) -> std::io::Result<Self> {
        // let version = Self::read_version(&mut reader)?;

        // if version > Self::VERSION {
        //     let e = format!(
        //         "Don't know how to read ledger wallet version {}. Do you have the latest version?",
        //         version
        //     );
        //     return Err(io::Error::new(io::ErrorKind::InvalidData, e));
        // }

        //retrieve the ledger id and verify it matches with the aocnnected device
        let ledger_id = {
            let mut buf = [0; secp256k1::constants::PUBLIC_KEY_SIZE];
            reader.read_exact(&mut buf)?;

            PublicKey::from_slice(&buf).map_err(|e| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("Bad public key stored for ledger id: {:?}", e),
                )
            })?
        };

        // let app = Self::connect_ledger()?;
        // // lets used futures simpler executor
        // if ledger_id != futures::executor::block_on(Self::get_id(&app))? {
        //     return Err(io::Error::new(
        //         io::ErrorKind::InvalidInput,
        //         "Detected different ledger than used previously".to_string(),
        //     ));
        // }
        Ok(LedgerKeys { ledger_id, _app: ZcashApp::new()})

    }

    fn write<W: std::io::Write>(&self, mut writer: W, _input: ()) -> std::io::Result<()> {
        let id = self.ledger_id.serialize();
        writer.write_all(&id)?;

        Ok(())
    }
}

#[derive(Debug)]
pub (crate) struct LedgerCapability {}

impl LedgerCapability {
    pub(crate) fn new() -> LedgerCapability { 
        LedgerCapability {}
    }
}
impl InternalCapability for LedgerCapability {
    fn get_ua_from_contained_transparent_receiver(
        &self,
        capability: &super::unified::WalletCapability,
        receiver: &zcash_primitives::legacy::TransparentAddress,
    ) -> Option<zcash_keys::address::UnifiedAddress> {
        todo!()
    }

    fn addresses(&self, capability: &super::unified::WalletCapability) -> &append_only_vec::AppendOnlyVec<zcash_keys::address::UnifiedAddress> {
        todo!()
    }

    fn transparent_child_addresses(
        &self, 
        capability: &super::unified::WalletCapability
    ) -> &std::sync::Arc<append_only_vec::AppendOnlyVec<(usize, zcash_primitives::legacy::TransparentAddress)>> {
        todo!()
    }

    fn new_address(
        &self,
        capability: &super::unified::WalletCapability,
        desired_receivers: super::unified::ReceiverSelection,
        legacy_key: bool,
    ) -> Result<zcash_keys::address::UnifiedAddress, String> {
        todo!()
    }

    fn generate_transparent_receiver(
        &self,
        capability: &super::unified::WalletCapability,
        // this should only be `true` when generating transparent addresses while loading from legacy keys (pre wallet version 29)
        // legacy transparent keys are already derived to the external scope so setting `legacy_key` to `true` will skip this scope derivation
        legacy_key: bool,
    ) -> Result<Option<zcash_primitives::legacy::TransparentAddress>, bip32::Error> {
        todo!()
    }

    fn get_taddr_to_secretkey_map(
        &self,
        capability: &super::unified::WalletCapability,
        chain: &crate::config::ChainType,
    ) -> Result<std::collections::HashMap<String, secp256k1::SecretKey>, crate::wallet::error::KeyError> {
        todo!()
    }

    fn first_sapling_address(&self, capability: &super::unified::WalletCapability) -> sapling_crypto::PaymentAddress {
        todo!()
    }

    fn can_view(&self, capability: &super::unified::WalletCapability) -> super::unified::ReceiverSelection {
        todo!()
    }
    
    fn get_external_taddrs(
        &self,
        capability: &super::unified::WalletCapability,
        chain: &crate::config::ChainType
    ) -> std::collections::HashSet<String> {
        todo!()
    }
    
    fn get_taddrs(&self,
        capability: &super::unified::WalletCapability,
        chain: &crate::config::ChainType
    ) -> std::collections::HashSet<String> {
        todo!()
    }
}