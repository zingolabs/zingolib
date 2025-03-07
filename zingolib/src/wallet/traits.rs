//! Provides unifying interfaces for transaction management across Sapling and Orchard
use std::io::{self, Read, Write};

use crate::config::ChainType;
use crate::wallet::data::{COMMITMENT_TREE_LEVELS, MAX_SHARD_LEVEL};
use byteorder::{LittleEndian, ReadBytesExt, WriteBytesExt};
use incrementalmerkletree::{witness::IncrementalWitness, Hashable, Level, Position};
use nonempty::NonEmpty;
use orchard::{
    note_encryption::{CompactAction, OrchardDomain},
    primitives::redpallas::{Signature, SpendAuth},
    tree::MerkleHashOrchard,
    Action,
};
use sapling_crypto::{bundle::GrothProofBytes, note_encryption::SaplingDomain};
use shardtree::store::memory::MemoryShardStore;
use shardtree::ShardTree;
use subtle::CtOption;
use zcash_address::unified::{self, Receiver};
use zcash_client_backend::{
    address::UnifiedAddress,
    encoding::encode_payment_address,
    proto::{
        compact_formats::{CompactOrchardAction, CompactSaplingOutput, CompactTx},
        service::TreeState,
    },
    ShieldedProtocol,
};
use zcash_encoding::{Optional, Vector};
use zcash_note_encryption::{
    BatchDomain, Domain, EphemeralKeyBytes, ShieldedOutput, COMPACT_NOTE_SIZE, ENC_CIPHERTEXT_SIZE,
};
use zcash_primitives::{
    consensus::{BlockHeight, NetworkConstants, NetworkUpgrade, Parameters},
    memo::{Memo, MemoBytes},
    merkle_tree::read_incremental_witness,
    transaction::{
        components::{Amount, OutputDescription, SpendDescription},
        Transaction, TxId,
    },
};
use zingo_status::confirmation_status::ConfirmationStatus;

use super::keys::unified::UnifiedKeyStore;
use super::legacy::PoolNullifier;

/// This provides a uniform `.to_bytes` to types that might require it in a generic context.
pub trait ToBytes<const N: usize> {
    /// TODO: Add Doc Comment Here!
    fn to_bytes(&self) -> [u8; N];
}

impl ToBytes<32> for sapling_crypto::Nullifier {
    fn to_bytes(&self) -> [u8; 32] {
        self.0
    }
}

impl ToBytes<32> for orchard::note::Nullifier {
    fn to_bytes(&self) -> [u8; 32] {
        orchard::note::Nullifier::to_bytes(*self)
    }
}

impl ToBytes<11> for sapling_crypto::Diversifier {
    fn to_bytes(&self) -> [u8; 11] {
        self.0
    }
}

impl ToBytes<11> for orchard::keys::Diversifier {
    fn to_bytes(&self) -> [u8; 11] {
        *self.as_array()
    }
}

impl ToBytes<512> for Memo {
    fn to_bytes(&self) -> [u8; 512] {
        *self.encode().as_array()
    }
}

impl ToBytes<512> for MemoBytes {
    fn to_bytes(&self) -> [u8; 512] {
        *self.as_array()
    }
}

impl<const N: usize> ToBytes<N> for [u8; N] {
    fn to_bytes(&self) -> [u8; N] {
        *self
    }
}

/// Provides a standard `from_bytes` interface to be used generically
pub trait FromBytes<const N: usize> {
    /// TODO: Add Doc Comment Here!
    fn from_bytes(bytes: [u8; N]) -> Self;
}

impl FromBytes<32> for sapling_crypto::Nullifier {
    fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }
}

impl FromBytes<32> for orchard::note::Nullifier {
    fn from_bytes(bytes: [u8; 32]) -> Self {
        Option::from(orchard::note::Nullifier::from_bytes(&bytes))
            .unwrap_or_else(|| panic!("Invalid nullifier {:?}", bytes))
    }
}

impl FromBytes<11> for sapling_crypto::Diversifier {
    fn from_bytes(bytes: [u8; 11]) -> Self {
        sapling_crypto::Diversifier(bytes)
    }
}

impl FromBytes<11> for orchard::keys::Diversifier {
    fn from_bytes(bytes: [u8; 11]) -> Self {
        orchard::keys::Diversifier::from_bytes(bytes)
    }
}

/// TODO: Add Doc Comment Here!
pub trait FromCommitment
where
    Self: Sized,
{
    /// TODO: Add Doc Comment Here!
    fn from_commitment(from: &[u8; 32]) -> CtOption<Self>;
}

impl FromCommitment for sapling_crypto::Node {
    fn from_commitment(from: &[u8; 32]) -> CtOption<Self> {
        let maybe_node =
            <sapling_crypto::Node as zcash_primitives::merkle_tree::HashSer>::read(from.as_slice());
        match maybe_node {
            Ok(node) => CtOption::new(node, subtle::Choice::from(1)),
            Err(_) => CtOption::new(Self::empty_root(Level::from(0)), subtle::Choice::from(0)),
        }
    }
}
impl FromCommitment for MerkleHashOrchard {
    fn from_commitment(from: &[u8; 32]) -> CtOption<Self> {
        Self::from_bytes(from)
    }
}

