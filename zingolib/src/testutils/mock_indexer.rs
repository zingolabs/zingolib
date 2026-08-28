//! In-process stateful mock of the `CompactTxStreamer` service, a
//! fabricated chain the real sync pipeline can scan, with no zebrad or
//! zainod involved.
//!
//! [`MockChain`] holds compact blocks (with correct `prev_hash` linkage
//! and cumulative `chain_metadata` tree sizes), full transaction bytes
//! by txid, and per-height serialized commitment-tree states, plus a
//! mempool that [`MockChain::mine_mempool`] turns into the next block.
//! [`MockIndexerService`] serves that state over the wire protocol
//! `GrpcIndexer` speaks (the zaino-proto codegen is wire-identical to
//! the lightwallet-protocol codegen. Zingo-cli's log_file test already
//! proves the pairing). [`MockNet`] launches the server on an ephemeral
//! localhost port and builds `LightClient`s pointed at it.
//!
//! Funding: a "faucet" is a [`SyntheticWalletBuilder`] wallet whose
//! BUILT transactions (via the build-without-transmit seam) are
//! cryptographically real: the recipient can decrypt and later spend
//! their outputs once they are mined into mock blocks, because the mock
//! appends every output to its commitment trees and the wallet's scan
//! of those blocks reconstructs the same tree.
//!
//! Deliberate simplifications, safe for wallet-side tests: the mock
//! validates nothing (no proofs, no signatures, no double-spends), and
//! block hashes are fabricated. Compact transaction indices start at 1
//! so no fabricated transaction is mistaken for a coinbase. The mempool
//! stream hangs until sync's shutdown drops it (pepper-sync's monitor
//! polls its shutdown flag alongside the stream).
//!
//! Available to zingolib's own unit tests and, via the `testutils`
//! feature, to downstream test crates (e.g. libtonode-tests), the
//! rescan-idempotence family's offline seam.

use std::collections::HashMap;
use std::pin::Pin;
use std::sync::Arc;

use tokio::sync::RwLock;
use tokio_stream::Stream;

// The message types come from `lightwallet_protocol`, the same generated
// proto the wallet's client side (pepper-sync via `zingo_netutils`) reads,
// because its `CompactTx` carries the `ironwood_actions` field the pinned
// `zaino-proto` predates. `zaino_proto::tonic` remains solely as the tonic
// re-export (the lock resolves a single tonic, so the types unify).
use zaino_proto::tonic::{self, Request, Response, Status};
use zingo_netutils::lightwallet_protocol::{
    Address, AddressList, Balance, BlockId, BlockRange, ChainMetadata, ChainSpec, CompactBlock,
    CompactOrchardAction, CompactSaplingOutput, CompactSaplingSpend, CompactTx, CompactTxIn,
    CompactTxStreamer, CompactTxStreamerServer, Duration as ProtoDuration, Empty,
    GetAddressUtxosArg, GetAddressUtxosReply, GetAddressUtxosReplyList, GetMempoolTxRequest,
    GetSubtreeRootsArg, LightdInfo, PingResponse, RawTransaction, SendResponse, SubtreeRoot,
    TransparentAddressBlockFilter, TreeState, TxFilter, TxOut as CompactTxOut,
};

use incrementalmerkletree::frontier::CommitmentTree;
use orchard::tree::MerkleHashOrchard;
use zcash_primitives::merkle_tree::{read_commitment_tree, write_commitment_tree};
use zcash_primitives::transaction::Transaction;
use zcash_protocol::consensus::{BlockHeight, BranchId};
use zingo_common_components::protocol::ActivationHeights;

use crate::config::{ChainType, ClientConfig, WalletConfig};
use crate::lightclient::LightClient;
use crate::testutils::default_test_wallet_settings;
use crate::testutils::lightclient::from_inputs;
use crate::testutils::synthetic_wallet::SyntheticWalletBuilder;
use crate::wallet::WalletSettings;

type SaplingTree =
    CommitmentTree<sapling_crypto::Node, { sapling_crypto::NOTE_COMMITMENT_TREE_DEPTH }>;
type OrchardTree = CommitmentTree<MerkleHashOrchard, 32>;

