use std::{convert::TryInto, sync::Arc};

use crate::{
    compact_formats::{CompactBlock, CompactOutput, CompactSpend, CompactTx},
    lightclient::test_server::TestServerData,
    lightwallet::{data::BlockData, keys::ToBase58Check},
};
use ff::{Field, PrimeField};
use group::GroupEncoding;
use prost::Message;
use rand::{rngs::OsRng, RngCore};
use secp256k1::PublicKey;
use sha2::{Digest, Sha256};
use tokio::sync::RwLock;

use zcash_note_encryption::{EphemeralKeyBytes, NoteEncryption};
use zcash_primitives::{
    block::BlockHash,
    consensus::{BranchId, TestNetwork},
    constants::SPENDING_KEY_GENERATOR,
    keys::OutgoingViewingKey,
    legacy::{Script, TransparentAddress},
    memo::Memo,
    merkle_tree::{CommitmentTree, Hashable, IncrementalWitness, MerklePath},
    sapling::{
        note_encryption::{sapling_note_encryption, SaplingDomain},
        prover::TxProver,
        redjubjub::Signature,
        Diversifier, Node, Note, Nullifier, PaymentAddress, ProofGenerationKey, Rseed, ValueCommitment,
    },
    transaction::{
        components::{transparent, Amount, OutPoint, OutputDescription, TxIn, TxOut, GROTH_PROOF_SIZE},
        Transaction, TransactionData, TxId, TxVersion,
    },
    zip32::{ExtendedFullViewingKey, ExtendedSpendingKey},
};
use zcash_proofs::sapling::SaplingProvingContext;

pub fn random_u8_32() -> [u8; 32] {
    let mut b = [0u8; 32];
    OsRng.fill_bytes(&mut b);

    b
}

pub fn tree_to_string(tree: &CommitmentTree<Node>) -> String {
    let mut b1 = vec![];
    tree.write(&mut b1).unwrap();
    hex::encode(b1)
}

pub fn incw_to_string(inc_witness: &IncrementalWitness<Node>) -> String {
    let mut b1 = vec![];
    inc_witness.write(&mut b1).unwrap();
    hex::encode(b1)
}

pub fn node_to_string(n: &Node) -> String {
    let mut b1 = vec![];
    n.write(&mut b1).unwrap();
    hex::encode(b1)
}

pub fn list_all_witness_nodes(cb: &CompactBlock) -> Vec<Node> {
    let mut nodes = vec![];
    for transaction in &cb.vtx {
        for co in &transaction.outputs {
            nodes.push(Node::new(co.cmu().unwrap().into()))
        }
    }

    nodes
}

pub struct FakeTransaction {
    pub compact_transaction: CompactTx,
    pub td: TransactionData<zcash_primitives::transaction::Authorized>,
    pub taddrs_involved: Vec<String>,
}

use zcash_primitives::transaction::components::sapling;
fn sapling_bundle() -> Option<sapling::Bundle<sapling::Authorized>> {
    let authorization = sapling::Authorized {
        binding_sig: Signature::read(&vec![0u8; 64][..]).expect("Signature read error!"),
    };
    Some(sapling::Bundle {
        shielded_spends: vec![],
        shielded_outputs: vec![],
        value_balance: Amount::zero(),
        authorization,
    })
}
fn optional_transparent_bundle(include_tbundle: bool) -> Option<transparent::Bundle<transparent::Authorized>> {
    if include_tbundle {
        return Some(transparent::Bundle {
            vin: vec![],
            vout: vec![],
            authorization: transparent::Authorized,
        });
    } else {
        return None;
    }
}
impl FakeTransaction {
    pub fn new(with_transparent: bool) -> Self {
        let fake_transaction_data = TransactionData::from_parts(
            TxVersion::Sapling,
            BranchId::Sapling,
            0,
            0u32.into(),
            optional_transparent_bundle(with_transparent),
            None,
            sapling_bundle(),
            None,
        );
        Self {
            compact_transaction: CompactTx::default(),
            td: fake_transaction_data,
            taddrs_involved: vec![],
        }
    }

