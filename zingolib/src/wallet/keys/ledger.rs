use hdwallet::rand_core::le;
use jubjub::Fr;
use ledger_zcash::zcash::primitives::zip32::ChildIndex;
use ripemd160::Ripemd160;
use sha2::{Digest, Sha256};
use zcash_primitives::consensus::NetworkConstants;
use std::collections::BTreeMap;
use std::sync::atomic::{self, AtomicBool};
use std::sync::{Arc, RwLock};
use std::{
    collections::{HashMap, HashSet},
    io::{self, Read, Write},
};

use append_only_vec::AppendOnlyVec;
use hdwallet::KeyIndex;
// use bip0039::Mnemonic;
use byteorder::{LittleEndian, ReadBytesExt, WriteBytesExt};
use ledger_transport::Exchange;
use ledger_transport_hid::{LedgerHIDError, TransportNativeHID};
use ledger_zcash::builder::BuilderError;
use ledger_zcash::{LedgerAppError, ZcashApp};
// use orchard::keys::Scope;
use crate::data::witness_trees::WitnessTrees;
use sapling_crypto::keys::OutgoingViewingKey;
use sapling_crypto::{PaymentAddress, SaplingIvk};
use secp256k1::PublicKey as SecpPublicKey;
// use secp256k1::SecretKey;
use zcash_address::unified::Ufvk;
use zcash_client_backend::address::UnifiedAddress;
// use zcash_client_backend::keys::{Era, UnifiedSpendingKey};
use zcash_primitives::{legacy::TransparentAddress, zip32::DiversifierIndex};

use sapling_crypto::Diversifier;


use zx_bip44::BIP44Path;

use crate::config::ZingoConfig;
use crate::wallet::traits::ReadableWriteable;

// use super::{
//     extended_transparent::{ExtendedPrivKey, ExtendedPubKey, KeyIndex},
//     get_zaddr_from_bip39seed, ToBase58Check,
// };
//
use super::unified::{Capability, ReceiverSelection};