/// The fabricated chain: blocks, transactions, tree states, mempool.
pub struct MockChain {
    chain_type: ChainType,
    /// `blocks[i]` is the block at height `i + 1`.
    blocks: Vec<CompactBlock>,
    /// Full transaction bytes and mined height, by txid (natural order).
    transactions: HashMap<[u8; 32], (u32, Vec<u8>)>,
    /// `tree_states[h]` is the serialized (sapling, orchard, ironwood)
    /// tree state hex as of the END of height `h`. Index 0 is the empty
    /// pre-chain state.
    tree_states: Vec<(String, String, String)>,
    sapling_tree: SaplingTree,
    orchard_tree: OrchardTree,
    /// The Ironwood pool's note commitment tree is orchard-shaped (the
    /// pool shares the Orchard cryptography).
    ironwood_tree: OrchardTree,
    mempool: Vec<Vec<u8>>,
    /// Raw transactions delivered but still in the validator's
    /// download/verification queue: present enough to reject a
    /// resubmission ("already queued for download"), not yet in the
    /// mempool. [`MockChain::promote_download_queue`] is the validator
    /// finishing verification.
    download_queue: Vec<Vec<u8>>,
    /// One-shot fault: the next `send_transaction` takes the bytes
    /// (into the mempool or the download queue per the destination)
    /// but answers with an error. The accepted-but-unanswered
    /// submission of issue #2450, in either of the validator's two
    /// pre-mining phases. Cleared on use.
    pub lose_next_send_response: Option<LostSendDestination>,
    /// The verification delay, made deterministic: how many "already
    /// queued for download" rejections the mock answers before the
    /// download queue promotes to the mempool. Decremented on each
    /// duplicate probe of a queued transaction. At zero the probe is
    /// answered with the mempool-phase rejection instead.
    pub queued_rejections_before_promotion: u8,
    /// One entry per `GetTaddressTxids` request served: the address, the
    /// requested range, and how many transactions were streamed back.
    /// Diagnostic surface for transparent-detection failures.
    taddr_request_log: Vec<String>,
    /// Bumped by [`MockChain::reorg_to`]. Folded into the hashes of
    /// blocks mined afterwards so a re-mined branch is distinguishable
    /// from the branch it replaced. A wallet detects the reorg by hash
    /// mismatch, exactly as against a real chain.
    branch_seed: u32,
}

fn fabricated_block_hash(height: u32) -> Vec<u8> {
    fabricated_branch_hash(height, 0)
}

/// Where a lost-response submission lands, mirroring the validator's
/// two pre-mining phases: zebra queues a submission for download and
/// verification before accepting it into the mempool, and rejects a
/// duplicate differently in each phase.
#[derive(Clone, Copy, Debug)]
pub enum LostSendDestination {
    /// Verification already finished: the duplicate rejection reads
    /// "transaction already exists in mempool".
    Mempool,
    /// Verification still pending: the duplicate rejection reads
    /// "transaction dropped because it is already queued for download".
    DownloadQueue,
}

/// Height- and branch-seeded fabricated hash. Seed 0 reproduces the
/// pre-reorg hashes byte for byte, so chains that never reorg are
/// unchanged by the branching machinery.
fn fabricated_branch_hash(height: u32, branch_seed: u32) -> Vec<u8> {
    let mut hash = vec![0x5a; 32];
    hash[..4].copy_from_slice(&height.to_le_bytes());
    if branch_seed != 0 {
        hash[4..8].copy_from_slice(&branch_seed.to_le_bytes());
    }
    hash
}

fn tree_state_hex(
    sapling_tree: &SaplingTree,
    orchard_tree: &OrchardTree,
    ironwood_tree: &OrchardTree,
) -> (String, String, String) {
    let mut sapling_bytes = vec![];
    write_commitment_tree(sapling_tree, &mut sapling_bytes)
        .expect("in-memory serialization is infallible");
    let mut orchard_bytes = vec![];
    write_commitment_tree(orchard_tree, &mut orchard_bytes)
        .expect("in-memory serialization is infallible");
    let mut ironwood_bytes = vec![];
    write_commitment_tree(ironwood_tree, &mut ironwood_bytes)
        .expect("in-memory serialization is infallible");
    (
        hex::encode(sapling_bytes),
        hex::encode(orchard_bytes),
        hex::encode(ironwood_bytes),
    )
}

impl Default for MockChain {
    fn default() -> Self {
        Self::new()
    }
}

impl MockChain {
    /// An empty regtest chain at height 0 with the default (everything
    /// at height 1) activation schedule, matching
    /// [`SyntheticWalletBuilder`]'s default and [`MockNet::client`]'s
    /// config.
    pub fn new() -> Self {
        let sapling_tree = SaplingTree::empty();
        let orchard_tree = OrchardTree::empty();
        let ironwood_tree = OrchardTree::empty();
        let genesis_state = tree_state_hex(&sapling_tree, &orchard_tree, &ironwood_tree);
        Self {
            chain_type: ChainType::Regtest(ActivationHeights::default()),
            blocks: Vec::new(),
            transactions: HashMap::new(),
            tree_states: vec![genesis_state],
            sapling_tree,
            orchard_tree,
            ironwood_tree,
            mempool: Vec::new(),
            download_queue: Vec::new(),
            lose_next_send_response: None,
            queued_rejections_before_promotion: 0,
            taddr_request_log: Vec::new(),
            branch_seed: 0,
        }
    }

