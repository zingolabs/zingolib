use std::io::{self, ErrorKind, Read, Write};

use byteorder::{LittleEndian, ReadBytesExt, WriteBytesExt};
use orchard::keys::{FullViewingKey, IncomingViewingKey, OutgoingViewingKey, Scope, SpendingKey};
use zcash_client_backend::address::UnifiedAddress;
use zcash_encoding::{Optional, Vector};
// A struct that holds orchard private keys or view keys
#[derive(Clone, Debug, PartialEq)]
pub struct OrchardKey {
    pub(crate) key: WalletOKeyInner,
    locked: bool,
    pub(crate) unified_address: UnifiedAddress,

    // If this is a key derived from our HD seed, the account number of the key
    // This is effectively the index number used to generate the key from the seed
    pub(crate) hdkey_num: Option<u32>,

    // If locked, the encrypted private key is stored here
    enc_key: Option<Vec<u8>>,
    nonce: Option<Vec<u8>>,
}

impl OrchardKey {
    fn serialized_version() -> u8 {
        0
    }
    pub fn new_imported_osk(key: SpendingKey) -> Self {
        Self {
            key: WalletOKeyInner::ImportedSpendingKey(key),
            locked: false,
            unified_address: UnifiedAddress::from_receivers(
                Some(FullViewingKey::from(&key).address_at(0u32, Scope::External)),
                None,
                None,
            )
            .unwrap(),
            hdkey_num: None,
            enc_key: None,
            nonce: None,
        }
    }

    pub fn write<W: Write>(&self, mut out: W) -> io::Result<()> {
        out.write_u8(Self::serialized_version())?;

        out.write_u8(self.locked as u8)?;

        match &self.key {
            WalletOKeyInner::HdKey(key) => {
                out.write_u8(0)?;
                out.write_all(key.to_bytes())?
            }
            WalletOKeyInner::ImportedSpendingKey(key) => {
                out.write_u8(1)?;
                out.write_all(key.to_bytes())?
            }
            WalletOKeyInner::ImportedFullViewKey(key) => {
                out.write_u8(2)?;
                out.write_all(&key.to_bytes())?
            }
            WalletOKeyInner::ImportedInViewKey(_) => todo!(),
            WalletOKeyInner::ImportedOutViewKey(_) => todo!(),
        }

        Optional::write(&mut out, self.hdkey_num, |o, n| {
            o.write_u32::<LittleEndian>(n)
        })?;

        // Write enc_key
        Optional::write(&mut out, self.enc_key.as_ref(), |o, v| {
            Vector::write(o, &v, |o, n| o.write_u8(*n))
        })?;

        // Write nonce
        Optional::write(&mut out, self.nonce.as_ref(), |o, v| {
            Vector::write(o, &v, |o, n| o.write_u8(*n))
        })
    }

    pub fn read<R: Read>(mut inp: R) -> io::Result<Self> {
        let version = inp.read_u8()?;
        assert!(version <= Self::serialized_version());

        let locked = inp.read_u8()? > 0;

        let key_type = inp.read_u8()?;
        let key: WalletOKeyInner = match key_type {
            0 => {
                let mut key_bytes = [0; 32];
                inp.read_exact(&mut key_bytes)?;
                Ok(WalletOKeyInner::HdKey(
                    SpendingKey::from_bytes(key_bytes).unwrap(),
                ))
            }
            1 => {
                let mut key_bytes = [0; 32];
                inp.read_exact(&mut key_bytes)?;
                Ok(WalletOKeyInner::ImportedSpendingKey(
                    SpendingKey::from_bytes(key_bytes).unwrap(),
                ))
            }
            2 => {
                let mut key_bytes = [0; 96];
                inp.read_exact(&mut key_bytes)?;
                Ok(WalletOKeyInner::ImportedFullViewKey(
                    FullViewingKey::from_bytes(&key_bytes).unwrap(),
                ))
            }
            n => Err(io::Error::new(
                ErrorKind::InvalidInput,
                format!("Unknown zkey type {}", n),
            )),
        }?;

        let unified_address = UnifiedAddress::from_receivers(
            Some(
                FullViewingKey::try_from(&key)
                    .expect("We don't support import/export of ivks/ovks yet")
                    .address_at(0u32, Scope::External),
            ),
            None,
            None,
        )
        .unwrap();

        let hdkey_num = Optional::read(&mut inp, |r| r.read_u32::<LittleEndian>())?;

        let enc_key = Optional::read(&mut inp, |r| Vector::read(r, |r| r.read_u8()))?;
        let nonce = Optional::read(&mut inp, |r| Vector::read(r, |r| r.read_u8()))?;

        Ok(OrchardKey {
            key,
            locked,
            hdkey_num,
            enc_key,
            nonce,
            unified_address,
        })
    }
}

