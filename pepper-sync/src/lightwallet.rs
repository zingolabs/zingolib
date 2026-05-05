use incrementalmerkletree::frontier::CommitmentTree;
use zcash_client_backend::data_api::chain::ChainState;
use zcash_note_encryption::EphemeralKeyBytes;
use zcash_primitives::{block::BlockHash, merkle_tree::read_commitment_tree, transaction::TxId};
use zcash_protocol::consensus::BlockHeight;

pub(crate) use lightwallet_protocol::*;

pub(crate) trait CompactBlockExt {
    fn hash(&self) -> BlockHash;
    fn height(&self) -> BlockHeight;
    fn prev_hash(&self) -> BlockHash;
}

impl CompactBlockExt for CompactBlock {
    fn hash(&self) -> BlockHash {
        BlockHash::from_slice(&self.hash)
    }

    fn height(&self) -> BlockHeight {
        self.height
            .try_into()
            .expect("block height must fit in u32")
    }

    fn prev_hash(&self) -> BlockHash {
        BlockHash::from_slice(&self.prev_hash)
    }
}

pub(crate) trait CompactTxExt {
    fn txid(&self) -> TxId;
}

impl CompactTxExt for CompactTx {
    fn txid(&self) -> TxId {
        let txid: [u8; 32] = self
            .txid
            .as_slice()
            .try_into()
            .expect("txid must be 32 bytes");
        TxId::from_bytes(txid)
    }
}

pub(crate) trait TreeStateExt {
    fn to_chain_state(&self) -> std::io::Result<ChainState>;
}

impl TreeStateExt for TreeState {
    fn to_chain_state(&self) -> std::io::Result<ChainState> {
        let mut hash_bytes = hex::decode(&self.hash).map_err(|e| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("Block hash is not valid hex: {e:?}"),
            )
        })?;
        hash_bytes.reverse();

        Ok(ChainState::new(
            self.height.try_into().map_err(|_| {
                std::io::Error::new(std::io::ErrorKind::InvalidData, "Invalid block height")
            })?,
            BlockHash::try_from_slice(&hash_bytes).ok_or_else(|| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "Invalid block hash length.",
                )
            })?,
            sapling_tree(self)?.to_frontier(),
            orchard_tree(self)?.to_frontier(),
        ))
    }
}

fn sapling_tree(
    tree_state: &TreeState,
) -> std::io::Result<
    CommitmentTree<sapling_crypto::Node, { sapling_crypto::NOTE_COMMITMENT_TREE_DEPTH }>,
> {
    if tree_state.sapling_tree.is_empty() {
        Ok(CommitmentTree::empty())
    } else {
        let tree_bytes = hex::decode(&tree_state.sapling_tree).map_err(|e| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("Hex decoding of Sapling tree bytes failed: {e:?}"),
            )
        })?;
        read_commitment_tree::<
            sapling_crypto::Node,
            _,
            { sapling_crypto::NOTE_COMMITMENT_TREE_DEPTH },
        >(&tree_bytes[..])
    }
}

fn orchard_tree(
    tree_state: &TreeState,
) -> std::io::Result<
    CommitmentTree<orchard::tree::MerkleHashOrchard, { orchard::NOTE_COMMITMENT_TREE_DEPTH as u8 }>,
> {
    if tree_state.orchard_tree.is_empty() {
        Ok(CommitmentTree::empty())
    } else {
        let tree_bytes = hex::decode(&tree_state.orchard_tree).map_err(|e| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("Hex decoding of Orchard tree bytes failed: {e:?}"),
            )
        })?;
        read_commitment_tree::<
            orchard::tree::MerkleHashOrchard,
            _,
            { orchard::NOTE_COMMITMENT_TREE_DEPTH as u8 },
        >(&tree_bytes[..])
    }
}

pub(crate) fn sapling_output_description(
    output: &CompactSaplingOutput,
) -> Result<sapling_crypto::note_encryption::CompactOutputDescription, ()> {
    Ok(sapling_crypto::note_encryption::CompactOutputDescription {
        cmu: sapling_cmu(output)?,
        ephemeral_key: ephemeral_key(&output.ephemeral_key)?,
        enc_ciphertext: output.ciphertext[..].try_into().map_err(|_| ())?,
    })
}

fn sapling_cmu(
    output: &CompactSaplingOutput,
) -> Result<sapling_crypto::note::ExtractedNoteCommitment, ()> {
    let repr: [u8; 32] = output.cmu[..].try_into().map_err(|_| ())?;
    Option::from(sapling_crypto::note::ExtractedNoteCommitment::from_bytes(
        &repr,
    ))
    .ok_or(())
}

pub(crate) fn orchard_compact_action(
    action: &CompactOrchardAction,
) -> Result<orchard::note_encryption::CompactAction, ()> {
    Ok(orchard::note_encryption::CompactAction::from_parts(
        orchard_nf(action)?,
        orchard_cmx(action)?,
        ephemeral_key(&action.ephemeral_key)?,
        action.ciphertext[..].try_into().map_err(|_| ())?,
    ))
}

fn orchard_cmx(
    action: &CompactOrchardAction,
) -> Result<orchard::note::ExtractedNoteCommitment, ()> {
    let cmx: [u8; 32] = action.cmx[..].try_into().map_err(|_| ())?;
    Option::from(orchard::note::ExtractedNoteCommitment::from_bytes(&cmx)).ok_or(())
}

fn orchard_nf(action: &CompactOrchardAction) -> Result<orchard::note::Nullifier, ()> {
    let nf: [u8; 32] = action.nullifier[..].try_into().map_err(|_| ())?;
    Option::from(orchard::note::Nullifier::from_bytes(&nf)).ok_or(())
}

fn ephemeral_key(bytes: &[u8]) -> Result<EphemeralKeyBytes, ()> {
    bytes.try_into().map(EphemeralKeyBytes).map_err(|_| ())
}