#[derive(Debug, thiserror::Error)]
/// Ledger Errors
pub enum LedgerError {
    #[error("Error: unable to create keystore")]
    /// TODO: Add docs
    InitializationError(#[from] LedgerHIDError),

    #[error("Error: error from inner builder: {}", .0)]
    /// TODO: Add docs
    Builder(#[from] BuilderError),
    #[error("Error: error when communicating with ledger: {}", .0)]
    /// TODO: Add docs
    Ledger(#[from] LedgerAppError<<TransportNativeHID as Exchange>::Error>),

    #[error("Error: the provided derivation path length was invalid, expected {} elements", .0)]
    /// TODO: Add docs
    InvalidPathLength(usize),
    #[error("Error: unable to parse public key returned from ledger")]
    /// TODO: Add docs
    InvalidPublicKey,

    #[error("Error: attempted to overflow diversifier index")]
    /// TODO: Add docs
    DiversifierIndexOverflow,

    #[error("Error: requested key was not found in keystore")]
    /// TODO: Add docs
    KeyNotFound,
}

impl From<LedgerError> for std::io::Error {
    fn from(err: LedgerError) -> Self {
        use std::io::ErrorKind;

        let kind = match &err {
            LedgerError::InitializationError(_) => ErrorKind::InvalidInput,
            LedgerError::Ledger(_) => ErrorKind::BrokenPipe,
            LedgerError::Builder(_)
            | LedgerError::InvalidPathLength(_)
            | LedgerError::InvalidPublicKey
            | LedgerError::DiversifierIndexOverflow
            | LedgerError::KeyNotFound => ErrorKind::InvalidData,
        };

        std::io::Error::new(kind, err)
    }
}
/// Wallet Capability trait of a Ledger Device
pub struct LedgerWalletCapability {
    /// TODO: Add docs
    app: ZcashApp<TransportNativeHID>,
    /// TODO: Add docs
    pub transparent: Capability<
        super::extended_transparent::ExtendedPubKey,
        super::extended_transparent::ExtendedPrivKey,
    >,
    /// TODO: Add docs
    pub sapling: Capability<
        sapling_crypto::zip32::DiversifiableFullViewingKey,
        sapling_crypto::zip32::ExtendedSpendingKey,
    >,
    /// Even though ledger application does not support orchard protocol
    /// we keep this field just for "compatibility" but always set to Capability::None
    pub orchard: Capability<orchard::keys::FullViewingKey, orchard::keys::SpendingKey>,

    // transparent_child_keys: append_only_vec::AppendOnlyVec<(usize, secp256k1::SecretKey)>,
    // addresses: append_only_vec::AppendOnlyVec<UnifiedAddress>,
    // Not all diversifier indexes produce valid sapling addresses.
    // Because of this, the index isn't necessarily equal to addresses.len()
    // addresses_write_lock: AtomicBool,
    //this is a public key with a specific path
    // used to "identify" a ledger
    // this is useful to detect when a different ledger
    // is connected instead of the one used with the keystore
    // originally
    ledger_id: SecpPublicKey,

    transparent_addrs: RwLock<BTreeMap<[u32; 5], SecpPublicKey>>,

    //associated a path with an ivk and the default diversifier
    shielded_addrs: RwLock<BTreeMap<[u32; 3], (SaplingIvk, Diversifier, OutgoingViewingKey)>>,

    pub(crate) transparent_child_addresses: Arc<append_only_vec::AppendOnlyVec<(usize, TransparentAddress)>>,

    pub(crate) addresses: append_only_vec::AppendOnlyVec<UnifiedAddress>,
    // Not all diversifier indexes produce valid sapling addresses.
    // Because of this, the index isn't necessarily equal to addresses.len()
    pub(crate) addresses_write_lock: AtomicBool,
}

impl std::fmt::Debug for LedgerWalletCapability {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LedgerWalletCapability")
            .field("app", &"ledger-app by Zondax")
            .field("transparent", &self.transparent)
            .field("sapling", &self.sapling)
            .field("orchard", &self.orchard)
            // .field("transparent_child_keys", &self.transparent_child_keys)
            // .field("addresses", &self.addresses)
            .field("transparent_addrs", &self.transparent_addrs)
            .field("shielded_addrs", &self.shielded_addrs)
            // .field("addresses_write_lock", &self.addresses_write_lock)
            .field("ledger_id", &self.ledger_id)
            .finish()
    }
}

impl LedgerWalletCapability {
    /// Retrieve the connected ledger's "ID"
    ///
    /// Uses 44'/1'/0/0/0 derivation path
    async fn get_id(app: &ZcashApp<TransportNativeHID>) -> Result<SecpPublicKey, LedgerError> {
        app.get_address_unshielded(
            &BIP44Path([44 + 0x8000_0000, 1 + 0x8000_0000, 0, 0, 0]),
            false,
        )
        .await
        .map_err(Into::into)
        .and_then(|addr| {
            SecpPublicKey::from_slice(&addr.public_key).map_err(|_| LedgerError::InvalidPublicKey)
        })
    }

    /// Attempt to create a handle to a ledger zcash ap
    ///
    /// Will attempt to connect to the first available device,
    /// but won't verify that the correct app is open or that is a "known" device
    fn connect_ledger() -> Result<ZcashApp<TransportNativeHID>, LedgerError> {
        let hidapi = ledger_transport_hid::hidapi::HidApi::new().map_err(LedgerHIDError::Hid)?;

        let transport = TransportNativeHID::new(&hidapi)?;
        let app = ZcashApp::new(transport);

        Ok(app)
    }
    /// TODO: Add docs
    pub fn new() -> Result<Self, LedgerError> {
        let app = Self::connect_ledger()?;
        let ledger_id = futures::executor::block_on(Self::get_id(&app))?;

        Ok(Self {
            app,
            ledger_id,
            // TODO: keep these fields just for compatibility
            // but ledger application is not limited by this.
            sapling: Capability::None,
            transparent: Capability::None,
            orchard: Capability::None,
            transparent_child_addresses: Arc::new(AppendOnlyVec::<(usize, TransparentAddress)>::new()),
            transparent_addrs: Default::default(),
            shielded_addrs: Default::default(),
            addresses: AppendOnlyVec::new(),
            addresses_write_lock: AtomicBool::new(false),
        })
    }

    // TODO: (Pacu) this is commented because the describe.rs file
    // uses this in a non async context and we have to figure out 
    // how to better refactor that.
    pub(crate) fn get_ua_from_contained_transparent_receiver(
        &self,
        receiver: &TransparentAddress,
    ) -> Option<UnifiedAddress> {
        self.addresses
            .iter()
            .find(|ua| ua.transparent() == Some(receiver))
            .cloned()
    }

    /// TODO: in our ledger-app integration we do keep a list of paths
    /// not pub/priv keys so it is necessary to figure out how to compute
    /// unified addresses from that list.
    pub fn addresses(&self) -> &AppendOnlyVec<UnifiedAddress> {
        &self.addresses
    }

    // TODO: Incompatible as secrets will never leave the ledger
    // device.
    // Note: (Pacu) I'm not sure what is this but seems not to be used 
    // in the new codebase
    // pub fn transparent_child_keys(
    //     &self,
    // ) -> Result<&AppendOnlyVec<(usize, secp256k1::SecretKey)>, String> {
    //     unimplemented!()
    // }

    // TODO: This currently not supported by ledger application
    pub(crate) fn ufvk(&self) -> Result<Ufvk, zcash_address::unified::ParseError> {
        unimplemented!()
    }
    /// TODO: Add docs
    pub fn new_address(
        &mut self,
        desired_receivers: ReceiverSelection,
        config: &ZingoConfig,
    ) -> Result<UnifiedAddress, String> {
        // Our wallet capability in this case our ledger application
        // does not make distinction in either it can or not spend or view.
        // so here we do check for the supported protocols, transparent and sappling
        if !desired_receivers.transparent || !desired_receivers.sapling {
            return Err("The wallet is not capable of producing desired receivers.".to_string());
        }

        let previous_num_addresses = self.addresses.len();
        let mut transparent: Option<TransparentAddress> = None;
        let count = self
                .transparent_addrs
                .read()
                .map_err(|e| format!("Error: {e}"))?
                .len();
        let path = Self::t_derivation_path(config.get_coin_type(), count as _);
        if desired_receivers.transparent {
            // let count = self
            //     .transparent_addrs
            //     .read()
            //     .map_err(|e| format!("Error: {e}"))?
            //     .len();

            let t_pubkey = futures::executor::block_on(self.get_t_pubkey(&path))
                .map_err(|e| format!("Error: {e}"))?;

            let mut path_u32 = [0u32; 5];
            path.into_iter()
                .map(|idx| idx.raw_index())
                .zip(path_u32.iter_mut())
                .for_each(|(a, b)| *b = a);

            // This is deprecated. Not sure what the alternative is,
            // other than implementing it ourselves.
            #[allow(deprecated)]
            let t_addrs = zcash_primitives::legacy::keys::pubkey_to_address(&t_pubkey);
            transparent = Some(t_addrs);

            if self
                .addresses_write_lock
                .swap(true, atomic::Ordering::Acquire)
            {
                return Err("addresses_write_lock collision!".to_string());
            }
            
            // TODO: Do we want to keep to separate lists for transparent address and keys?
            // what is appending in one fails whereas the other succeded?
            self.transparent_child_addresses.push((count, t_addrs.clone()));

            self.addresses_write_lock
                .swap(false, atomic::Ordering::Release);

            self.transparent_addrs
                .write()
                .map(|mut w| w.insert(path_u32, t_pubkey))
                .map_err(|e| format!("Error: {e}"))?;
        }

        let mut sapling: Option<PaymentAddress> = None;

        if desired_receivers.sapling {
            let z_addr = futures::executor::block_on(self.add_zaddr(&""))?;
            sapling = Some(z_addr);
        }

        
        self.addresses_write_lock
                    .swap(false, atomic::Ordering::Release);
        let ua = match UnifiedAddress::from_receivers(None, sapling, transparent) {
            Some(address) => address,
            None => {
                return Err(
                    "Invalid receivers requested! At least one of sapling or transparent required, orchard is not supported"
                        .to_string(),
                );
            }
        };

        self.addresses.push(ua.clone());
        assert_eq!(self.addresses.len(), previous_num_addresses + 1);

        Ok(ua)
        
    }
    /// TODO: Add docs
    pub fn t_derivation_path(coin_type: u32, index: u32) -> [KeyIndex; 5] {
        [
            KeyIndex::hardened_from_normalize_index(44).expect("unreachable"),
            KeyIndex::hardened_from_normalize_index(coin_type).expect("unreachable"),
            KeyIndex::hardened_from_normalize_index(0).expect("unreachable"),
            KeyIndex::from_index(0).expect("unreachable"),
            KeyIndex::from_index(index).expect("unreachable"),
        ]
    }

    /// Create a new transparent address with path +1 from the latest one
    pub fn add_taddr(&self, config: &ZingoConfig) -> Result<String, String> {
        //find the highest path we have
        let taddrs = self
            .transparent_addrs
            .read()
            .map_err(|e| format!("Error: {e}"))?;
        let count = taddrs.len();

        let path = taddrs
            .keys()
            .last()
            .cloned()
            .map(|path| {
                [
                    KeyIndex::hardened_from_normalize_index(path[0]).expect("unreachable"),
                    KeyIndex::hardened_from_normalize_index(path[1]).expect("unreachable"),
                    KeyIndex::hardened_from_normalize_index(path[2]).expect("unreachable"),
                    KeyIndex::from_index(path[3]).expect("unreachable"),
                    KeyIndex::from_index(path[4] + 1).expect("unreachable"),
                ]
            })
            // TODO: define how to choose a proper acount, we are using the number of
            // current taddresses
            .unwrap_or_else(|| Self::t_derivation_path(config.get_coin_type(), count as _));

        // do not hold any lock longer
        drop(taddrs);

        let key = futures::executor::block_on(self.get_t_pubkey(&path));

        match key {
            Ok(key) => {
                // tpk is of type secp256k1 so
                // 1. we need to serialize it.
                // 2. compute its sha256
                // 3. comput its ripemd160
                let serialize_key = key.serialize();
                let sha256_hash = Sha256::digest(&serialize_key);
                let hash = Ripemd160::digest(sha256_hash.as_slice());
                Ok(super::ToBase58Check::to_base58check(
                    hash.as_slice(),
                    &config.base58_pubkey_address(),
                    &[],
                ))
            }
            Err(e) => Err(format!("Error: {:?}", e.to_string())),
        }
    }

    async fn get_t_pubkey(&self, path: &[KeyIndex]) -> Result<SecpPublicKey, LedgerError> {
        let path = Self::path_slice_to_bip44(path)?;
        let Ok(cached) = self.transparent_addrs.read() else {
            return Err(LedgerError::InvalidPublicKey);
        };

        let key = cached.get(&path.0).copied();
        drop(cached);

        match key {
            Some(key) => Ok(key),
            None => {
                let addr = self.app.get_address_unshielded(&path, false).await?;

                let pkey = SecpPublicKey::from_slice(&addr.public_key)
                    .map_err(|_| LedgerError::InvalidPublicKey)?;
                self.transparent_addrs
                    .write()
                    .expect("Failed to get transparent_addrs RwLock")
                    .insert(path.0, pkey);

                Ok(pkey)
            }
        }
    }

    fn path_slice_to_bip44(path: &[KeyIndex]) -> Result<BIP44Path, LedgerError> {
        let path = path
            .iter()
            .take(5)
            .map(|child| child.raw_index())
            .collect::<Vec<_>>();

        BIP44Path::from_slice(path.as_slice()).map_err(|_| LedgerError::InvalidPathLength(5))
    }

    /// TODO: Incompatible with our ledger-application as secrets never leave the device
    pub fn get_taddr_to_secretkey_map(
        &self,
        _config: &ZingoConfig,
    ) -> Result<HashMap<String, secp256k1::SecretKey>, String> {
        // Secrects will never leave the ledger device
        // so this method does not makes sense.
        unimplemented!()
    }
    /// TODO: Add docs
    pub(crate) fn get_all_taddrs(&self, config: &ZingoConfig) -> HashSet<String> {
        let Ok(taddrs) = self.transparent_addrs.read() else {
            return HashSet::new();
        };

        taddrs
            .iter()
            .map(|(_, tpk)| {
                // tpk is of type secp256k1 so
                // 1. we need to serialize it.
                // 2. compute its sha256
                // 3. comput its ripemd160
                let serialize_key = tpk.serialize();
                let sha256_hash = Sha256::digest(&serialize_key);
                let hash = Ripemd160::digest(sha256_hash.as_slice());
                super::ToBase58Check::to_base58check(
                    hash.as_slice(),
                    &config.chain.b58_pubkey_address_prefix(),
                    &[],
                )
            })
            .collect::<HashSet<String>>()
        
    }
    /// TODO: Add docs
    pub fn first_sapling_address(&self) -> sapling_crypto::PaymentAddress {
        let Ok(saddrs) = self.shielded_addrs.try_read() else {
            unreachable!()
        };

        // shielded_addrs: RwLock<BTreeMap<[u32; 3], (SaplingIvk, Diversifier, OutgoingViewingKey)>>,
        let path = saddrs
            .keys()
            .next()
            .cloned()
            .expect("No sapling address available");

        self.payment_address_from_path(&path)
            .expect("Error computing paymentAddress")
    }

    /// Attempt to lookup stored data for given path and compute payment address thereafter
    pub fn payment_address_from_path(&self, path: &[u32; 3]) -> Option<PaymentAddress> {
        let Ok(saddrs) = self.shielded_addrs.try_read() else {
            return None;
        };

        saddrs
            .get(path)
            .and_then(|(ivk, d, _)| ivk.to_payment_address(*d))
    }

    /// Create a new shielded address with path +1 from the latest one
    pub async fn add_zaddr(&mut self, optional_path: &str) -> Result<PaymentAddress, String> {
        let path = match optional_path {
            "" => {
                //find the highest path we have

                    self
                    .shielded_addrs
                    .get_mut()
                    .map_err(|e| e.to_string())?
                    
                    .keys()
                    .last()
                    .cloned()
                    .map(|path| {
                        [
                            ChildIndex::from_index(path[0]),
                            ChildIndex::from_index(path[1]),
                            ChildIndex::from_index(path[2] + 1),
                        ]
                    }).unwrap_or_else(||[
                        // FIXME: Figure out how to not hardcode this
                        ChildIndex::Hardened(32),
                        ChildIndex::Hardened(133),
                        ChildIndex::Hardened(0),
                    ])
            },
            val => {
                match Self::convert_path_to_num(val, 3) {
                    Ok(addr) =>
                        [
                            addr.get(0).cloned().unwrap(),
                            addr.get(1).cloned().unwrap(),
                            addr.get(2).cloned().unwrap()
                        ],
                    Err(e) => {
                            return Err(e.to_string())
                        }
                }
            }
        };

        let addr = self.get_z_payment_address(&path)
            .await;
        match addr {
            Ok(sapling_address) => Ok(sapling_address),
            Err(ledger_error) => Err(ledger_error.to_string())
        }

        
    }

    async fn get_z_payment_address(&self, path: &[ChildIndex]) -> Result<PaymentAddress, LedgerError> {
        if path.len() != 3 {
            return Err(LedgerError::InvalidPathLength(3));
        }

        let path = {
            let elements = path
                .iter()
                .map(|ci| match ci {
                    ChildIndex::NonHardened(i) => *i,
                    ChildIndex::Hardened(i) => *i + (1 << 31),
                })
                .enumerate();

            let mut array = [0; 3];
            for (i, e) in elements {
                array[i] = e;
            }

            array
        };

        match self.payment_address_from_path(&path) {
            Some(key) => Ok(key),
            None => {
                let fr = self.app.get_ivk(path[2]).await.unwrap();
                // FIXME: figure out what the hell is going on with these types
                let ivk = SaplingIvk(Fr::from_bytes(&fr).unwrap());

                let div = self.get_default_div(path[2]).await?;

                let ovk = self.app.get_ovk(path[2]).await.map(|ovk| OutgoingViewingKey(ovk))?;

                let addr = ivk
                    .to_payment_address(div)
                    .expect("guaranteed valid diversifier should get a payment address");

                let mut addresses = self.shielded_addrs.write()
                    .map_err(|_| LedgerAppError::Unknown(0))?;
                addresses.insert(path, (ivk, div, ovk));
                Ok(addr)
            }
        }
    }
    

    /// Returns a selection of pools where the wallet can spend funds.
    pub fn can_spend_from_all_pools(&self) -> bool {
        // TODO: as a ledger application, we can either view or spend
        // as our keys lives there.
        // self.sapling.can_spend() && self.transparent.can_spend()
        true
    }

    /// Retrieve the defualt diversifier from a given device and path
    ///
    /// The defualt diversifier is the first valid diversifier starting
    /// from index 0
    async fn get_default_div_from(app: &ZcashApp<TransportNativeHID>, idx: u32) -> Result<Diversifier, LedgerError> {
        let mut index = DiversifierIndex::new();

        loop {
            let divs = app.get_div_list(idx, index.as_bytes()).await?;
            let divs: &[[u8; 11]] = bytemuck::cast_slice(&divs);

            //find the first div that is not all 0s
            // all 0s is when it's an invalid diversifier
            for div in divs {
                if div != &[0; 11] {
                    return Ok(Diversifier(*div));
                }

                //increment the index for each diversifier returned
                index.increment().map_err(|_| LedgerError::DiversifierIndexOverflow)?;
            }
        }
    }

    /// Retrieve the default diversifier for a given path
    ///
    /// The default diversifier is the first valid diversifier starting from
    /// index 0
    pub async fn get_default_div(&self, idx: u32) -> Result<Diversifier, LedgerError> {
        Self::get_default_div_from(&self.app, idx).await
    }

    ///TODO: NAME?????!!
    pub fn get_trees_witness_trees(&self) -> Option<WitnessTrees> {
        if self.can_spend_from_all_pools() {
            Some(WitnessTrees::default())
        } else {
            None
        }
    }
    /// Returns a selection of pools where the wallet can view funds.
    pub fn can_view(&self) -> ReceiverSelection {
        // Not sure why this distintion which seems incompatible
        // with our ledger application.
        ReceiverSelection {
            orchard: false,
            sapling: true,
            transparent: true,
        }
    }

    fn read_version<R: Read>(reader: &mut R) -> io::Result<u8> {
        let version = reader.read_u8()?;

        Ok(version)
    }

    /// Retrieve the defualt diversifier from a given device and path
    ///
    /// The defualt diversifier is the first valid diversifier starting
    /// from index 0
    async fn get_default_diversifier_from(
        app: &ZcashApp<TransportNativeHID>,
        idx: u32,
    ) -> Result<Diversifier, LedgerError> {
        let mut index = DiversifierIndex::new();

        loop {
            let divs = app.get_div_list(idx, &index.as_bytes()).await?;
            let divs: &[[u8; 11]] = bytemuck::cast_slice(&divs);

            //find the first div that is not all 0s
            // all 0s is when it's an invalid diversifier
            for div in divs {
                if div != &[0; 11] {
                    return Ok(Diversifier(*div));
                }

                //increment the index for each diversifier returned
                index
                    .increment()
                    .map_err(|_| LedgerError::DiversifierIndexOverflow)?;
            }
        }
    }

    fn convert_path_to_num(path_str: &str, child_index_len: usize) -> Result<Vec<ChildIndex>, &'static str> {
        let path_parts: Vec<&str> = path_str.split('/').collect();
        let mut path = Vec::new();
    
        if path_parts.len() != child_index_len{
            return Err("invalid path");
        }
    
        for part in path_parts {
            let value = if part.ends_with('\'') {
                0x80000000 | part.trim_end_matches('\'').parse::<u32>().unwrap()
            } else {
                part.parse::<u32>().unwrap()
            };
            path.push(ChildIndex::from_index(value));
        }
    
        Ok(path)
    }


    fn convert_path_to_str(path_num: Vec<ChildIndex>) -> Result<String, &'static str> {
        let mut path = Vec::new();

        for part in path_num.into_iter() {
            let (value, is_hardened) = match part {
                ChildIndex::NonHardened(num) => (num, false),
                ChildIndex::Hardened(num) => (num, true)
            };

            let mut value: String = value.to_string();
            if is_hardened {
            value.push('\'');
            }

            path.push(value);
        }

        Ok(path.join("/"))
    }