    /// The `GetTaddressTxids` requests served so far, for diagnosing
    /// transparent-detection failures in tests.
    pub fn taddr_request_log(&self) -> &[String] {
        &self.taddr_request_log
    }

    /// The height of the chain tip (0 on an empty chain).
    pub fn tip(&self) -> u32 {
        self.blocks.len() as u32
    }

    /// Accepts raw transaction bytes into the mempool, returning the
    /// txid in the display encoding `send_transaction` answers with.
    pub fn submit_transaction(&mut self, bytes: Vec<u8>) -> String {
        let transaction = self.parse_transaction(&bytes, self.tip() + 1);
        let txid = transaction.txid().to_string();
        self.mempool.push(bytes);
        txid
    }

    /// The validator finishing verification: queued transactions enter
    /// the mempool.
    pub fn promote_download_queue(&mut self) {
        let verified = std::mem::take(&mut self.download_queue);
        self.mempool.extend(verified);
    }

    /// Mines every mempool transaction into the next block.
    pub fn mine_mempool(&mut self) {
        let pending = std::mem::take(&mut self.mempool);
        self.mine_block(pending);
    }

    /// Mines the given raw transactions (plus nothing else) into the
    /// next block.
    pub fn mine_block(&mut self, raw_transactions: Vec<Vec<u8>>) {
        let height = self.tip() + 1;
        let mut vtx = Vec::new();
        for (i, bytes) in raw_transactions.into_iter().enumerate() {
            let transaction = self.parse_transaction(&bytes, height);
            for output in transaction
                .sapling_bundle()
                .iter()
                .flat_map(|bundle| bundle.shielded_outputs())
            {
                self.sapling_tree
                    .append(sapling_crypto::Node::from_cmu(output.cmu()))
                    .expect("the fabricated chain stays far below tree capacity");
            }
            for action in transaction
                .orchard_bundle()
                .iter()
                .flat_map(|bundle| bundle.actions())
            {
                self.orchard_tree
                    .append(MerkleHashOrchard::from_cmx(action.cmx()))
                    .expect("the fabricated chain stays far below tree capacity");
            }
            for action in transaction
                .ironwood_bundle()
                .iter()
                .flat_map(|bundle| bundle.actions())
            {
                self.ironwood_tree
                    .append(MerkleHashOrchard::from_cmx(action.cmx()))
                    .expect("the fabricated chain stays far below tree capacity");
            }
            // Indices start at 1: light clients treat index 0 as the
            // coinbase, and no fabricated transaction is one.
            vtx.push(compact_transaction(i as u64 + 1, &transaction));
            self.transactions
                .insert(*transaction.txid().as_ref(), (height, bytes));
        }
        // Chain from the stored predecessor hash rather than recomputing
        // it: after a reorg the predecessor may belong to a different
        // branch seed than this block.
        let prev_hash = self
            .blocks
            .last()
            .map(|block| block.hash.clone())
            .unwrap_or_else(|| vec![0u8; 32]);
        self.blocks.push(CompactBlock {
            height: u64::from(height),
            hash: fabricated_branch_hash(height, self.branch_seed),
            prev_hash,
            time: 1_700_000_000 + height,
            header: vec![],
            vtx,
            chain_metadata: Some(ChainMetadata {
                sapling_commitment_tree_size: self.sapling_tree.size() as u32,
                orchard_commitment_tree_size: self.orchard_tree.size() as u32,
                ironwood_commitment_tree_size: self.ironwood_tree.size() as u32,
            }),
        });
        self.tree_states.push(tree_state_hex(
            &self.sapling_tree,
            &self.orchard_tree,
            &self.ironwood_tree,
        ));
    }

    /// Mines `count` empty blocks, advancing the tip without new outputs.
    pub fn mine_empty_blocks(&mut self, count: u32) {
        for _ in 0..count {
            self.mine_block(vec![]);
        }
    }