    // Add a dummy compact output with given value sending it to 'to', and encode
    // the output with the ovk if available
    fn add_sapling_output(&mut self, value: u64, ovk: Option<OutgoingViewingKey>, to: &PaymentAddress) -> Note {
        // Create a fake Note for the account
        let mut rng = OsRng;
        let note = Note {
            g_d: to.diversifier().g_d().unwrap(),
            pk_d: to.pk_d().clone(),
            value,
            rseed: Rseed::BeforeZip212(jubjub::Fr::random(rng)),
        };

        let mut encryptor: NoteEncryption<SaplingDomain<TestNetwork>> =
            sapling_note_encryption(ovk, note.clone(), to.clone(), Memo::default().into(), &mut rng);

        let mut rng = OsRng;
        let rcv = jubjub::Fr::random(&mut rng);
        let cv = ValueCommitment {
            value,
            randomness: rcv.clone(),
        };

        let cmu = note.cmu();
        let od = OutputDescription {
            cv: cv.commitment().into(),
            cmu: note.cmu(),
            ephemeral_key: EphemeralKeyBytes::from(encryptor.epk().to_bytes()),
            enc_ciphertext: encryptor.encrypt_note_plaintext(),
            out_ciphertext: encryptor.encrypt_outgoing_plaintext(&cv.commitment().into(), &cmu, &mut rng),
            zkproof: [0; GROTH_PROOF_SIZE],
        };

        let mut cmu = vec![];
        cmu.extend_from_slice(&note.cmu().to_repr());
        let mut epk = vec![];
        epk.extend_from_slice(&encryptor.epk().to_bytes());
        let enc_ciphertext = encryptor.encrypt_note_plaintext();

        // Create a fake CompactBlock containing the note
        let mut cout = CompactOutput::default();
        cout.cmu = cmu;
        cout.epk = epk;
        cout.ciphertext = enc_ciphertext[..52].to_vec();

        //self.td.sapling_bundle().unwrap().shielded_outputs.push(od);
        self.compact_transaction.outputs.push(cout);

        note
    }

    pub fn add_transaction_spending(
        &mut self,
        nf: &Nullifier,
        value: u64,
        ovk: &OutgoingViewingKey,
        to: &PaymentAddress,
    ) {
        let _ = self.add_sapling_output(value, Some(ovk.clone()), to);

        let mut cs = CompactSpend::default();
        cs.nf = nf.to_vec();
        self.compact_transaction.spends.push(cs);

        // We should be adding the nullifier to the full tx (tx.shielded_spends) as well, but we don't use it,
        // so we pretend it doen't exist :)
    }

    // Add a new transaction into the block, paying the given address the amount.
    // Returns the nullifier of the new note.
    pub fn add_transaction_paying(&mut self, extfvk: &ExtendedFullViewingKey, value: u64) -> Note {
        let to = extfvk.default_address().1;
        let note = self.add_sapling_output(value, None, &to);

        note
    }

    // Add a t output which will be paid to the given PubKey
    pub fn add_t_output(&mut self, pk: &PublicKey, taddr: String, value: u64) {
        let mut hash160 = ripemd160::Ripemd160::new();
        hash160.update(Sha256::digest(&pk.serialize()[..].to_vec()));

        let taddr_bytes = hash160.finalize();
        /*self.td
            .transparent_bundle()
            .expect("Construction should guarantee Some")
            .vout
            .push(TxOut {
                value: Amount::from_u64(value).unwrap(),
                script_pubkey: TransparentAddress::PublicKey(taddr_bytes.try_into().unwrap()).script(),
            });
        */
        self.taddrs_involved.push(taddr)
    }

    // Spend the given utxo
    pub fn add_t_input(&mut self, transaction_id: TxId, n: u32, taddr: String) {
        /*
        self.td
            .transparent_bundle()
            .expect("Depend on construction for Some.")
            .vin
            .push(TxIn {
                prevout: OutPoint::new(*(transaction_id.as_ref()), n),
                script_sig: Script { 0: vec![] },
                sequence: 0,
            });*/
        self.taddrs_involved.push(taddr);
    }

    pub fn into_transaction(mut self) -> (CompactTx, Transaction, Vec<String>) {
        let transaction = self.td.freeze().unwrap();
        self.compact_transaction.hash = Vec::from(*(transaction.txid().clone().as_ref()));

        (self.compact_transaction, transaction, self.taddrs_involved)
    }
}

pub struct FakeCompactBlock {
    pub block: CompactBlock,
    pub height: u64,
}