    fn convert_path_slice_to_string(path_slice: [u32; 5]) -> String{
        let mut path_vec = Vec::new();
        for i in 0..path_slice.len() {
            path_vec.push( ChildIndex::from_index(path_slice[i]));
        }

        Self::convert_path_to_str(path_vec).unwrap()
    }
}

impl ReadableWriteable<()> for LedgerWalletCapability {
    const VERSION: u8 = 2;

    fn read<R: Read>(mut reader: R, _input: ()) -> io::Result<Self> {
        let version = Self::read_version(&mut reader)?;

        if version > Self::VERSION {
            let e = format!(
                "Don't know how to read ledger wallet version {}. Do you have the latest version?",
                version
            );
            return Err(io::Error::new(io::ErrorKind::InvalidData, e));
        }

        //retrieve the ledger id and verify it matches with the aocnnected device
        let ledger_id = {
            let mut buf = [0; secp256k1::constants::PUBLIC_KEY_SIZE];
            reader.read_exact(&mut buf)?;

            SecpPublicKey::from_slice(&buf).map_err(|e| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("Bad public key stored for ledger id: {:?}", e),
                )
            })?
        };

        let app = Self::connect_ledger()?;
        // lets used futures simpler executor
        if ledger_id != futures::executor::block_on(Self::get_id(&app))? {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Detected different ledger than used previously".to_string(),
            ));
        }

        //read the transparent paths
        // the keys will be retrieved one by one from the device
        let transparent_addrs_len = reader.read_u64::<LittleEndian>()?;
        let mut transparent_addrs = BTreeMap::new();
        for _ in 0..transparent_addrs_len {
            let path = {
                let mut buf = [0; 4 * 5];
                reader.read_exact(&mut buf)?;

                let path_bytes: [[u8; 4]; 5] = bytemuck::cast(buf);
                [
                    u32::from_le_bytes(path_bytes[0]),
                    u32::from_le_bytes(path_bytes[1]),
                    u32::from_le_bytes(path_bytes[2]),
                    u32::from_le_bytes(path_bytes[3]),
                    u32::from_le_bytes(path_bytes[4]),
                ]
            };

            let key =
                futures::executor::block_on(app.get_address_unshielded(&BIP44Path(path), false))
                    .map_err(LedgerError::Ledger)?
                    .public_key;
            let key = SecpPublicKey::from_slice(&key).map_err(|_| LedgerError::InvalidPublicKey)?;

            transparent_addrs.insert(path, key);
        }

        //read the transparent(maybe shielded)? paths
        // the keys and the diversifiers
        // will be retrieved one by one from the device
        let shielded_addrs_len = reader.read_u64::<LittleEndian>()?;
        let mut shielded_addrs = BTreeMap::new();
        for _ in 0..shielded_addrs_len {
            let path = {
                let mut buf = [0; 4 * 3];
                reader.read_exact(&mut buf)?;

                let path_bytes: [[u8; 4]; 3] = bytemuck::cast(buf);
                [
                    u32::from_le_bytes(path_bytes[0]),
                    u32::from_le_bytes(path_bytes[1]),
                    u32::from_le_bytes(path_bytes[2]),
                ]
            };

            //ZIP32 uses fixed path, so the actual index
            // is only the latest element
            let idx = path[2];

            let ivk = futures::executor::block_on(app.get_ivk(idx))
                .map_err(LedgerError::Ledger) // Convert any errors.
                .and_then(|ivk| {
                    let sa = jubjub::Fr::from_bytes(&ivk).map(SaplingIvk);
                    if sa.is_some().unwrap_u8() == 1 {
                        Ok(sa.unwrap())
                    } else {
                        Err(LedgerError::InvalidPublicKey)
                    }
                })?;

            let div = futures::executor::block_on(Self::get_default_diversifier_from(&app, idx))?;

            let ovk = futures::executor::block_on(app.get_ovk(idx))
                .map(OutgoingViewingKey)
                .map_err(LedgerError::Ledger)?;

            shielded_addrs.insert(path, (ivk, div, ovk));
        }
        // let receiver_selections = Vector::read(reader, |r| ReceiverSelection::read(r, ()))?;
        // let addresses = append_only_vec::AppendOnlyVec::new();
        let wc = Self {
            app,
            ledger_id,
            transparent_addrs: RwLock::new(transparent_addrs),
            shielded_addrs: RwLock::new(shielded_addrs),
            orchard: Capability::None,
            transparent: Capability::None,
            sapling: Capability::None,
            transparent_child_addresses: Arc::new(AppendOnlyVec::<(usize, TransparentAddress)>::new()),
            addresses: append_only_vec::AppendOnlyVec::new(),
            addresses_write_lock: AtomicBool::new(false),
        };

        // TODO: how to conciliate this? reader does not know anything about
        // configuration, but our ledger application requires almost full paths which uses
        // coin_type and account.
        // for rs in receiver_selections {
        //     wc.new_address(rs)
        //         .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        // }
        //
        Ok(wc)
    }

    fn write<W: Write>(&self, mut writer: W) -> io::Result<()> {
        writer.write_u8(Self::VERSION)?;
        let id = self.ledger_id.serialize();
        writer.write_all(&id)?;

        //write the transparent paths
        let transparent_paths = match self.transparent_addrs.try_read() {
            Ok(locked_addrs) => locked_addrs
                .keys()
                .map(|path| {
                    [
                        path[0].to_le_bytes(),
                        path[1].to_le_bytes(),
                        path[2].to_le_bytes(),
                        path[3].to_le_bytes(),
                        path[4].to_le_bytes(),
                    ]
                })
                .map(|path_bytes| bytemuck::cast(path_bytes))
                .collect::<Vec<[u8; 4 * 5]>>(),
            Err(_) => {
                // Return an empty vector if the lock cannot be acquired
                Vec::new()
            }
        };
        writer.write_u64::<LittleEndian>(transparent_paths.len() as u64)?;
        for path in transparent_paths {
            writer.write_all(&path)?;
        }

        //write the shielded paths
        let shielded_paths = match self.shielded_addrs.try_read() {
            Ok(locked_addrs) => locked_addrs
                .keys()
                .map(|path| {
                    [
                        path[0].to_le_bytes(),
                        path[1].to_le_bytes(),
                        path[2].to_le_bytes(),
                    ]
                })
                .map(|path_bytes| bytemuck::cast::<_, [u8; 4 * 3]>(path_bytes))
                .collect::<Vec<[u8; 4 * 3]>>(),
            Err(_) => {
                // Return an empty vector if the lock cannot be acquired
                Vec::new()
            }
        };

        writer.write_u64::<LittleEndian>(shielded_paths.len() as u64)?;
        for path in shielded_paths {
            writer.write_all(&path)?;
        }

        Ok(())
    }
}