    /// Rewinds the chain to `height`, discarding every block above it,
    /// the reorg primitive. Blocks mined afterwards carry a new branch
    /// seed in their hashes, so a syncing wallet sees a hash mismatch
    /// above `height` and truncates, exactly as against a real reorg.
    /// Returns the raw bytes of the discarded transactions in mined
    /// order. The caller models miner behavior by resubmitting them to
    /// the mempool, re-mining them at new heights, or dropping them to
    /// let the wallet expire them.
    pub fn reorg_to(&mut self, height: u32) -> Vec<Vec<u8>> {
        assert!(
            height <= self.tip(),
            "reorg_to({height}) above the tip {}",
            self.tip()
        );
        self.blocks.truncate(height as usize);
        self.tree_states.truncate(height as usize + 1);
        let (sapling_hex, orchard_hex, ironwood_hex) = self.tree_states[height as usize].clone();
        self.sapling_tree = read_commitment_tree(
            hex::decode(sapling_hex)
                .expect("stored tree state is valid hex")
                .as_slice(),
        )
        .expect("stored sapling tree state deserializes");
        self.orchard_tree = read_commitment_tree(
            hex::decode(orchard_hex)
                .expect("stored tree state is valid hex")
                .as_slice(),
        )
        .expect("stored orchard tree state deserializes");
        self.ironwood_tree = read_commitment_tree(
            hex::decode(ironwood_hex)
                .expect("stored tree state is valid hex")
                .as_slice(),
        )
        .expect("stored ironwood tree state deserializes");
        let mut evicted: Vec<(u32, Vec<u8>)> = Vec::new();
        self.transactions.retain(|_, (mined_height, bytes)| {
            if *mined_height > height {
                evicted.push((*mined_height, bytes.clone()));
                false
            } else {
                true
            }
        });
        evicted.sort_by_key(|(mined_height, _)| *mined_height);
        self.branch_seed += 1;
        evicted.into_iter().map(|(_, bytes)| bytes).collect()
    }

    fn parse_transaction(&self, bytes: &[u8], height: u32) -> Transaction {
        Transaction::read(
            bytes,
            BranchId::for_height(&self.chain_type, BlockHeight::from_u32(height)),
        )
        .expect("fabricated and wallet-built transactions parse")
    }

    fn tree_state_at(&self, height: u32) -> Option<TreeState> {
        let (sapling_tree, orchard_tree, ironwood_tree) =
            self.tree_states.get(height as usize)?.clone();
        // The stored block's own hash, so tree states follow the
        // current branch after a reorg. Height 0 predates the chain.
        let hash = if height == 0 {
            fabricated_block_hash(0)
        } else {
            self.blocks[height as usize - 1].hash.clone()
        };
        Some(TreeState {
            network: "regtest".to_string(),
            height: u64::from(height),
            hash: hex::encode(hash),
            time: 1_700_000_000 + height,
            sapling_tree,
            orchard_tree,
            ironwood_tree,
        })
    }
}

fn compact_transaction(index: u64, transaction: &Transaction) -> CompactTx {
    let spends = transaction
        .sapling_bundle()
        .iter()
        .flat_map(|bundle| bundle.shielded_spends())
        .map(|spend| CompactSaplingSpend {
            nf: spend.nullifier().as_ref().to_vec(),
        })
        .collect();
    let outputs = transaction
        .sapling_bundle()
        .iter()
        .flat_map(|bundle| bundle.shielded_outputs())
        .map(|output| CompactSaplingOutput {
            cmu: output.cmu().to_bytes().to_vec(),
            ephemeral_key: output.ephemeral_key().0.to_vec(),
            ciphertext: output.enc_ciphertext()[..52].to_vec(),
        })
        .collect();
    let actions = transaction
        .orchard_bundle()
        .iter()
        .flat_map(|bundle| bundle.actions())
        .map(|action| CompactOrchardAction {
            nullifier: action.nullifier().to_bytes().to_vec(),
            cmx: action.cmx().to_bytes().to_vec(),
            ephemeral_key: action.encrypted_note().epk_bytes.to_vec(),
            ciphertext: action.encrypted_note().enc_ciphertext[..52].to_vec(),
        })
        .collect();
    let ironwood_actions = transaction
        .ironwood_bundle()
        .iter()
        .flat_map(|bundle| bundle.actions())
        .map(|action| CompactOrchardAction {
            nullifier: action.nullifier().to_bytes().to_vec(),
            cmx: action.cmx().to_bytes().to_vec(),
            ephemeral_key: action.encrypted_note().epk_bytes.to_vec(),
            ciphertext: action.encrypted_note().enc_ciphertext[..52].to_vec(),
        })
        .collect();
    let vin = transaction
        .transparent_bundle()
        .iter()
        .flat_map(|bundle| bundle.vin.iter())
        .map(|txin| CompactTxIn {
            prevout_txid: txin.prevout().txid().as_ref().to_vec(),
            prevout_index: txin.prevout().n(),
        })
        .collect();
    let vout = transaction
        .transparent_bundle()
        .iter()
        .flat_map(|bundle| bundle.vout.iter())
        .map(|txout| CompactTxOut {
            value: u64::from(txout.value()),
            script_pub_key: txout.script_pubkey().0.0.clone(),
        })
        .collect();
    CompactTx {
        index,
        txid: transaction.txid().as_ref().to_vec(),
        fee: 0,
        spends,
        outputs,
        actions,
        ironwood_actions,
        vin,
        vout,
    }
}