impl From<orchard::note::Nullifier> for PoolNullifier {
    fn from(n: orchard::note::Nullifier) -> Self {
        PoolNullifier::Orchard(n)
    }
}

impl From<sapling_crypto::Nullifier> for PoolNullifier {
    fn from(n: sapling_crypto::Nullifier) -> Self {
        PoolNullifier::Sapling(n)
    }
}

///  Recipients provide the means to generate a Receiver.  A Receiver contains the information necessary
///  to transfer an asset to the generating Recipient.
///  <https://zips.z.cash/zip-0316#terminology>
pub trait Recipient {
    /// TODO: Add Doc Comment Here!
    type Diversifier: Copy;

    /// TODO: Add Doc Comment Here!
    fn diversifier(&self) -> Self::Diversifier;

    /// TODO: Add Doc Comment Here!
    fn b32encode_for_network(&self, chain: &ChainType) -> String;
}

impl Recipient for orchard::Address {
    type Diversifier = orchard::keys::Diversifier;

    fn diversifier(&self) -> Self::Diversifier {
        orchard::Address::diversifier(self)
    }

    fn b32encode_for_network(&self, chain: &ChainType) -> String {
        unified::Encoding::encode(
            &<unified::Address as unified::Encoding>::try_from_items(vec![Receiver::Orchard(
                self.to_raw_address_bytes(),
            )])
            .expect("Could not create UA from orchard address"),
            &chain.network_type(),
        )
    }
}

impl Recipient for sapling_crypto::PaymentAddress {
    type Diversifier = sapling_crypto::Diversifier;

    fn diversifier(&self) -> Self::Diversifier {
        *sapling_crypto::PaymentAddress::diversifier(self)
    }

    fn b32encode_for_network(&self, chain: &ChainType) -> String {
        encode_payment_address(chain.hrp_sapling_payment_address(), self)
    }
}

fn slice_to_array<const N: usize>(slice: &[u8]) -> &[u8; N] {
    <&[u8; N]>::try_from(slice).unwrap_or(&[0; N])
    //todo: This default feels dangerous. Find better solution
}

type MemoryStoreShardTree<T> =
    ShardTree<MemoryShardStore<T, BlockHeight>, COMMITMENT_TREE_LEVELS, MAX_SHARD_LEVEL>;

/// TODO: Add Doc Comment Here!
pub trait ReadableWriteable<ReadInput = (), WriteInput = ()>: Sized {
    /// TODO: Add Doc Comment Here!
    const VERSION: u8;

    /// TODO: Add Doc Comment Here!
    fn read<R: Read>(reader: R, input: ReadInput) -> io::Result<Self>;

    /// TODO: Add Doc Comment Here!
    fn write<W: Write>(&self, writer: W, input: WriteInput) -> io::Result<()>;

    /// TODO: Add Doc Comment Here!
    fn get_version<R: Read>(mut reader: R) -> io::Result<u8> {
        let external_version = reader.read_u8()?;
        if external_version > Self::VERSION {
            Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "Wallet file version \"{}\" is from future version of zingo",
                    external_version,
                ),
            ))
        } else {
            Ok(external_version)
        }
    }
}

impl ReadableWriteable for sapling_crypto::zip32::ExtendedSpendingKey {
    const VERSION: u8 = 0; //Not applicable

    fn read<R: Read>(reader: R, _input: ()) -> io::Result<Self> {
        Self::read(reader)
    }

    fn write<W: Write>(&self, writer: W, _input: ()) -> io::Result<()> {
        self.write(writer)
    }
}

impl ReadableWriteable for sapling_crypto::zip32::DiversifiableFullViewingKey {
    const VERSION: u8 = 0; //Not applicable

    fn read<R: Read>(mut reader: R, _input: ()) -> io::Result<Self> {
        let mut fvk_bytes = [0u8; 128];
        reader.read_exact(&mut fvk_bytes)?;
        sapling_crypto::zip32::DiversifiableFullViewingKey::from_bytes(&fvk_bytes).ok_or(
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "Couldn't read a Sapling Diversifiable Full Viewing Key",
            ),
        )
    }

    fn write<W: Write>(&self, mut writer: W, _input: ()) -> io::Result<()> {
        writer.write_all(&self.to_bytes())
    }
}

impl ReadableWriteable for orchard::keys::FullViewingKey {
    const VERSION: u8 = 0; //Not applicable

    fn read<R: Read>(reader: R, _input: ()) -> io::Result<Self> {
        Self::read(reader)
    }

    fn write<W: Write>(&self, writer: W, _input: ()) -> io::Result<()> {
        self.write(writer)
    }
}