// TODO: Incompatible secrets must not live ledger device
impl TryFrom<&LedgerWalletCapability> for super::extended_transparent::ExtendedPrivKey {
    type Error = String;
    fn try_from(_wc: &LedgerWalletCapability) -> Result<Self, String> {
        unimplemented!()
    }
}

// TODO: Incompatible secrets must not live ledger device
impl TryFrom<&LedgerWalletCapability> for sapling_crypto::zip32::ExtendedSpendingKey {
    type Error = String;
    fn try_from(_wc: &LedgerWalletCapability) -> Result<Self, String> {
        unimplemented!()
    }
}

// TODO: Incompatible secrets must not live ledger device
impl TryFrom<&LedgerWalletCapability> for orchard::keys::SpendingKey {
    type Error = String;
    fn try_from(_wc: &LedgerWalletCapability) -> Result<Self, String> {
        // Ledger application does not support orchard!
        unimplemented!()
    }
}

impl TryFrom<&LedgerWalletCapability> for super::extended_transparent::ExtendedPubKey {
    type Error = String;
    fn try_from(_wc: &LedgerWalletCapability) -> Result<Self, String> {
        unimplemented!()
    }
}

impl TryFrom<&LedgerWalletCapability> for orchard::keys::FullViewingKey {
    type Error = String;
    fn try_from(_wc: &LedgerWalletCapability) -> Result<Self, String> {
        unimplemented!()
    }
}