type ResponseStream<T> = Pin<Box<dyn Stream<Item = Result<T, Status>> + Send>>;

/// Serves a [`MockChain`] over the `CompactTxStreamer` protocol.
pub struct MockIndexerService {
    chain: Arc<RwLock<MockChain>>,
}

#[tonic::async_trait]
impl CompactTxStreamer for MockIndexerService {
    async fn get_latest_block(
        &self,
        _request: Request<ChainSpec>,
    ) -> Result<Response<BlockId>, Status> {
        let chain = self.chain.read().await;
        Ok(Response::new(BlockId {
            height: u64::from(chain.tip()),
            hash: fabricated_block_hash(chain.tip()),
        }))
    }

    async fn get_block(&self, request: Request<BlockId>) -> Result<Response<CompactBlock>, Status> {
        let height = request.into_inner().height;
        let chain = self.chain.read().await;
        chain
            .blocks
            .get((height as usize).wrapping_sub(1))
            .cloned()
            .map(Response::new)
            .ok_or_else(|| Status::not_found(format!("no block at height {height}")))
    }

    async fn get_block_nullifiers(
        &self,
        request: Request<BlockId>,
    ) -> Result<Response<CompactBlock>, Status> {
        self.get_block(request).await
    }

    type GetBlockRangeStream = ResponseStream<CompactBlock>;
    async fn get_block_range(
        &self,
        request: Request<BlockRange>,
    ) -> Result<Response<Self::GetBlockRangeStream>, Status> {
        let range = request.into_inner();
        let start = range.start.map_or(0, |id| id.height) as usize;
        let end = range.end.map_or(0, |id| id.height) as usize;
        if start == 0 || end < start {
            return Err(Status::invalid_argument(
                "the mock serves ascending ranges starting at height 1",
            ));
        }
        let chain = self.chain.read().await;
        if end > chain.blocks.len() {
            return Err(Status::not_found(format!("no block at height {end}")));
        }
        let blocks: Vec<_> = chain.blocks[start - 1..end].to_vec();
        Ok(Response::new(Box::pin(tokio_stream::iter(
            blocks.into_iter().map(Ok),
        ))))
    }

    type GetBlockRangeNullifiersStream = ResponseStream<CompactBlock>;
    async fn get_block_range_nullifiers(
        &self,
        request: Request<BlockRange>,
    ) -> Result<Response<Self::GetBlockRangeNullifiersStream>, Status> {
        self.get_block_range(request).await
    }

    async fn get_tree_state(
        &self,
        request: Request<BlockId>,
    ) -> Result<Response<TreeState>, Status> {
        let height = request.into_inner().height as u32;
        let chain = self.chain.read().await;
        chain
            .tree_state_at(height)
            .map(Response::new)
            .ok_or_else(|| Status::not_found(format!("no tree state at height {height}")))
    }

    async fn get_latest_tree_state(
        &self,
        _request: Request<Empty>,
    ) -> Result<Response<TreeState>, Status> {
        let chain = self.chain.read().await;
        let tip = chain.tip();
        chain
            .tree_state_at(tip)
            .map(Response::new)
            .ok_or_else(|| Status::internal("tip tree state always exists"))
    }

    type GetSubtreeRootsStream = ResponseStream<SubtreeRoot>;
    async fn get_subtree_roots(
        &self,
        _request: Request<GetSubtreeRootsArg>,
    ) -> Result<Response<Self::GetSubtreeRootsStream>, Status> {
        // A short regtest chain has no completed shards; an empty stream
        // is exactly what zainod serves in the live suites.
        Ok(Response::new(Box::pin(tokio_stream::iter(
            std::iter::empty(),
        ))))
    }

    async fn get_transaction(
        &self,
        request: Request<TxFilter>,
    ) -> Result<Response<RawTransaction>, Status> {
        let txid: [u8; 32] = request
            .into_inner()
            .hash
            .try_into()
            .map_err(|_| Status::invalid_argument("txid must be 32 bytes"))?;
        let chain = self.chain.read().await;
        chain
            .transactions
            .get(&txid)
            .map(|(height, bytes)| {
                Response::new(RawTransaction {
                    data: bytes.clone(),
                    height: u64::from(*height),
                })
            })
            .ok_or_else(|| Status::not_found("transaction not mined on the mock chain"))
    }