#[allow(dead_code)]
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub(crate) enum WalletOKeyInner {
    HdKey(SpendingKey),
    ImportedSpendingKey(SpendingKey),
    ImportedFullViewKey(FullViewingKey),
    ImportedInViewKey(IncomingViewingKey),
    ImportedOutViewKey(OutgoingViewingKey),
}

impl TryFrom<&WalletOKeyInner> for SpendingKey {
    type Error = String;
    fn try_from(key: &WalletOKeyInner) -> Result<SpendingKey, String> {
        match key {
            WalletOKeyInner::HdKey(k) => Ok(*k),
            WalletOKeyInner::ImportedSpendingKey(k) => Ok(*k),
            other => Err(format!("{other:?} is not a spending key")),
        }
    }
}
impl TryFrom<&WalletOKeyInner> for FullViewingKey {
    type Error = String;
    fn try_from(key: &WalletOKeyInner) -> Result<FullViewingKey, String> {
        match key {
            WalletOKeyInner::HdKey(k) => Ok(FullViewingKey::from(k)),
            WalletOKeyInner::ImportedSpendingKey(k) => Ok(FullViewingKey::from(k)),
            WalletOKeyInner::ImportedFullViewKey(k) => Ok(k.clone()),
            other => Err(format!("{other:?} is not a full viewing key")),
        }
    }
}
impl TryFrom<&WalletOKeyInner> for OutgoingViewingKey {
    type Error = String;
    fn try_from(key: &WalletOKeyInner) -> Result<OutgoingViewingKey, String> {
        match key {
            WalletOKeyInner::ImportedOutViewKey(k) => Ok(k.clone()),
            WalletOKeyInner::ImportedFullViewKey(k) => Ok(k.to_ovk(Scope::External)),
            WalletOKeyInner::ImportedInViewKey(k) => {
                Err(format!("Received ivk {k:?} which does not contain an ovk"))
            }
            _ => Ok(FullViewingKey::try_from(key)
                .unwrap()
                .to_ovk(Scope::External)),
        }
    }
}

impl TryFrom<&WalletOKeyInner> for IncomingViewingKey {
    type Error = String;
    fn try_from(key: &WalletOKeyInner) -> Result<IncomingViewingKey, String> {
        match key {
            WalletOKeyInner::ImportedInViewKey(k) => Ok(k.clone()),
            WalletOKeyInner::ImportedFullViewKey(k) => Ok(k.to_ivk(Scope::External)),
            WalletOKeyInner::ImportedOutViewKey(k) => {
                Err(format!("Received ovk {k:?} which does not contain an ivk"))
            }
            _ => Ok(FullViewingKey::try_from(key)
                .unwrap()
                .to_ivk(Scope::External)),
        }
    }
}

impl PartialEq for WalletOKeyInner {
    fn eq(&self, other: &Self) -> bool {
        use subtle::ConstantTimeEq as _;
        use WalletOKeyInner::*;
        match (self, other) {
            (HdKey(a), HdKey(b)) => bool::from(a.ct_eq(b)),
            (ImportedSpendingKey(a), ImportedSpendingKey(b)) => bool::from(a.ct_eq(b)),
            (ImportedFullViewKey(a), ImportedFullViewKey(b)) => a == b,
            (ImportedInViewKey(a), ImportedInViewKey(b)) => a == b,
            (ImportedOutViewKey(a), ImportedOutViewKey(b)) => a.as_ref() == b.as_ref(),
            _ => false,
        }
    }
}
impl OrchardKey {
    pub fn new_hdkey(hdkey_num: u32, spending_key: SpendingKey) -> Self {
        let key = WalletOKeyInner::HdKey(spending_key);
        let address = FullViewingKey::from(&spending_key).address_at(0u64, Scope::External);
        let unified_address = UnifiedAddress::from_receivers(Some(address), None, None).unwrap();

        OrchardKey {
            key,
            locked: false,
            unified_address,
            hdkey_num: Some(hdkey_num),
            enc_key: None,
            nonce: None,
        }
    }
}