impl TryFrom<&LedgerWalletCapability> for sapling_crypto::zip32::DiversifiableFullViewingKey {
    type Error = String;
    fn try_from(_wc: &LedgerWalletCapability) -> Result<Self, String> {
        unimplemented!()
    }
}

impl TryFrom<&LedgerWalletCapability>
    for sapling_crypto::note_encryption::PreparedIncomingViewingKey
{
    type Error = String;

    fn try_from(_value: &LedgerWalletCapability) -> Result<Self, Self::Error> {
        unimplemented!()
    }
}

// TODO: Orchard is not supported by ledger application
impl TryFrom<&LedgerWalletCapability> for orchard::keys::IncomingViewingKey {
    type Error = String;
    fn try_from(_wc: &LedgerWalletCapability) -> Result<Self, String> {
        // Ledger application does not support orchard!
        unimplemented!()
    }
}

// TODO: Orchard is not supported by ledger application
impl TryFrom<&LedgerWalletCapability> for orchard::keys::PreparedIncomingViewingKey {
    type Error = String;
    fn try_from(_wc: &LedgerWalletCapability) -> Result<Self, String> {
        unimplemented!()
    }
}

// TODO: Ledger application supports ivk but we need a BIP44Path
// somehow we need to pass it so that in here we just call self.get_ivk()
impl TryFrom<&LedgerWalletCapability> for sapling_crypto::SaplingIvk {
    type Error = String;
    fn try_from(_wc: &LedgerWalletCapability) -> Result<Self, String> {
        unimplemented!()
    }
}

// TODO: Orchard is not supported by ledger application
impl TryFrom<&LedgerWalletCapability> for orchard::keys::OutgoingViewingKey {
    type Error = String;
    fn try_from(_wc: &LedgerWalletCapability) -> Result<Self, String> {
        unimplemented!()
    }
}

// TODO: Ledger application supports ovk but we need a BIP44Path
// somehow we need to pass it so that in here we just call self.get_ovk()
impl TryFrom<&LedgerWalletCapability> for sapling_crypto::keys::OutgoingViewingKey {
    type Error = String;
    fn try_from(_wc: &LedgerWalletCapability) -> Result<Self, String> {
        unimplemented!()
    }
}