    async fn send_transaction(
        &self,
        request: Request<RawTransaction>,
    ) -> Result<Response<SendResponse>, Status> {
        let mut chain = self.chain.write().await;
        let bytes = request.into_inner().data;
        // The validator rejects resubmitted bytes rather than
        // re-accepting them, with a phase-specific message; reproduce
        // both rejections verbatim as zainod 0.6.0-rc.1 surfaces them
        // (zingolabs/zaino#1392), so client handling is exercised
        // against the messages it really receives.
        if chain.mempool.contains(&bytes) {
            return Err(Status::internal(
                "unhandled rpc-specific zaino_fetch::jsonrpsee::response::SendTransactionError \
                 error: RPC Error (code: -1): transaction already exists in mempool",
            ));
        }
        if chain.download_queue.contains(&bytes) {
            if chain.queued_rejections_before_promotion == 0 {
                // Verification finished between probes: the queue
                // promotes and this probe already sees the
                // mempool-phase rejection.
                chain.promote_download_queue();
                return Err(Status::internal(
                    "unhandled rpc-specific zaino_fetch::jsonrpsee::response::SendTransactionError \
                     error: RPC Error (code: -1): transaction already exists in mempool",
                ));
            }
            chain.queued_rejections_before_promotion -= 1;
            return Err(Status::internal(
                "unhandled rpc-specific zaino_fetch::jsonrpsee::response::SendTransactionError \
                 error: RPC Error (code: -1): transaction dropped because it is already queued \
                 for download",
            ));
        }
        // The fault of issue #2450: delivery happened, the caller
        // never learns.
        match chain.lose_next_send_response.take() {
            Some(LostSendDestination::Mempool) => {
                chain.submit_transaction(bytes);
                Err(Status::deadline_exceeded(
                    "mock fault: response lost after mempool acceptance",
                ))
            }
            Some(LostSendDestination::DownloadQueue) => {
                chain.download_queue.push(bytes);
                Err(Status::deadline_exceeded(
                    "mock fault: response lost while queued for download",
                ))
            }
            None => {
                let txid = chain.submit_transaction(bytes);
                Ok(Response::new(SendResponse {
                    error_code: 0,
                    error_message: txid,
                }))
            }
        }
    }

    type GetTaddressTxidsStream = ResponseStream<RawTransaction>;
    async fn get_taddress_txids(
        &self,
        request: Request<TransparentAddressBlockFilter>,
    ) -> Result<Response<Self::GetTaddressTxidsStream>, Status> {
        let filter = request.into_inner();
        let range = filter
            .range
            .ok_or_else(|| Status::invalid_argument("missing block range"))?;
        let start = range.start.map_or(1, |id| id.height) as u32;
        let end = range.end.map_or(u64::MAX, |id| id.height) as u32;
        let chain = self.chain.read().await;
        let target_script = taddr_script_bytes(&filter.address, &chain.chain_type)
            .map_err(Status::invalid_argument)?;
        // A transaction involves the address if an output pays its
        // script, or an input spends an outpoint that paid its script.
        let mut involved: Vec<(u32, usize, Vec<u8>)> = Vec::new();
        for (height, bytes) in chain.transactions.values() {
            if *height < start || *height > end {
                continue;
            }
            let transaction = chain.parse_transaction(bytes, *height);
            let Some(bundle) = transaction.transparent_bundle() else {
                continue;
            };
            let pays = bundle
                .vout
                .iter()
                .any(|txout| txout.script_pubkey().0.0 == target_script);
            let spends = bundle.vin.iter().any(|txin| {
                chain
                    .transactions
                    .get(txin.prevout().txid().as_ref())
                    .and_then(|(prev_height, prev_bytes)| {
                        let prev = chain.parse_transaction(prev_bytes, *prev_height);
                        prev.transparent_bundle().and_then(|prev_bundle| {
                            prev_bundle
                                .vout
                                .get(txin.prevout().n() as usize)
                                .map(|prev_out| prev_out.script_pubkey().0.0 == target_script)
                        })
                    })
                    .unwrap_or(false)
            });
            if pays || spends {
                let block = &chain.blocks[*height as usize - 1];
                let index = block
                    .vtx
                    .iter()
                    .position(|compact| compact.txid == transaction.txid().as_ref())
                    .expect("mined transactions appear in their block");
                involved.push((*height, index, bytes.clone()));
            }
        }
        involved.sort_by_key(|(height, index, _)| (*height, *index));
        let raw: Vec<_> = involved
            .into_iter()
            .map(|(height, _, data)| {
                Ok(RawTransaction {
                    data,
                    height: u64::from(height),
                })
            })
            .collect();
        let served = raw.len();
        drop(chain);
        self.chain.write().await.taddr_request_log.push(format!(
            "address={} range=[{start},{end}] served={served}",
            filter.address
        ));
        Ok(Response::new(Box::pin(tokio_stream::iter(raw))))
    }

