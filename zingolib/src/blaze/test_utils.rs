use crate::compact_formats::{CompactBlock, CompactSaplingOutput, CompactTx};
use incrementalmerkletree::{frontier::CommitmentTree, witness::IncrementalWitness, Hashable};
use orchard::tree::MerkleHashOrchard;
use prost::Message;
use rand::{rngs::OsRng, RngCore};

use zcash_primitives::{
    block::BlockHash,
    merkle_tree::{write_incremental_witness, HashSer},
    sapling::{self, value::NoteValue, Note, Rseed},
    transaction::components::Amount,
    zip32::ExtendedSpendingKey,
};

// This function can be used by TestServerData, or other test code
// TODO: Replace with actual lightclient functionality
pub fn trees_from_cblocks(
    compactblock_list: &Vec<crate::blaze::test_utils::FakeCompactBlock>,
) -> (
    Vec<sapling::CommitmentTree>,
    Vec<CommitmentTree<MerkleHashOrchard, 32>>,
) {
    let mut sapling_trees = Vec::new();
    let mut orchard_trees = Vec::new();
    for fake_compact_block in compactblock_list {
        let mut sapling_tree = sapling_trees
            .last()
            .map(Clone::clone)
            .unwrap_or_else(CommitmentTree::empty);
        let mut orchard_tree = orchard_trees
            .last()
            .map(Clone::clone)
            .unwrap_or_else(CommitmentTree::empty);
        for compact_transaction in &fake_compact_block.block.vtx {
            update_trees_with_compact_transaction(
                &mut sapling_tree,
                &mut orchard_tree,
                compact_transaction,
            )
        }
        sapling_trees.push(sapling_tree);
        orchard_trees.push(orchard_tree);
    }
    (sapling_trees, orchard_trees)
}
pub fn random_u8_32() -> [u8; 32] {
    let mut b = [0u8; 32];
    OsRng.fill_bytes(&mut b);

    b
}

pub fn incw_to_string<Node: Hashable + HashSer>(
    inc_witness: &IncrementalWitness<Node, 32>,
) -> String {
    let mut b1 = vec![];
    write_incremental_witness(inc_witness, &mut b1).unwrap();
    hex::encode(b1)
}

pub fn node_to_string<Node: Hashable + HashSer>(n: &Node) -> String {
    let mut b1 = vec![];
    n.write(&mut b1).unwrap();
    hex::encode(b1)
}

use super::block_management_reorg_detection::update_trees_with_compact_transaction;

pub struct FakeCompactBlock {
    pub block: CompactBlock,
    pub height: u64,
}

impl FakeCompactBlock {
    pub fn new(height: u64, prev_hash: BlockHash) -> Self {
        // Create a fake Note for the account
        let mut rng = OsRng;

        let mut cb = CompactBlock {
            height,
            ..Default::default()
        };
        cb.hash.resize(32, 0);
        rng.fill_bytes(&mut cb.hash);

        cb.prev_hash.extend_from_slice(&prev_hash.0);

        Self { block: cb, height }
    }

    pub fn add_transactions(&mut self, compact_transactions: Vec<CompactTx>) {
        self.block.vtx.extend(compact_transactions);
    }

    // Add a new transaction into the block, paying the given address the amount.
    // Returns the nullifier of the new note.
    pub fn add_random_sapling_transaction(&mut self, num_outputs: usize) {
        let xsk_m = ExtendedSpendingKey::master(&[1u8; 32]);
        let dfvk = xsk_m.to_diversifiable_full_viewing_key();
        let fvk = dfvk;

        let to = fvk.default_address().1;
        let value = Amount::from_u64(1).unwrap();

        let mut compact_transaction = CompactTx {
            hash: random_u8_32().to_vec(),
            ..Default::default()
        };

        for _ in 0..num_outputs {
            // Create a fake Note for the account
            let note = Note::from_parts(
                to,
                NoteValue::from_raw(u64::try_from(value).expect("Represent as u64")),
                Rseed::AfterZip212(random_u8_32()),
            );

            // Create a fake CompactBlock containing the note
            let cout = CompactSaplingOutput {
                cmu: note.cmu().to_bytes().to_vec(),
                ..Default::default()
            };

            compact_transaction.outputs.push(cout);
        }

        self.block.vtx.push(compact_transaction);
    }

    pub fn as_bytes(&self) -> Vec<u8> {
        let mut b = vec![];
        self.block.encode(&mut b).unwrap();

        b
    }

    pub fn into_cb(self) -> CompactBlock {
        self.block
    }
}