impl FakeCompactBlock {
    pub fn new(height: u64, prev_hash: BlockHash) -> Self {
        // Create a fake Note for the account
        let mut rng = OsRng;

        let mut cb = CompactBlock::default();

        cb.height = height;
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
    pub fn add_random_transaction(&mut self, num_outputs: usize) {
        let xsk_m = ExtendedSpendingKey::master(&[1u8; 32]);
        let extfvk = ExtendedFullViewingKey::from(&xsk_m);

        let to = extfvk.default_address().1;
        let value = Amount::from_u64(1).unwrap();

        let mut compact_transaction = CompactTx::default();
        compact_transaction.hash = random_u8_32().to_vec();

        for _ in 0..num_outputs {
            // Create a fake Note for the account
            let note = Note {
                g_d: to.diversifier().g_d().unwrap(),
                pk_d: to.pk_d().clone(),
                value: value.into(),
                rseed: Rseed::AfterZip212(random_u8_32()),
            };

            // Create a fake CompactBlock containing the note
            let mut cout = CompactOutput::default();
            cout.cmu = note.cmu().to_bytes().to_vec();

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

pub struct FakeCompactBlockList {
    pub blocks: Vec<FakeCompactBlock>,
    pub transactions: Vec<(Transaction, u64, Vec<String>)>,
    pub prev_hash: BlockHash,
    pub next_height: u64,
}

impl FakeCompactBlockList {
    pub fn new(len: u64) -> Self {
        let mut s = Self {
            blocks: vec![],
            transactions: vec![],
            prev_hash: BlockHash([0u8; 32]),
            next_height: 1,
        };

        s.add_blocks(len);

        s
    }

    pub async fn add_pending_sends(&mut self, data: &Arc<RwLock<TestServerData>>) {
        let sent_transactions = data.write().await.sent_transactions.split_off(0);

        for read_transaction in sent_transactions {
            let transaction = Transaction::read(
                &read_transaction.data[..],
                zcash_primitives::consensus::BranchId::Sapling,
            )
            .unwrap();
            let mut compact_transaction = CompactTx::default();

            for out in &transaction
                .sapling_bundle()
                .expect("surprise missing sapling bundle")
                .shielded_outputs
            {
                let mut epkv = vec![];
                for c in (*out.ephemeral_key.clone().as_ref()).iter() {
                    epkv.push(*c);
                }
                let mut cout = CompactOutput::default();
                cout.cmu = out.cmu.to_repr().to_vec();
                cout.epk = epkv;
                cout.ciphertext = out.enc_ciphertext[..52].to_vec();

                compact_transaction.outputs.push(cout);
            }

            for spend in &transaction
                .sapling_bundle()
                .expect("missing sapling bundle")
                .shielded_spends
            {
                let mut cs = CompactSpend::default();
                cs.nf = spend.nullifier.to_vec();

                compact_transaction.spends.push(cs);
            }

            let config = data.read().await.config.clone();
            let taddrs = transaction
                .transparent_bundle()
                .expect("missing transparent bundle")
                .vout
                .iter()
                .filter_map(|vout| {
                    if let Some(TransparentAddress::PublicKey(taddr_hash)) = vout.script_pubkey.address() {
                        let taddr = taddr_hash.to_base58check(&config.base58_pubkey_address(), &[]);
                        Some(taddr)
                    } else {
                        None
                    }
                })
                .collect::<Vec<_>>();

            let new_block_height = {
                let new_block = self.add_empty_block();
                compact_transaction.hash = transaction.txid().as_ref().to_vec();
                new_block.add_transactions(vec![compact_transaction]);
                new_block.height
            };
            self.transactions.push((transaction, new_block_height, taddrs));
        }
    }

    pub fn add_fake_transaction(&mut self, fake_transaction: FakeTransaction) -> (Transaction, u64) {
        let (compact_transaction, transaction, taddrs) = fake_transaction.into_transaction();

        let height = self.next_height;
        //self.transactions.push((transaction, height, taddrs));
        self.add_empty_block().add_transactions(vec![compact_transaction]);

        (transaction, height)
    }

    pub fn add_transaction_spending(
        &mut self,
        nf: &Nullifier,
        value: u64,
        ovk: &OutgoingViewingKey,
        to: &PaymentAddress,
    ) -> Transaction {
        let mut fake_transaction = FakeTransaction::new(false);
        fake_transaction.add_transaction_spending(nf, value, ovk, to);

        let (transaction, _) = self.add_fake_transaction(fake_transaction);

        transaction
    }

    // Add a new transaction into the block, paying the given address the amount.
    // Returns the nullifier of the new note.
    pub fn add_transaction_paying(&mut self, extfvk: &ExtendedFullViewingKey, value: u64) -> (Transaction, u64, Note) {
        let mut fake_transaction = FakeTransaction::new(false);
        let note = fake_transaction.add_transaction_paying(extfvk, value);

        let (transaction, height) = self.add_fake_transaction(fake_transaction);

        (transaction, height, note)
    }

    pub fn add_empty_block(&mut self) -> &'_ mut FakeCompactBlock {
        let newblk = FakeCompactBlock::new(self.next_height, self.prev_hash);
        self.next_height += 1;
        self.prev_hash = newblk.block.hash();

        self.blocks.push(newblk);
        self.blocks.last_mut().unwrap()
    }

    pub fn add_blocks(&mut self, len: u64) -> &mut Self {
        let nexth = self.next_height;

        for i in nexth..(nexth + len) {
            let mut b = FakeCompactBlock::new(i, self.prev_hash);

            self.next_height = i + 1;
            self.prev_hash = b.block.hash();

            // Add 2 transactions, each with some random Compact Outputs to this block
            for _ in 0..2 {
                b.add_random_transaction(2);
            }

            self.blocks.push(b);
        }

        self
    }

    pub fn into_blockdatas(&mut self) -> Vec<BlockData> {
        let blocks = self.blocks.drain(..).collect::<Vec<_>>();

        blocks
            .into_iter()
            .map(|fcb| BlockData::new(fcb.into_cb()))
            .rev()
            .collect()
    }

    pub fn into_compact_blocks(&mut self) -> Vec<CompactBlock> {
        let blocks = self.blocks.drain(..).collect::<Vec<_>>();

        blocks.into_iter().map(|fcb| fcb.block).rev().collect()
    }

    pub fn into_transactions(&mut self) -> Vec<(Transaction, u64, Vec<String>)> {
        self.transactions.drain(..).collect()
    }
}

pub struct FakeTransactionProver {}

impl TxProver for FakeTransactionProver {
    type SaplingProvingContext = SaplingProvingContext;

    fn new_sapling_proving_context(&self) -> Self::SaplingProvingContext {
        SaplingProvingContext::new()
    }

    fn spend_proof(
        &self,
        _compact_transaction: &mut Self::SaplingProvingContext,
        proof_generation_key: ProofGenerationKey,
        _diversifier: Diversifier,
        _rseed: Rseed,
        ar: jubjub::Fr,
        value: u64,
        _anchor: bls12_381::Scalar,
        _merkle_path: MerklePath<Node>,
    ) -> Result<
        (
            [u8; GROTH_PROOF_SIZE],
            jubjub::ExtendedPoint,
            zcash_primitives::sapling::redjubjub::PublicKey,
        ),
        (),
    > {
        let zkproof = [0u8; GROTH_PROOF_SIZE];

        let mut rng = OsRng;

        // We create the randomness of the value commitment
        let rcv = jubjub::Fr::random(&mut rng);
        let cv = ValueCommitment { value, randomness: rcv };
        // Compute value commitment
        let value_commitment: jubjub::ExtendedPoint = cv.commitment().into();

        let rk = zcash_primitives::sapling::redjubjub::PublicKey(proof_generation_key.ak.clone().into())
            .randomize(ar, SPENDING_KEY_GENERATOR);

        Ok((zkproof, value_commitment, rk))
    }

    fn output_proof(
        &self,
        _compact_transaction: &mut Self::SaplingProvingContext,
        _esk: jubjub::Fr,
        _payment_address: PaymentAddress,
        _rcm: jubjub::Fr,
        value: u64,
    ) -> ([u8; GROTH_PROOF_SIZE], jubjub::ExtendedPoint) {
        let zkproof = [0u8; GROTH_PROOF_SIZE];

        let mut rng = OsRng;

        // We create the randomness of the value commitment
        let rcv = jubjub::Fr::random(&mut rng);

        let cv = ValueCommitment { value, randomness: rcv };
        // Compute value commitment
        let value_commitment: jubjub::ExtendedPoint = cv.commitment().into();
        (zkproof, value_commitment)
    }

    fn binding_sig(
        &self,
        _compact_transaction: &mut Self::SaplingProvingContext,
        _value_balance: Amount,
        _sighash: &[u8; 32],
    ) -> Result<Signature, ()> {
        let fake_bytes = vec![0u8; 64];
        Signature::read(&fake_bytes[..]).map_err(|_e| ())
    }
}