    type GetTaddressTransactionsStream = ResponseStream<RawTransaction>;
    async fn get_taddress_transactions(
        &self,
        request: Request<TransparentAddressBlockFilter>,
    ) -> Result<Response<Self::GetTaddressTransactionsStream>, Status> {
        self.get_taddress_txids(request).await
    }

    type GetMempoolTxStream = ResponseStream<CompactTx>;
    async fn get_mempool_tx(
        &self,
        _request: Request<GetMempoolTxRequest>,
    ) -> Result<Response<Self::GetMempoolTxStream>, Status> {
        let chain = self.chain.read().await;
        let height = chain.tip() + 1;
        let compact: Vec<_> = chain
            .mempool
            .iter()
            .enumerate()
            .map(|(i, bytes)| {
                Ok(compact_transaction(
                    i as u64 + 1,
                    &chain.parse_transaction(bytes, height),
                ))
            })
            .collect();
        Ok(Response::new(Box::pin(tokio_stream::iter(compact))))
    }

    type GetMempoolStreamStream = ResponseStream<RawTransaction>;
    async fn get_mempool_stream(
        &self,
        _request: Request<Empty>,
    ) -> Result<Response<Self::GetMempoolStreamStream>, Status> {
        // Hangs until the client drops it: pepper-sync's monitor polls
        // its shutdown flag alongside the stream, so a pending-forever
        // stream is dropped cleanly at session end.
        Ok(Response::new(Box::pin(tokio_stream::pending())))
    }

    async fn get_lightd_info(
        &self,
        _request: Request<Empty>,
    ) -> Result<Response<LightdInfo>, Status> {
        let chain = self.chain.read().await;
        let tip = chain.tip();
        let branch_id: u32 =
            BranchId::for_height(&chain.chain_type, BlockHeight::from_u32(tip.max(1))).into();
        Ok(Response::new(LightdInfo {
            version: "mock-indexer".to_string(),
            vendor: "zingolib test".to_string(),
            taddr_support: true,
            chain_name: "regtest".to_string(),
            sapling_activation_height: 1,
            consensus_branch_id: format!("{branch_id:08x}"),
            block_height: u64::from(tip),
            estimated_height: u64::from(tip),
            ..Default::default()
        }))
    }

    async fn get_taddress_balance(
        &self,
        _request: Request<AddressList>,
    ) -> Result<Response<Balance>, Status> {
        Err(Status::unimplemented("get_taddress_balance"))
    }

    async fn get_taddress_balance_stream(
        &self,
        _request: Request<tonic::Streaming<Address>>,
    ) -> Result<Response<Balance>, Status> {
        Err(Status::unimplemented("get_taddress_balance_stream"))
    }

    async fn get_address_utxos(
        &self,
        _request: Request<GetAddressUtxosArg>,
    ) -> Result<Response<GetAddressUtxosReplyList>, Status> {
        Err(Status::unimplemented("get_address_utxos"))
    }

    type GetAddressUtxosStreamStream = ResponseStream<GetAddressUtxosReply>;
    async fn get_address_utxos_stream(
        &self,
        _request: Request<GetAddressUtxosArg>,
    ) -> Result<Response<Self::GetAddressUtxosStreamStream>, Status> {
        Err(Status::unimplemented("get_address_utxos_stream"))
    }

    async fn ping(
        &self,
        _request: Request<ProtoDuration>,
    ) -> Result<Response<PingResponse>, Status> {
        Err(Status::unimplemented("ping"))
    }
}

/// Decodes a transparent address string to its script bytes.
fn taddr_script_bytes(address: &str, chain_type: &ChainType) -> Result<Vec<u8>, String> {
    use pepper_sync::keys::decode_address;
    use zcash_client_backend::address::Address as DecodedAddress;

    match decode_address(chain_type, address) {
        Ok(DecodedAddress::Transparent(taddr)) => {
            let script: zcash_transparent::address::Script = taddr.script().into();
            Ok(script.0.0)
        }
        Ok(_) => Err(format!("{address} is not a transparent address")),
        Err(e) => Err(format!("undecodable address {address}: {e:?}")),
    }
}

/// A launched mock network: the chain handle, the serving task, and a
/// factory for `LightClient`s dialed at it.
pub struct MockNet {
    /// The fabricated chain the server reads and the test mutates.
    pub chain: Arc<RwLock<MockChain>>,
    indexer_uri: http::Uri,
    _wallet_dirs: Vec<tempfile::TempDir>,
    _server: tokio::task::JoinHandle<()>,
}

