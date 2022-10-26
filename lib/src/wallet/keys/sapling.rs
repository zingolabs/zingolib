use std::io::{self, Read, Write};
use std::io::{Error, ErrorKind};

use byteorder::{LittleEndian, ReadBytesExt, WriteBytesExt};

use sodiumoxide::crypto::secretbox;

use zcash_encoding::{Optional, Vector};
use zcash_primitives::{
    sapling::PaymentAddress,
    zip32::{ExtendedFullViewingKey, ExtendedSpendingKey},
};

use zingoconfig::ZingoConfig;

use super::get_zaddr_from_bip39seed;

#[derive(PartialEq, Debug, Clone)]
pub enum WalletZKeyType {
    HdKey = 0,
    ImportedSpendingKey = 1,
    ImportedViewKey = 2,
}

// A struct that holds z-address private keys or view keys
#[derive(Clone, Debug, PartialEq)]
pub struct SaplingKey {
    pub(crate) keytype: WalletZKeyType,
    locked: bool,
    pub(crate) extsk: Option<ExtendedSpendingKey>,
    pub(crate) extfvk: ExtendedFullViewingKey,
    pub(crate) zaddress: PaymentAddress,

    // If this is a HD key, what is the key number
    pub(crate) hdkey_num: Option<u32>,

    // If locked, the encrypted private key is stored here
    enc_key: Option<Vec<u8>>,
    nonce: Option<Vec<u8>>,
}

impl SaplingKey {
    pub fn new_hdkey(hdkey_num: u32, extsk: ExtendedSpendingKey) -> Self {
        let extfvk = ExtendedFullViewingKey::from(&extsk);
        let zaddress = extfvk.default_address().1;

        SaplingKey {
            keytype: WalletZKeyType::HdKey,
            locked: false,
            extsk: Some(extsk),
            extfvk,
            zaddress,
            hdkey_num: Some(hdkey_num),
            enc_key: None,
            nonce: None,
        }
    }

    pub fn new_locked_hdkey(hdkey_num: u32, extfvk: ExtendedFullViewingKey) -> Self {
        let zaddress = extfvk.default_address().1;

        SaplingKey {
            keytype: WalletZKeyType::HdKey,
            locked: true,
            extsk: None,
            extfvk,
            zaddress,
            hdkey_num: Some(hdkey_num),
            enc_key: None,
            nonce: None,
        }
    }

    pub fn new_imported_sk(extsk: ExtendedSpendingKey) -> Self {
        let extfvk = ExtendedFullViewingKey::from(&extsk);
        let zaddress = extfvk.default_address().1;

        SaplingKey {
            keytype: WalletZKeyType::ImportedSpendingKey,
            locked: false,
            extsk: Some(extsk),
            extfvk,
            zaddress,
            hdkey_num: None,
            enc_key: None,
            nonce: None,
        }
    }

    pub fn new_imported_viewkey(extfvk: ExtendedFullViewingKey) -> Self {
        let zaddress = extfvk.default_address().1;

        SaplingKey {
            keytype: WalletZKeyType::ImportedViewKey,
            locked: false,
            extsk: None,
            extfvk,
            zaddress,
            hdkey_num: None,
            enc_key: None,
            nonce: None,
        }
    }

    pub fn have_sapling_spending_key(&self) -> bool {
        self.extsk.is_some() || self.enc_key.is_some() || self.hdkey_num.is_some()
    }

    pub fn extfvk(&self) -> &'_ ExtendedFullViewingKey {
        &self.extfvk
    }

    pub fn lock(&mut self) -> io::Result<()> {
        match self.keytype {
            WalletZKeyType::HdKey => {
                // For HD keys, just empty out the keys, since they will be reconstructed from the hdkey_num
                self.extsk = None;
                self.locked = true;
            }
            WalletZKeyType::ImportedSpendingKey => {
                // For imported keys, encrypt the key into enckey
                // assert that we have the encrypted key.
                if self.enc_key.is_none() {
                    return Err(Error::new(
                        ErrorKind::InvalidInput,
                        "Can't lock when imported key is not encrypted",
                    ));
                }
                self.extsk = None;
                self.locked = true;
            }
            WalletZKeyType::ImportedViewKey => {
                // For viewing keys, there is nothing to lock, so just return true
                self.locked = true;
            }
        }

        Ok(())
    }

    pub fn unlock(
        &mut self,
        config: &ZingoConfig,
        bip39_seed: &[u8],
        key: &secretbox::Key,
    ) -> io::Result<()> {
        match self.keytype {
            WalletZKeyType::HdKey => {
                let (extsk, extfvk, address) =
                    get_zaddr_from_bip39seed(&config, &bip39_seed, self.hdkey_num.unwrap());

                if address != self.zaddress {
                    return Err(io::Error::new(
                        ErrorKind::InvalidData,
                        format!(
                            "zaddress mismatch at {}. {:?} vs {:?}",
                            self.hdkey_num.unwrap(),
                            address,
                            self.zaddress
                        ),
                    ));
                }

                if extfvk != self.extfvk {
                    return Err(io::Error::new(
                        ErrorKind::InvalidData,
                        format!(
                            "fvk mismatch at {}. {:?} vs {:?}",
                            self.hdkey_num.unwrap(),
                            extfvk,
                            self.extfvk
                        ),
                    ));
                }

                self.extsk = Some(extsk);
            }
            WalletZKeyType::ImportedSpendingKey => {
                // For imported keys, we need to decrypt from the encrypted key
                let nonce = secretbox::Nonce::from_slice(&self.nonce.as_ref().unwrap()).unwrap();
                let extsk_bytes =
                    match secretbox::open(&self.enc_key.as_ref().unwrap(), &nonce, &key) {
                        Ok(s) => s,
                        Err(_) => {
                            return Err(io::Error::new(
                                ErrorKind::InvalidData,
                                "Decryption failed. Is your password correct?",
                            ));
                        }
                    };

                self.extsk = Some(ExtendedSpendingKey::read(&extsk_bytes[..])?);
            }
            WalletZKeyType::ImportedViewKey => {
                // Viewing key unlocking is basically a no op
            }
        };

        self.locked = false;
        Ok(())
    }
}