impl MockNet {
    /// Launches the mock server on an ephemeral localhost port with an
    /// empty chain.
    pub async fn launch() -> Self {
        let chain = Arc::new(RwLock::new(MockChain::new()));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("an ephemeral localhost port binds");
        let port = listener
            .local_addr()
            .expect("bound socket has an address")
            .port();
        let service = MockIndexerService {
            chain: chain.clone(),
        };
        let server = tokio::spawn(async move {
            tonic::transport::Server::builder()
                .add_service(CompactTxStreamerServer::new(service))
                .serve_with_incoming(tokio_stream::wrappers::TcpListenerStream::new(listener))
                .await
                .ok();
        });
        let indexer_uri: http::Uri = format!("http://127.0.0.1:{port}")
            .parse()
            .expect("a localhost uri parses");
        Self {
            chain,
            indexer_uri,
            _wallet_dirs: Vec::new(),
            _server: server,
        }
    }

    /// Builds a `LightClient` for `mnemonic` (birthday 1) dialed at the
    /// mock, with its wallet directory in a tempdir this net keeps
    /// alive.
    pub async fn client(
        &mut self,
        mnemonic: &str,
        wallet_settings_opt: Option<WalletSettings>,
    ) -> LightClient {
        let wallet_dir = tempfile::tempdir().expect("a tempdir is creatable");
        let config = ClientConfig::builder()
            .set_chain_type(ChainType::Regtest(ActivationHeights::default()))
            .set_indexer_uri(self.indexer_uri.clone())
            .set_wallet_dir(wallet_dir.path().to_path_buf())
            .set_wallet_config(WalletConfig::MnemonicPhrase {
                mnemonic_phrase: mnemonic.to_string(),
                no_of_accounts: 1.try_into().expect("hard-coded non-zero"),
                birthday: 1,
                wallet_settings: wallet_settings_opt.unwrap_or_else(default_test_wallet_settings),
            })
            .build()
            .unwrap();
        self._wallet_dirs.push(wallet_dir);
        let mut lightclient = LightClient::new(config, true)
            .await
            .expect("mock-net client construction succeeds");
        // Mirror the live scenarios' build_client: a sapling-only UA at
        // index 1, which get_base_address's sapling arm pins.
        lightclient
            .generate_unified_address(
                crate::wallet::keys::unified::ReceiverSelection::sapling_only(),
                zip32::AccountId::ZERO,
            )
            .await
            .expect("sapling-only address generation succeeds");
        // Mock-net clients run with Mixnet Mode switched on, so every
        // chain-mock send walks the fail-closed route resolver and the
        // escalation orchestration instead of quietly consenting to clearnet.
        // The address is never dialed: the transmit path pairs this slot
        // state with arms that submit over the mock indexer's channel.
        // Without the nym feature there is no mixnet and sends stay
        // clearnet, so the same tests cover both routes across the
        // feature matrix.
        #[cfg(feature = "nym")]
        lightclient
            .switch_on_mixnet_for_tests(crate::mocks::transmission::MOCK_SOCKS5_ADDR)
            .await;
        lightclient
    }
}

/// Builds (without transmitting) one real transaction from a synthetic
/// faucet to the given receivers, returning its raw bytes, the mock
/// chain's funding primitive. Each call uses a fresh faucet whose
/// fabricated backing note never exists on the mock chain. Nothing
/// validates that, and the recipient-facing outputs are real.
pub async fn faucet_funding_transaction(receivers: Vec<(&str, u64, Option<&str>)>) -> Vec<u8> {
    let total: u64 = receivers.iter().map(|(_, value, _)| value).sum();
    let wallet = SyntheticWalletBuilder::new(zingo_test_vectors::seeds::ABANDON_ART_SEED)
        // Generous headroom over payments + any fee.
        .ironwood_note(total + 1_000_000)
        .build();
    let mut faucet = LightClient::new_for_test(wallet).await;
    let proposal = from_inputs::propose(&mut faucet, receivers)
        .await
        .expect("the faucet's funding proposal succeeds");
    let txids = faucet
        .wallet()
        .write()
        .await
        .calculate_transactions(proposal, zip32::AccountId::ZERO)
        .await
        .expect("the faucet's funding transaction builds");
    assert_eq!(txids.len(), 1, "funding sends are single-step");
    let wallet = faucet.wallet();
    let wallet = wallet.read().await;
    let mut bytes = vec![];
    wallet
        .wallet_transactions
        .get(&txids[0])
        .expect("the built transaction is stored")
        .transaction()
        .write(&mut bytes)
        .expect("in-memory serialization is infallible");
    bytes
}
