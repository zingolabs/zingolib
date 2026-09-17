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
//! Available to zingolib's own unit tests and, via the `testutils`
//! feature, to downstream test crates (e.g. libtonode-tests), the
//! rescan-idempotence family's offline seam.

use std::collections::{BTreeSet, HashMap, HashSet, VecDeque};
use std::fmt;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{RwLock, mpsc};
use tokio_stream::Stream;

use zaino_proto::tonic::{self, Code, Request, Response, Status};
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
use zcash_primitives::transaction::fees::zip317::{GRACE_ACTIONS, MARGINAL_FEE};
use zcash_primitives::transaction::{Authorized, Transaction, TransactionData, TxId, TxVersion};
use zcash_protocol::consensus::{
    BlockHeight, BranchId, COINBASE_MATURITY_BLOCKS, NetworkUpgrade, Parameters,
};
use zcash_protocol::value::{ZatBalance, Zatoshis};
use zcash_protocol::{PoolType, ShieldedPool};
use zcash_transparent::address::Script;
use zcash_transparent::bundle::{
    Authorized as TransparentAuthorized, Bundle as TransparentBundle, OutPoint, TxIn, TxOut,
};
use zingo_common_components::protocol::ActivationHeights;

use crate::config::{ChainType, ClientConfig, ClientConfigBuilder, WalletConfig};
use crate::lightclient::LightClient;
use crate::testutils::chain_generics::conduct_chain::ConductChain;
use crate::testutils::default_test_wallet_settings;
use crate::testutils::lightclient::{from_inputs, get_base_address};
use crate::testutils::synthetic_wallet::SyntheticWalletBuilder;
use crate::wallet::WalletSettings;

type SaplingTree =
    CommitmentTree<sapling_crypto::Node, { sapling_crypto::NOTE_COMMITMENT_TREE_DEPTH }>;
type OrchardTree = CommitmentTree<MerkleHashOrchard, 32>;

const MIN_MEMPOOL_FEE_RATE_ZAT_PER_KB: u64 = 100;
const BYTES_PER_KB: u64 = 1000;
const BLOCK_UNPAID_ACTION_LIMIT: usize = 50;
const ZAINO_SEND_ERROR_PREFIX: &str = "unhandled rpc-specific zaino_fetch::jsonrpsee::response::SendTransactionError error: RPC Error";
const ZAINO_DUPLICATE_CODE: i32 = -1;
const ZAINO_REJECTION_CODE: i32 = -26;
const STREAM_EOF_MESSAGE: &str = "Unexpected EOF decoding stream.";
const MEMPOOL_RAW_TRANSACTION_HEIGHT: u64 = 0;
const BLOCK_TIME_BASE: u32 = 1_700_000_000;
const OP_SMALL_INTEGER_BASE: u8 = 0x50;
const SMALL_INTEGER_MAX: u32 = 16;
const SIGN_BIT: u8 = 0x80;
const GENESIS_HEIGHT: BlockHeight = BlockHeight::from_u32(0);
const NO_EXPIRY: BlockHeight = BlockHeight::from_u32(0);
const FAUCET_FUNDING: u64 = 1_000_000_000;
const FAUCET_HEADROOM: u64 = 1_000_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
/// The set of consensus checks applied to a submitted transaction.
pub struct Rules {
    /// Whether the consensus branch id must match the next block's.
    pub branch_id: bool,
    /// Whether the expiry height must be at or above the next block.
    pub expiry: bool,
    /// Whether every nullifier must be unrevealed on chain and in the mempool.
    pub nullifiers: bool,
    /// Whether every anchor must be a past tree root.
    pub anchors: bool,
    /// Whether every transparent input must exist and be unspent.
    pub transparent_inputs: bool,
    /// Whether a spent coinbase output must have reached maturity.
    pub coinbase_maturity: bool,
    /// Whether the fee must satisfy the ZIP 317 mempool policy.
    pub fees: bool,
}

impl Rules {
    /// The rule set with every check enabled.
    pub const STRICT: Self = Self {
        branch_id: true,
        expiry: true,
        nullifiers: true,
        anchors: true,
        transparent_inputs: true,
        coinbase_maturity: true,
        fees: true,
    };

    /// The rule set with every check disabled.
    pub const LAX: Self = Self {
        branch_id: false,
        expiry: false,
        nullifiers: false,
        anchors: false,
        transparent_inputs: false,
        coinbase_maturity: false,
        fees: false,
    };
}

impl Default for Rules {
    fn default() -> Self {
        Self::STRICT
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
/// A nullifier tagged with its shielded pool.
pub enum PoolNullifier {
    /// A Sapling nullifier.
    Sapling(sapling_crypto::Nullifier),
    /// An Orchard nullifier.
    Orchard(orchard::note::Nullifier),
    /// An Ironwood nullifier.
    Ironwood(orchard::note::Nullifier),
}

impl PoolNullifier {
    /// Returns the pool this value belongs to.
    pub fn pool(&self) -> ShieldedPool {
        match self {
            Self::Sapling(_) => ShieldedPool::Sapling,
            Self::Orchard(_) => ShieldedPool::Orchard,
            Self::Ironwood(_) => ShieldedPool::Ironwood,
        }
    }

    /// Returns the 32-byte encoding.
    pub fn to_bytes(self) -> [u8; 32] {
        match self {
            Self::Sapling(nullifier) => nullifier.0,
            Self::Orchard(nullifier) | Self::Ironwood(nullifier) => nullifier.to_bytes(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
/// A tree root tagged with its shielded pool.
pub enum PoolAnchor {
    /// A Sapling anchor.
    Sapling(sapling_crypto::Anchor),
    /// An Orchard anchor.
    Orchard(orchard::Anchor),
    /// An Ironwood anchor.
    Ironwood(orchard::Anchor),
}

impl PoolAnchor {
    /// Returns the pool this value belongs to.
    pub fn pool(&self) -> ShieldedPool {
        match self {
            Self::Sapling(_) => ShieldedPool::Sapling,
            Self::Orchard(_) => ShieldedPool::Orchard,
            Self::Ironwood(_) => ShieldedPool::Ironwood,
        }
    }

    /// Returns the 32-byte encoding.
    pub fn to_bytes(self) -> [u8; 32] {
        match self {
            Self::Sapling(anchor) => anchor.to_bytes(),
            Self::Orchard(anchor) | Self::Ironwood(anchor) => anchor.to_bytes(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
/// The reason the mock refused a submitted transaction.
pub enum Rejection {
    /// The bytes did not parse as a transaction.
    Unparseable(String),
    /// The consensus branch id is not the next block's.
    BranchId {
        /// The branch id at the next block height.
        expected: BranchId,
        /// The branch id in the transaction.
        found: BranchId,
    },
    /// The expiry height is below the next block.
    Expired {
        /// The transaction's expiry height.
        expiry: BlockHeight,
        /// The height of the next block.
        next_height: BlockHeight,
    },
    /// The nullifier was already revealed.
    DuplicateNullifier(PoolNullifier),
    /// The anchor is not a past tree root.
    UnknownAnchor(PoolAnchor),
    /// The transparent input does not exist.
    UnknownInput(OutPoint),
    /// The transparent input was already spent.
    SpentInput(OutPoint),
    /// A coinbase output was spent before maturity.
    ImmatureCoinbase {
        /// The height of the coinbase block.
        mined_at: BlockHeight,
        /// The height of the next block.
        next_height: BlockHeight,
    },
    /// Outputs exceed inputs by this amount.
    NegativeFee(Zatoshis),
    /// The fee is below the minimum rate for the transaction size.
    FeeBelowMinimumRate {
        /// The fee paid.
        fee: Zatoshis,
        /// The minimum fee for the size.
        required: Zatoshis,
    },
    /// More unpaid logical actions than the mempool admits.
    UnpaidActions {
        /// Unpaid logical actions.
        unpaid: usize,
        /// The block limit on unpaid actions.
        limit: usize,
    },
}

impl fmt::Display for Rejection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unparseable(error) => write!(f, "transaction failed to deserialize: {error}"),
            Self::BranchId { expected, found } => write!(
                f,
                "transaction consensus branch id {:08x} does not match the branch id {:08x} at \
                 the next block height",
                u32::from(*found),
                u32::from(*expected)
            ),
            Self::Expired {
                expiry,
                next_height,
            } => write!(
                f,
                "transaction has expired: expiry height {expiry} is below the next block \
                 height {next_height}"
            ),
            Self::DuplicateNullifier(nullifier) => write!(
                f,
                "{:?} nullifier {} was already revealed",
                nullifier.pool(),
                hex::encode(nullifier.to_bytes())
            ),
            Self::UnknownAnchor(anchor) => write!(
                f,
                "{:?} anchor {} is not the note commitment tree root of any block",
                anchor.pool(),
                hex::encode(anchor.to_bytes())
            ),
            Self::UnknownInput(outpoint) => write!(
                f,
                "transparent input {}:{} does not exist",
                outpoint.txid(),
                outpoint.n()
            ),
            Self::SpentInput(outpoint) => write!(
                f,
                "transparent input {}:{} was already spent",
                outpoint.txid(),
                outpoint.n()
            ),
            Self::ImmatureCoinbase {
                mined_at,
                next_height,
            } => write!(
                f,
                "coinbase output mined at height {mined_at} is immature at height {next_height}"
            ),
            Self::NegativeFee(deficit) => write!(
                f,
                "transaction outputs exceed inputs by {} zatoshis",
                deficit.into_u64()
            ),
            Self::FeeBelowMinimumRate { fee, required } => write!(
                f,
                "transaction fee {} is below the minimum {} for its size",
                fee.into_u64(),
                required.into_u64()
            ),
            Self::UnpaidActions { unpaid, limit } => write!(
                f,
                "transaction has {unpaid} unpaid actions, above the mempool limit of {limit}"
            ),
        }
    }
}

impl Rejection {
    /// Returns the rejection as the gRPC status zaino would return.
    pub fn status(&self) -> Status {
        zaino_send_error(ZAINO_REJECTION_CODE, &self.to_string())
    }
}

fn zaino_send_error(code: i32, text: &str) -> Status {
    Status::internal(format!("{ZAINO_SEND_ERROR_PREFIX} (code: {code}): {text}"))
}

/// Applies zebra's ZIP 317 mempool checks: minimum fee rate and unpaid action limit.
pub fn check_zip317_mempool_policy(
    logical_actions: usize,
    fee: Zatoshis,
    size_bytes: usize,
) -> Result<(), Rejection> {
    let required = Zatoshis::const_from_u64(
        (size_bytes as u64 * MIN_MEMPOOL_FEE_RATE_ZAT_PER_KB).div_ceil(BYTES_PER_KB),
    );
    if fee < required {
        return Err(Rejection::FeeBelowMinimumRate { fee, required });
    }
    let conventional_actions = logical_actions.max(GRACE_ACTIONS);
    let paid_actions = usize::try_from(fee.into_u64() / MARGINAL_FEE.into_u64())
        .expect("paid actions fit in usize");
    let unpaid = conventional_actions.saturating_sub(paid_actions);
    if unpaid > BLOCK_UNPAID_ACTION_LIMIT {
        return Err(Rejection::UnpaidActions {
            unpaid,
            limit: BLOCK_UNPAID_ACTION_LIMIT,
        });
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
/// A `CompactTxStreamer` method a fault can target.
pub enum Rpc {
    /// `GetLatestBlock`.
    LatestBlock,
    /// `GetBlock` and `GetBlockNullifiers`.
    Block,
    /// `GetBlockRange` and `GetBlockRangeNullifiers`.
    BlockRange,
    /// `GetTreeState`.
    TreeState,
    /// `GetLatestTreeState`.
    LatestTreeState,
    /// `GetSubtreeRoots`.
    SubtreeRoots,
    /// `GetTransaction`.
    Transaction,
    /// `SendTransaction`.
    SendTransaction,
    /// `GetTaddressTxids` and `GetTaddressTransactions`.
    TaddressTxids,
    /// `GetMempoolTx`.
    MempoolTx,
    /// `GetMempoolStream`.
    MempoolStream,
    /// `GetLightdInfo`.
    LightdInfo,
    /// `GetTaddressBalance` and its stream variant.
    TaddressBalance,
    /// `GetAddressUtxos` and its stream variant.
    AddressUtxos,
    /// `Ping`.
    Ping,
}

#[derive(Clone, Debug, PartialEq, Eq)]
/// One injected failure, consumed by the next call to its RPC.
pub enum Fault {
    /// A call that fails with this gRPC status.
    Fail(Code, String),
    /// A call that is delayed by this duration before it answers.
    Delay(Duration),
    /// A stream that ends with an EOF error after this many items.
    TruncateStream {
        /// The number of items streamed before the error.
        after: usize,
    },
    /// A stream that ends cleanly after this many items.
    EndStream {
        /// The number of items streamed before the end.
        after: usize,
    },
}

#[derive(Debug, Default)]
/// Per-RPC queues of injected faults.
pub struct Faults {
    queued: HashMap<Rpc, VecDeque<Fault>>,
}

impl Faults {
    /// Queues a fault for the next call to `rpc`.
    pub fn inject(&mut self, rpc: Rpc, fault: Fault) {
        self.queued.entry(rpc).or_default().push_back(fault);
    }

    /// Returns the number of faults still queued for `rpc`.
    pub fn pending(&self, rpc: Rpc) -> usize {
        self.queued.get(&rpc).map_or(0, VecDeque::len)
    }

    /// Removes every queued fault for `rpc` and returns how many were removed.
    pub fn clear(&mut self, rpc: Rpc) -> usize {
        self.queued.remove(&rpc).map_or(0, |queued| queued.len())
    }

    fn take(&mut self, rpc: Rpc) -> Option<Fault> {
        self.queued.get_mut(&rpc).and_then(VecDeque::pop_front)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Roots {
    sapling: sapling_crypto::Anchor,
    orchard: orchard::Anchor,
    ironwood: orchard::Anchor,
}

impl Roots {
    fn contains(&self, anchor: PoolAnchor) -> bool {
        match anchor {
            PoolAnchor::Sapling(anchor) => self.sapling == anchor,
            PoolAnchor::Orchard(anchor) => self.orchard == anchor,
            PoolAnchor::Ironwood(anchor) => self.ironwood == anchor,
        }
    }
}

struct Mined {
    height: BlockHeight,
    bytes: Vec<u8>,
}

type MempoolSubscriber = mpsc::UnboundedSender<Result<RawTransaction, Status>>;

#[derive(Default)]
struct MempoolState {
    nullifiers: BTreeSet<PoolNullifier>,
    spends: HashSet<OutPoint>,
    outputs: HashMap<OutPoint, TxOut>,
}

/// The fabricated chain: blocks, transactions, tree states, mempool.
pub struct MockChain {
    chain_type: ChainType,
    /// `blocks[i]` is the block at height `i + 1`.
    blocks: Vec<CompactBlock>,
    transactions: HashMap<TxId, Mined>,
    /// `tree_states[h]` is the serialized (sapling, orchard, ironwood)
    /// tree state hex as of the END of height `h`. Index 0 is the empty
    /// pre-chain state.
    tree_states: Vec<(String, String, String)>,
    roots: Vec<Roots>,
    sapling_tree: SaplingTree,
    orchard_tree: OrchardTree,
    /// The Ironwood pool's note commitment tree is orchard-shaped (the
    /// pool shares the Orchard cryptography).
    ironwood_tree: OrchardTree,
    mempool: Vec<Vec<u8>>,
    mempool_subscribers: Vec<MempoolSubscriber>,
    revealed_nullifiers: BTreeSet<PoolNullifier>,
    spent_outpoints: HashSet<OutPoint>,
    /// The checks applied to submitted transactions.
    pub rules: Rules,
    /// The queued RPC faults.
    pub faults: Faults,
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
    /// Standing fault: every `send_transaction` is refused with an internal status.
    pub reject_all_sends: bool,
    /// Standing fault: every `send_transaction` is answered with this error code.
    pub answer_sends_with_error_code: Option<i32>,
    /// How many submissions the standing faults refused.
    pub rejected_sends: u32,
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

fn tree_roots(
    sapling_tree: &SaplingTree,
    orchard_tree: &OrchardTree,
    ironwood_tree: &OrchardTree,
) -> Roots {
    Roots {
        sapling: sapling_crypto::Anchor::from(sapling_tree.root()),
        orchard: orchard::Anchor::from(orchard_tree.root()),
        ironwood: orchard::Anchor::from(ironwood_tree.root()),
    }
}

fn nullifiers_of(transaction: &Transaction) -> Vec<PoolNullifier> {
    let sapling = transaction
        .sapling_bundle()
        .into_iter()
        .flat_map(|bundle| bundle.shielded_spends())
        .map(|spend| PoolNullifier::Sapling(*spend.nullifier()));
    let orchard = transaction
        .orchard_bundle()
        .into_iter()
        .flat_map(|bundle| bundle.actions())
        .map(|action| PoolNullifier::Orchard(*action.nullifier()));
    let ironwood = transaction
        .ironwood_bundle()
        .into_iter()
        .flat_map(|bundle| bundle.actions())
        .map(|action| PoolNullifier::Ironwood(*action.nullifier()));
    sapling.chain(orchard).chain(ironwood).collect()
}

fn anchors_of(transaction: &Transaction) -> Vec<PoolAnchor> {
    let sapling = transaction
        .sapling_bundle()
        .into_iter()
        .flat_map(|bundle| bundle.shielded_spends())
        .map(|spend| PoolAnchor::Sapling(sapling_crypto::Anchor::from(*spend.anchor())));
    let orchard = transaction
        .orchard_bundle()
        .map(|bundle| PoolAnchor::Orchard(*bundle.anchor()));
    let ironwood = transaction
        .ironwood_bundle()
        .map(|bundle| PoolAnchor::Ironwood(*bundle.anchor()));
    sapling.chain(orchard).chain(ironwood).collect()
}

fn spent_outpoints_of(transaction: &Transaction) -> Vec<OutPoint> {
    transaction
        .transparent_bundle()
        .filter(|bundle| !bundle.is_coinbase())
        .into_iter()
        .flat_map(|bundle| bundle.vin.iter())
        .map(|txin| txin.prevout().clone())
        .collect()
}

fn logical_actions(transaction: &Transaction) -> usize {
    let (transparent_inputs, transparent_outputs) = transaction
        .transparent_bundle()
        .map_or((0, 0), |bundle| (bundle.vin.len(), bundle.vout.len()));
    let (sapling_spends, sapling_outputs) = transaction.sapling_bundle().map_or((0, 0), |bundle| {
        (
            bundle.shielded_spends().len(),
            bundle.shielded_outputs().len(),
        )
    });
    let orchard_actions = transaction
        .orchard_bundle()
        .map_or(0, |bundle| bundle.actions().len());
    let ironwood_actions = transaction
        .ironwood_bundle()
        .map_or(0, |bundle| bundle.actions().len());
    transparent_inputs.max(transparent_outputs)
        + sapling_spends.max(sapling_outputs)
        + orchard_actions
        + ironwood_actions
}

fn shielded_value_balance(transaction: &Transaction) -> ZatBalance {
    let orchard = transaction
        .orchard_bundle()
        .map_or(ZatBalance::zero(), |bundle| *bundle.value_balance());
    let ironwood = transaction
        .ironwood_bundle()
        .map_or(ZatBalance::zero(), |bundle| *bundle.value_balance());
    (transaction.sapling_value_balance() + orchard + ironwood)
        .expect("bundle value balances stay within the money range")
}

fn transparent_output_total(transaction: &Transaction) -> Zatoshis {
    transaction
        .transparent_bundle()
        .into_iter()
        .flat_map(|bundle| bundle.vout.iter())
        .try_fold(Zatoshis::ZERO, |total, txout| total + txout.value())
        .expect("chain values stay within the money range")
}

fn coinbase_script_sig(height: BlockHeight) -> Script {
    let height = u32::from(height);
    let push = if (1..=SMALL_INTEGER_MAX).contains(&height) {
        vec![OP_SMALL_INTEGER_BASE + height as u8]
    } else {
        let mut little_endian = height.to_le_bytes().to_vec();
        while little_endian.len() > 1 && little_endian.last() == Some(&0) {
            little_endian.pop();
        }
        if little_endian
            .last()
            .is_some_and(|byte| byte & SIGN_BIT != 0)
        {
            little_endian.push(0);
        }
        let mut push = vec![little_endian.len() as u8];
        push.extend(little_endian);
        push
    };
    let mut encoded = vec![push.len() as u8];
    encoded.extend(push);
    Script::read(encoded.as_slice()).expect("a height push is a valid script")
}

/// Serializes an unsigned transparent-only transaction for the branch at `height`.
pub fn transparent_only_transaction(
    chain_type: &ChainType,
    height: BlockHeight,
    inputs: Vec<TxIn<TransparentAuthorized>>,
    outputs: Vec<TxOut>,
) -> Vec<u8> {
    let bundle = TransparentBundle {
        vin: inputs,
        vout: outputs,
        authorization: TransparentAuthorized,
    };
    let branch_id = BranchId::for_height(chain_type, height);
    let data = match TxVersion::suggested_for_branch(branch_id) {
        TxVersion::V6 => TransactionData::<Authorized>::from_parts_v6(
            branch_id,
            0,
            NO_EXPIRY,
            Some(bundle),
            None,
            None,
            None,
        ),
        version => TransactionData::<Authorized>::from_parts(
            version,
            branch_id,
            0,
            NO_EXPIRY,
            Some(bundle),
            None,
            None,
            None,
        ),
    };
    let mut bytes = vec![];
    data.freeze()
        .expect("a transparent-only transaction freezes")
        .write(&mut bytes)
        .expect("in-memory serialization is infallible");
    bytes
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
        Self::with_activation_heights(ActivationHeights::default())
    }

    /// Creates an empty regtest chain with the given activation schedule.
    pub fn with_activation_heights(activation_heights: ActivationHeights) -> Self {
        let sapling_tree = SaplingTree::empty();
        let orchard_tree = OrchardTree::empty();
        let ironwood_tree = OrchardTree::empty();
        let genesis_state = tree_state_hex(&sapling_tree, &orchard_tree, &ironwood_tree);
        let genesis_roots = tree_roots(&sapling_tree, &orchard_tree, &ironwood_tree);
        Self {
            chain_type: ChainType::Regtest(activation_heights),
            blocks: Vec::new(),
            transactions: HashMap::new(),
            tree_states: vec![genesis_state],
            roots: vec![genesis_roots],
            sapling_tree,
            orchard_tree,
            ironwood_tree,
            mempool: Vec::new(),
            mempool_subscribers: Vec::new(),
            revealed_nullifiers: BTreeSet::new(),
            spent_outpoints: HashSet::new(),
            rules: Rules::STRICT,
            faults: Faults::default(),
            download_queue: Vec::new(),
            lose_next_send_response: None,
            queued_rejections_before_promotion: 0,
            reject_all_sends: false,
            answer_sends_with_error_code: None,
            rejected_sends: 0,
            taddr_request_log: Vec::new(),
            branch_seed: 0,
        }
    }

    /// Returns the chain type.
    pub fn chain_type(&self) -> ChainType {
        self.chain_type
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

    /// The height of the next block.
    pub fn next_height(&self) -> BlockHeight {
        BlockHeight::from_u32(self.tip() + 1)
    }

    /// Returns the number of transactions in the mempool.
    pub fn mempool_len(&self) -> usize {
        self.mempool.len()
    }

    /// Validates `bytes` under the chain's rules and enters the mempool on success.
    pub fn submit_transaction(&mut self, bytes: Vec<u8>) -> Result<TxId, Rejection> {
        let transaction = self.validate(&bytes)?;
        let txid = transaction.txid();
        self.enter_mempool(bytes);
        Ok(txid)
    }

    /// Adds `bytes` to the mempool without validation.
    pub fn enter_mempool(&mut self, bytes: Vec<u8>) {
        let raw = RawTransaction {
            data: bytes.clone(),
            height: MEMPOOL_RAW_TRANSACTION_HEIGHT,
        };
        self.mempool_subscribers
            .retain(|subscriber| subscriber.send(Ok(raw.clone())).is_ok());
        self.mempool.push(bytes);
    }

    /// Checks `bytes` against the chain's rules for the next block.
    pub fn validate(&self, bytes: &[u8]) -> Result<Transaction, Rejection> {
        let next_height = self.next_height();
        let expected_branch = BranchId::for_height(&self.chain_type, next_height);
        let transaction = Transaction::read(bytes, expected_branch)
            .map_err(|error| Rejection::Unparseable(error.to_string()))?;

        if self.rules.branch_id
            && matches!(transaction.version(), TxVersion::V5 | TxVersion::V6)
            && transaction.consensus_branch_id() != expected_branch
        {
            return Err(Rejection::BranchId {
                expected: expected_branch,
                found: transaction.consensus_branch_id(),
            });
        }

        let expiry = transaction.expiry_height();
        if self.rules.expiry && expiry != NO_EXPIRY && expiry < next_height {
            return Err(Rejection::Expired {
                expiry,
                next_height,
            });
        }

        if self.rules.anchors {
            for anchor in anchors_of(&transaction) {
                if !self.roots.iter().any(|roots| roots.contains(anchor)) {
                    return Err(Rejection::UnknownAnchor(anchor));
                }
            }
        }

        let mempool = self.mempool_state(next_height);
        if self.rules.nullifiers {
            for nullifier in nullifiers_of(&transaction) {
                if self.revealed_nullifiers.contains(&nullifier)
                    || mempool.nullifiers.contains(&nullifier)
                {
                    return Err(Rejection::DuplicateNullifier(nullifier));
                }
            }
        }

        let transparent_input_total =
            self.check_transparent_inputs(&transaction, next_height, &mempool)?;

        if let (true, Some(input_total)) = (self.rules.fees, transparent_input_total) {
            check_fee(&transaction, input_total, bytes.len())?;
        }

        Ok(transaction)
    }

    fn mempool_state(&self, next_height: BlockHeight) -> MempoolState {
        let mut state = MempoolState::default();
        for bytes in self.mempool.iter().chain(self.download_queue.iter()) {
            let transaction = self.parse_transaction(bytes, next_height);
            state.nullifiers.extend(nullifiers_of(&transaction));
            state.spends.extend(spent_outpoints_of(&transaction));
            let txid = transaction.txid();
            for (n, output) in transaction
                .transparent_bundle()
                .into_iter()
                .flat_map(|bundle| bundle.vout.iter().enumerate())
            {
                state
                    .outputs
                    .insert(OutPoint::new(*txid.as_ref(), n as u32), output.clone());
            }
        }
        state
    }

    fn check_transparent_inputs(
        &self,
        transaction: &Transaction,
        next_height: BlockHeight,
        mempool: &MempoolState,
    ) -> Result<Option<Zatoshis>, Rejection> {
        let Some(bundle) = transaction.transparent_bundle() else {
            return Ok(Some(Zatoshis::ZERO));
        };
        if bundle.is_coinbase() {
            return Ok(Some(Zatoshis::ZERO));
        }
        let mut total = Zatoshis::ZERO;
        let mut all_known = true;
        for txin in &bundle.vin {
            let outpoint = txin.prevout();
            let Some(previous_output) = self.previous_output(outpoint, next_height, mempool)?
            else {
                if self.rules.transparent_inputs {
                    return Err(Rejection::UnknownInput(outpoint.clone()));
                }
                all_known = false;
                continue;
            };
            if self.rules.transparent_inputs
                && (self.spent_outpoints.contains(outpoint) || mempool.spends.contains(outpoint))
            {
                return Err(Rejection::SpentInput(outpoint.clone()));
            }
            total = (total + previous_output.value())
                .expect("chain values stay within the money range");
        }
        Ok(all_known.then_some(total))
    }

    fn previous_output(
        &self,
        outpoint: &OutPoint,
        next_height: BlockHeight,
        mempool: &MempoolState,
    ) -> Result<Option<TxOut>, Rejection> {
        let Some(mined) = self.transactions.get(outpoint.txid()) else {
            return Ok(mempool.outputs.get(outpoint).cloned());
        };
        let previous = self.parse_transaction(&mined.bytes, mined.height);
        let Some(previous_bundle) = previous.transparent_bundle() else {
            return Ok(None);
        };
        if self.rules.coinbase_maturity
            && previous_bundle.is_coinbase()
            && next_height - mined.height < COINBASE_MATURITY_BLOCKS
        {
            return Err(Rejection::ImmatureCoinbase {
                mined_at: mined.height,
                next_height,
            });
        }
        Ok(previous_bundle.vout.get(outpoint.n() as usize).cloned())
    }

    /// The validator finishing verification: queued transactions enter
    /// the mempool.
    pub fn promote_download_queue(&mut self) {
        let verified = std::mem::take(&mut self.download_queue);
        for bytes in verified {
            self.enter_mempool(bytes);
        }
    }

    /// Mines every mempool transaction into the next block.
    pub fn mine_mempool(&mut self) {
        let pending = std::mem::take(&mut self.mempool);
        self.mine_block(pending);
    }

    /// Mines the given raw transactions (plus nothing else) into the
    /// next block.
    pub fn mine_block(&mut self, raw_transactions: Vec<Vec<u8>>) {
        self.mine(None, raw_transactions);
    }

    /// Mines the given raw transactions (plus a coinbase transaction).
    pub fn mine_block_rewarding(
        &mut self,
        miner: &str,
        reward: Zatoshis,
        raw_transactions: Vec<Vec<u8>>,
    ) -> Vec<u8> {
        let coinbase = self.coinbase_transaction(miner, reward);
        self.mine(Some(coinbase.clone()), raw_transactions);
        coinbase
    }

    fn coinbase_transaction(&self, miner: &str, reward: Zatoshis) -> Vec<u8> {
        let height = self.next_height();
        let script_pubkey = taddr_script(miner, &self.chain_type)
            .expect("the miner address is a transparent address of this chain");
        let input = TxIn::from_parts(OutPoint::NULL, coinbase_script_sig(height), u32::MAX);
        let output = TxOut::new(reward, script_pubkey);
        transparent_only_transaction(&self.chain_type, height, vec![input], vec![output])
    }

    fn mine(&mut self, coinbase: Option<Vec<u8>>, raw_transactions: Vec<Vec<u8>>) {
        let height = self.next_height();
        let mut vtx = Vec::new();
        let coinbase_slot = coinbase.into_iter().map(|bytes| (0u64, bytes));
        let ordinary = raw_transactions
            .into_iter()
            .enumerate()
            .map(|(i, bytes)| (i as u64 + 1, bytes));
        for (index, bytes) in coinbase_slot.chain(ordinary) {
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
            self.revealed_nullifiers.extend(nullifiers_of(&transaction));
            self.spent_outpoints
                .extend(spent_outpoints_of(&transaction));
            vtx.push(compact_transaction(index, &transaction));
            self.transactions
                .insert(transaction.txid(), Mined { height, bytes });
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
            height: u64::from(u32::from(height)),
            hash: fabricated_branch_hash(u32::from(height), self.branch_seed),
            prev_hash,
            time: BLOCK_TIME_BASE + u32::from(height),
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
        self.roots.push(tree_roots(
            &self.sapling_tree,
            &self.orchard_tree,
            &self.ironwood_tree,
        ));
        self.mempool_subscribers.clear();
        self.evict_expired_from_mempool();
    }

    fn evict_expired_from_mempool(&mut self) {
        let next_height = self.next_height();
        let chain_type = self.chain_type;
        self.mempool.retain(|bytes| {
            let expiry = Transaction::read(
                bytes.as_slice(),
                BranchId::for_height(&chain_type, next_height),
            )
            .expect("mempool transactions parse")
            .expiry_height();
            expiry == NO_EXPIRY || expiry >= next_height
        });
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
        self.roots.truncate(height as usize + 1);
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
        let cutoff = BlockHeight::from_u32(height);
        let mut evicted: Vec<(BlockHeight, Vec<u8>)> = Vec::new();
        self.transactions.retain(|_, mined| {
            if mined.height > cutoff {
                evicted.push((mined.height, mined.bytes.clone()));
                false
            } else {
                true
            }
        });
        evicted.sort_by_key(|(mined_height, _)| *mined_height);
        self.rebuild_spend_sets();
        self.branch_seed += 1;
        evicted.into_iter().map(|(_, bytes)| bytes).collect()
    }

    fn rebuild_spend_sets(&mut self) {
        self.revealed_nullifiers.clear();
        self.spent_outpoints.clear();
        for compact in self.blocks.iter().flat_map(|block| block.vtx.iter()) {
            let sapling = compact
                .spends
                .iter()
                .map(|spend| PoolNullifier::Sapling(sapling_crypto::Nullifier(bytes32(&spend.nf))));
            let orchard = compact
                .actions
                .iter()
                .map(|action| PoolNullifier::Orchard(orchard_nullifier(&action.nullifier)));
            let ironwood = compact
                .ironwood_actions
                .iter()
                .map(|action| PoolNullifier::Ironwood(orchard_nullifier(&action.nullifier)));
            self.revealed_nullifiers
                .extend(sapling.chain(orchard).chain(ironwood));
            let spent = compact
                .vin
                .iter()
                .map(|txin| OutPoint::new(bytes32(&txin.prevout_txid), txin.prevout_index))
                .filter(|outpoint| *outpoint != OutPoint::NULL);
            self.spent_outpoints.extend(spent);
        }
    }

    fn parse_transaction(&self, bytes: &[u8], height: BlockHeight) -> Transaction {
        Transaction::read(bytes, BranchId::for_height(&self.chain_type, height))
            .expect("fabricated and wallet-built transactions parse")
    }

    /// Returns the unspent transparent outputs paying `address` mined at or above `start_height`.
    pub fn unspent_outputs(
        &self,
        address: &str,
        start_height: BlockHeight,
    ) -> Result<Vec<GetAddressUtxosReply>, String> {
        let target_script = taddr_script(address, &self.chain_type)?;
        let mut unspent = Vec::new();
        for (txid, mined) in &self.transactions {
            if mined.height < start_height {
                continue;
            }
            let transaction = self.parse_transaction(&mined.bytes, mined.height);
            let Some(bundle) = transaction.transparent_bundle() else {
                continue;
            };
            for (index, txout) in bundle.vout.iter().enumerate() {
                let outpoint = OutPoint::new(*txid.as_ref(), index as u32);
                if *txout.script_pubkey() != target_script
                    || self.spent_outpoints.contains(&outpoint)
                {
                    continue;
                }
                unspent.push(GetAddressUtxosReply {
                    address: address.to_string(),
                    txid: txid.as_ref().to_vec(),
                    index: index as i32,
                    script: target_script.0.0.clone(),
                    value_zat: i64::from(ZatBalance::from(txout.value())),
                    height: u64::from(u32::from(mined.height)),
                });
            }
        }
        unspent.sort_by(|a, b| (a.height, &a.txid, a.index).cmp(&(b.height, &b.txid, b.index)));
        Ok(unspent)
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
            time: BLOCK_TIME_BASE + height,
            sapling_tree,
            orchard_tree,
            ironwood_tree,
        })
    }

    fn tip_hash(&self) -> Vec<u8> {
        self.blocks
            .last()
            .map_or_else(|| fabricated_block_hash(0), |block| block.hash.clone())
    }
}

fn check_fee(
    transaction: &Transaction,
    transparent_input_total: Zatoshis,
    size_bytes: usize,
) -> Result<(), Rejection> {
    let fee = (ZatBalance::from(transparent_input_total)
        - ZatBalance::from(transparent_output_total(transaction))
        + shielded_value_balance(transaction))
    .expect("chain values stay within the money range");
    let fee = Zatoshis::try_from(fee).map_err(|_| {
        Rejection::NegativeFee(Zatoshis::const_from_u64(i64::from(fee).unsigned_abs()))
    })?;
    check_zip317_mempool_policy(logical_actions(transaction), fee, size_bytes)
}

fn bytes32(raw: &[u8]) -> [u8; 32] {
    <[u8; 32]>::try_from(raw).expect("compact block hashes and nullifiers are 32 bytes")
}

fn orchard_nullifier(raw: &[u8]) -> orchard::note::Nullifier {
    Option::from(orchard::note::Nullifier::from_bytes(&bytes32(raw)))
        .expect("a mined orchard nullifier is a valid field element")
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

fn stream_with_fault<T: Send + 'static>(items: Vec<T>, fault: Option<Fault>) -> ResponseStream<T> {
    let items: Vec<Result<T, Status>> = match fault {
        Some(Fault::TruncateStream { after }) => items
            .into_iter()
            .take(after)
            .map(Ok)
            .chain(std::iter::once(Err(Status::unavailable(
                STREAM_EOF_MESSAGE,
            ))))
            .collect(),
        Some(Fault::EndStream { after }) => items.into_iter().take(after).map(Ok).collect(),
        _ => items.into_iter().map(Ok).collect(),
    };
    Box::pin(tokio_stream::iter(items))
}

/// Serves a [`MockChain`] over the `CompactTxStreamer` protocol.
pub struct MockIndexerService {
    chain: Arc<RwLock<MockChain>>,
}

impl MockIndexerService {
    /// Creates a service that serves `chain`.
    pub fn new(chain: Arc<RwLock<MockChain>>) -> Self {
        Self { chain }
    }

    async fn fault_for(&self, rpc: Rpc) -> Result<Option<Fault>, Status> {
        let fault = self.chain.write().await.faults.take(rpc);
        match fault {
            Some(Fault::Fail(code, message)) => Err(Status::new(code, message)),
            Some(Fault::Delay(delay)) => {
                tokio::time::sleep(delay).await;
                Ok(None)
            }
            other => Ok(other),
        }
    }

    async fn address_utxos(
        &self,
        arg: GetAddressUtxosArg,
    ) -> Result<Vec<GetAddressUtxosReply>, Status> {
        let chain = self.chain.read().await;
        let start_height = BlockHeight::from_u32(arg.start_height as u32);
        let mut address_utxos = Vec::new();
        for address in &arg.addresses {
            address_utxos.extend(
                chain
                    .unspent_outputs(address, start_height)
                    .map_err(Status::invalid_argument)?,
            );
        }
        address_utxos
            .sort_by(|a, b| (a.height, &a.txid, a.index).cmp(&(b.height, &b.txid, b.index)));
        if arg.max_entries > 0 {
            address_utxos.truncate(arg.max_entries as usize);
        }
        Ok(address_utxos)
    }
}

#[tonic::async_trait]
impl CompactTxStreamer for MockIndexerService {
    async fn get_latest_block(
        &self,
        _request: Request<ChainSpec>,
    ) -> Result<Response<BlockId>, Status> {
        self.fault_for(Rpc::LatestBlock).await?;
        let chain = self.chain.read().await;
        Ok(Response::new(BlockId {
            height: u64::from(chain.tip()),
            hash: chain.tip_hash(),
        }))
    }

    async fn get_block(&self, request: Request<BlockId>) -> Result<Response<CompactBlock>, Status> {
        self.fault_for(Rpc::Block).await?;
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
        let fault = self.fault_for(Rpc::BlockRange).await?;
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
        Ok(Response::new(stream_with_fault(blocks, fault)))
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
        self.fault_for(Rpc::TreeState).await?;
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
        self.fault_for(Rpc::LatestTreeState).await?;
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
        let fault = self.fault_for(Rpc::SubtreeRoots).await?;
        // A short regtest chain has no completed shards; an empty stream
        // is exactly what zainod serves in the live suites.
        Ok(Response::new(stream_with_fault(Vec::new(), fault)))
    }

    async fn get_transaction(
        &self,
        request: Request<TxFilter>,
    ) -> Result<Response<RawTransaction>, Status> {
        self.fault_for(Rpc::Transaction).await?;
        let txid: [u8; 32] = request
            .into_inner()
            .hash
            .try_into()
            .map_err(|_| Status::invalid_argument("txid must be 32 bytes"))?;
        let chain = self.chain.read().await;
        chain
            .transactions
            .get(&TxId::from_bytes(txid))
            .map(|mined| {
                Response::new(RawTransaction {
                    data: mined.bytes.clone(),
                    height: u64::from(u32::from(mined.height)),
                })
            })
            .ok_or_else(|| Status::not_found("transaction not mined on the mock chain"))
    }

    async fn send_transaction(
        &self,
        request: Request<RawTransaction>,
    ) -> Result<Response<SendResponse>, Status> {
        self.fault_for(Rpc::SendTransaction).await?;
        let mut chain = self.chain.write().await;
        let bytes = request.into_inner().data;
        if chain.reject_all_sends {
            chain.rejected_sends += 1;
            return Err(Status::internal(
                "mock fault: this Destination suppresses every transaction",
            ));
        }
        if let Some(error_code) = chain.answer_sends_with_error_code {
            chain.rejected_sends += 1;
            return Ok(Response::new(SendResponse {
                error_code,
                error_message: "mock fault: this Destination rejects the transaction".to_string(),
            }));
        }
        // The validator rejects resubmitted bytes rather than
        // re-accepting them, with a phase-specific message; reproduce
        // both rejections verbatim as zainod 0.6.0-rc.1 surfaces them
        // (zingolabs/zaino#1392), so client handling is exercised
        // against the messages it really receives.
        if chain.mempool.contains(&bytes) {
            return Err(zaino_send_error(
                ZAINO_DUPLICATE_CODE,
                "transaction already exists in mempool",
            ));
        }
        if chain.download_queue.contains(&bytes) {
            if chain.queued_rejections_before_promotion == 0 {
                // Verification finished between probes: the queue
                // promotes and this probe already sees the
                // mempool-phase rejection.
                chain.promote_download_queue();
                return Err(zaino_send_error(
                    ZAINO_DUPLICATE_CODE,
                    "transaction already exists in mempool",
                ));
            }
            chain.queued_rejections_before_promotion -= 1;
            return Err(zaino_send_error(
                ZAINO_DUPLICATE_CODE,
                "transaction dropped because it is already queued for download",
            ));
        }
        // The fault of issue #2450: delivery happened, the caller
        // never learns.
        match chain.lose_next_send_response.take() {
            Some(LostSendDestination::Mempool) => {
                chain
                    .submit_transaction(bytes)
                    .map_err(|rejection| rejection.status())?;
                Err(Status::deadline_exceeded(
                    "mock fault: response lost after mempool acceptance",
                ))
            }
            Some(LostSendDestination::DownloadQueue) => {
                chain
                    .validate(&bytes)
                    .map_err(|rejection| rejection.status())?;
                chain.download_queue.push(bytes);
                Err(Status::deadline_exceeded(
                    "mock fault: response lost while queued for download",
                ))
            }
            None => {
                let txid = chain
                    .submit_transaction(bytes)
                    .map_err(|rejection| rejection.status())?;
                Ok(Response::new(SendResponse {
                    error_code: 0,
                    error_message: txid.to_string(),
                }))
            }
        }
    }

    type GetTaddressTxidsStream = ResponseStream<RawTransaction>;
    async fn get_taddress_txids(
        &self,
        request: Request<TransparentAddressBlockFilter>,
    ) -> Result<Response<Self::GetTaddressTxidsStream>, Status> {
        let fault = self.fault_for(Rpc::TaddressTxids).await?;
        let filter = request.into_inner();
        let range = filter
            .range
            .ok_or_else(|| Status::invalid_argument("missing block range"))?;
        let start = BlockHeight::from_u32(range.start.map_or(1, |id| id.height) as u32);
        let end = BlockHeight::from_u32(range.end.map_or(u64::MAX, |id| id.height) as u32);
        let chain = self.chain.read().await;
        let target_script =
            taddr_script(&filter.address, &chain.chain_type).map_err(Status::invalid_argument)?;
        // A transaction involves the address if an output pays its
        // script, or an input spends an outpoint that paid its script.
        let mut involved: Vec<(BlockHeight, usize, Vec<u8>)> = Vec::new();
        for mined in chain.transactions.values() {
            if mined.height < start || mined.height > end {
                continue;
            }
            let transaction = chain.parse_transaction(&mined.bytes, mined.height);
            let Some(bundle) = transaction.transparent_bundle() else {
                continue;
            };
            let pays = bundle
                .vout
                .iter()
                .any(|txout| *txout.script_pubkey() == target_script);
            let spends = bundle.vin.iter().any(|txin| {
                chain
                    .transactions
                    .get(txin.prevout().txid())
                    .and_then(|previous| {
                        let prev = chain.parse_transaction(&previous.bytes, previous.height);
                        prev.transparent_bundle().and_then(|prev_bundle| {
                            prev_bundle
                                .vout
                                .get(txin.prevout().n() as usize)
                                .map(|prev_out| *prev_out.script_pubkey() == target_script)
                        })
                    })
                    .unwrap_or(false)
            });
            if pays || spends {
                let block = &chain.blocks[u32::from(mined.height) as usize - 1];
                let index = block
                    .vtx
                    .iter()
                    .position(|compact| compact.txid == transaction.txid().as_ref())
                    .expect("mined transactions appear in their block");
                involved.push((mined.height, index, mined.bytes.clone()));
            }
        }
        involved.sort_by_key(|(height, index, _)| (*height, *index));
        let raw: Vec<_> = involved
            .into_iter()
            .map(|(height, _, data)| RawTransaction {
                data,
                height: u64::from(u32::from(height)),
            })
            .collect();
        let served = raw.len();
        drop(chain);
        self.chain.write().await.taddr_request_log.push(format!(
            "address={} range=[{start},{end}] served={served}",
            filter.address
        ));
        Ok(Response::new(stream_with_fault(raw, fault)))
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
        let fault = self.fault_for(Rpc::MempoolTx).await?;
        let chain = self.chain.read().await;
        let height = chain.next_height();
        let compact: Vec<_> = chain
            .mempool
            .iter()
            .enumerate()
            .map(|(i, bytes)| {
                compact_transaction(i as u64 + 1, &chain.parse_transaction(bytes, height))
            })
            .collect();
        Ok(Response::new(stream_with_fault(compact, fault)))
    }

    type GetMempoolStreamStream = ResponseStream<RawTransaction>;
    async fn get_mempool_stream(
        &self,
        _request: Request<Empty>,
    ) -> Result<Response<Self::GetMempoolStreamStream>, Status> {
        self.fault_for(Rpc::MempoolStream).await?;
        let mut chain = self.chain.write().await;
        let (sender, receiver) = mpsc::unbounded_channel();
        for bytes in &chain.mempool {
            let _already_closed = sender.send(Ok(RawTransaction {
                data: bytes.clone(),
                height: MEMPOOL_RAW_TRANSACTION_HEIGHT,
            }));
        }
        chain.mempool_subscribers.push(sender);
        Ok(Response::new(Box::pin(
            tokio_stream::wrappers::UnboundedReceiverStream::new(receiver),
        )))
    }

    async fn get_lightd_info(
        &self,
        _request: Request<Empty>,
    ) -> Result<Response<LightdInfo>, Status> {
        self.fault_for(Rpc::LightdInfo).await?;
        let chain = self.chain.read().await;
        let tip = chain.tip();
        let branch_id: u32 =
            BranchId::for_height(&chain.chain_type, BlockHeight::from_u32(tip.max(1))).into();
        let sapling_activation_height = chain
            .chain_type
            .activation_height(NetworkUpgrade::Sapling)
            .map_or(0, |height| u64::from(u32::from(height)));
        Ok(Response::new(LightdInfo {
            version: "mock-indexer".to_string(),
            vendor: "zingolib test".to_string(),
            taddr_support: true,
            chain_name: "regtest".to_string(),
            sapling_activation_height,
            consensus_branch_id: format!("{branch_id:08x}"),
            block_height: u64::from(tip),
            estimated_height: u64::from(tip),
            ..Default::default()
        }))
    }

    async fn get_taddress_balance(
        &self,
        request: Request<AddressList>,
    ) -> Result<Response<Balance>, Status> {
        self.fault_for(Rpc::TaddressBalance).await?;
        let chain = self.chain.read().await;
        let mut value_zat = 0i64;
        for address in &request.into_inner().addresses {
            value_zat += chain
                .unspent_outputs(address, GENESIS_HEIGHT)
                .map_err(Status::invalid_argument)?
                .iter()
                .map(|utxo| utxo.value_zat)
                .sum::<i64>();
        }
        Ok(Response::new(Balance { value_zat }))
    }

    async fn get_taddress_balance_stream(
        &self,
        request: Request<tonic::Streaming<Address>>,
    ) -> Result<Response<Balance>, Status> {
        self.fault_for(Rpc::TaddressBalance).await?;
        let mut addresses = request.into_inner();
        let chain = self.chain.read().await;
        let mut value_zat = 0i64;
        while let Some(address) = addresses.message().await? {
            value_zat += chain
                .unspent_outputs(&address.address, GENESIS_HEIGHT)
                .map_err(Status::invalid_argument)?
                .iter()
                .map(|utxo| utxo.value_zat)
                .sum::<i64>();
        }
        Ok(Response::new(Balance { value_zat }))
    }

    async fn get_address_utxos(
        &self,
        request: Request<GetAddressUtxosArg>,
    ) -> Result<Response<GetAddressUtxosReplyList>, Status> {
        self.fault_for(Rpc::AddressUtxos).await?;
        let address_utxos = self.address_utxos(request.into_inner()).await?;
        Ok(Response::new(GetAddressUtxosReplyList { address_utxos }))
    }

    type GetAddressUtxosStreamStream = ResponseStream<GetAddressUtxosReply>;
    async fn get_address_utxos_stream(
        &self,
        request: Request<GetAddressUtxosArg>,
    ) -> Result<Response<Self::GetAddressUtxosStreamStream>, Status> {
        let fault = self.fault_for(Rpc::AddressUtxos).await?;
        let address_utxos = self.address_utxos(request.into_inner()).await?;
        Ok(Response::new(stream_with_fault(address_utxos, fault)))
    }

    async fn ping(
        &self,
        request: Request<ProtoDuration>,
    ) -> Result<Response<PingResponse>, Status> {
        self.fault_for(Rpc::Ping).await?;
        let entry = unix_micros();
        let interval = u64::try_from(request.into_inner().interval_us).unwrap_or(0);
        tokio::time::sleep(Duration::from_micros(interval)).await;
        Ok(Response::new(PingResponse {
            entry,
            exit: unix_micros(),
        }))
    }
}

fn unix_micros() -> i64 {
    i64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("the clock is after the epoch")
            .as_micros(),
    )
    .expect("microseconds since the epoch fit in i64")
}

fn taddr_script(address: &str, chain_type: &ChainType) -> Result<Script, String> {
    use pepper_sync::keys::decode_address;
    use zcash_client_backend::address::Address as DecodedAddress;

    match decode_address(chain_type, address) {
        Ok(DecodedAddress::Transparent(taddr)) => Ok(taddr.script().into()),
        Ok(_) => Err(format!("{address} is not a transparent address")),
        Err(e) => Err(format!("undecodable address {address}: {e:?}")),
    }
}

/// A launched mock network: the chain handle, the serving task, and a
/// factory for `LightClient`s dialed at it.
pub struct MockNet {
    /// The fabricated chain the server reads and the test mutates.
    pub chain: Arc<RwLock<MockChain>>,
    chain_type: ChainType,
    indexer_uri: http::Uri,
    addr: std::net::SocketAddr,
    wallet_dirs: Vec<tempfile::TempDir>,
    _server: tokio::task::JoinHandle<()>,
}

impl MockNet {
    /// Launches the mock server on an ephemeral localhost port with an
    /// empty chain.
    pub async fn launch() -> Self {
        Self::launch_with(MockChain::new()).await
    }

    /// Launches the mock server on an ephemeral localhost port serving `chain`.
    pub async fn launch_with(chain: MockChain) -> Self {
        Self::serve(chain, None).await
    }

    /// Launches the mock over TLS with the committed localhost certificate.
    pub async fn launch_tls() -> Self {
        zingo_netutils::ensure_default_crypto_provider();
        Self::serve(
            MockChain::new(),
            Some(tonic::transport::Identity::from_pem(
                zingo_netutils::test_tls::LOCALHOST_CERT_PEM,
                zingo_netutils::test_tls::LOCALHOST_KEY_PEM,
            )),
        )
        .await
    }

    async fn serve(chain: MockChain, tls: Option<tonic::transport::Identity>) -> Self {
        let chain_type = chain.chain_type();
        let chain = Arc::new(RwLock::new(chain));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("an ephemeral localhost port binds");
        let addr = listener.local_addr().expect("bound socket has an address");
        let service = MockIndexerService::new(chain.clone());
        let scheme = if tls.is_some() { "https" } else { "http" };
        let mut builder = tonic::transport::Server::builder();
        if let Some(identity) = tls {
            builder = builder
                .tls_config(tonic::transport::ServerTlsConfig::new().identity(identity))
                .expect("the committed localhost identity loads");
        }
        let server = tokio::spawn(async move {
            builder
                .add_service(CompactTxStreamerServer::new(service))
                .serve_with_incoming(tokio_stream::wrappers::TcpListenerStream::new(listener))
                .await
                .ok();
        });
        let indexer_uri: http::Uri = format!("{scheme}://127.0.0.1:{}", addr.port())
            .parse()
            .expect("a localhost uri parses");
        Self {
            chain,
            chain_type,
            indexer_uri,
            addr,
            wallet_dirs: Vec::new(),
            _server: server,
        }
    }

    /// The socket the mock listens on.
    pub fn addr(&self) -> std::net::SocketAddr {
        self.addr
    }

    /// Returns the chain type.
    pub fn chain_type(&self) -> ChainType {
        self.chain_type
    }

    /// Returns the URI clients connect to.
    pub fn indexer_uri(&self) -> http::Uri {
        self.indexer_uri.clone()
    }

    fn config_builder(&mut self) -> ClientConfigBuilder {
        let wallet_dir = tempfile::tempdir().expect("a tempdir is creatable");
        let builder = ClientConfig::builder()
            .set_chain_type(self.chain_type)
            .set_indexer_uri(self.indexer_uri.clone())
            .set_wallet_dir(wallet_dir.path().to_path_buf());
        self.wallet_dirs.push(wallet_dir);
        builder
    }

    /// Builds a `LightClient` for `mnemonic` (birthday 1) dialed at the
    /// mock, with its wallet directory in a tempdir this net keeps
    /// alive.
    pub async fn client(
        &mut self,
        mnemonic: &str,
        wallet_settings_opt: Option<WalletSettings>,
    ) -> LightClient {
        let config = self
            .config_builder()
            .set_wallet_config(WalletConfig::MnemonicPhrase {
                mnemonic_phrase: mnemonic.to_string(),
                no_of_accounts: 1.try_into().expect("hard-coded non-zero"),
                birthday: 1,
                wallet_settings: wallet_settings_opt.unwrap_or_else(default_test_wallet_settings),
            })
            .build()
            .unwrap();
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
        attach_mock_mixnet(&mut lightclient).await;
        lightclient
    }

    /// Builds a `LightClient` from wallet file saved in wallet directory at position `client_index` in `self.wallet_dirs`.
    pub async fn client_from_file(&mut self, client_index: usize) -> LightClient {
        let wallet_dir = self
            .wallet_dirs
            .get(client_index)
            .expect("client at given index should exist");
        let config = ClientConfig::builder()
            .set_chain_type(self.chain_type)
            .set_indexer_uri(self.indexer_uri.clone())
            .set_wallet_dir(wallet_dir.path().to_path_buf())
            .set_wallet_config(WalletConfig::Read)
            .build()
            .unwrap();
        let mut lightclient = LightClient::new(config, true)
            .await
            .expect("mock-net client construction succeeds");
        attach_mock_mixnet(&mut lightclient).await;
        lightclient
    }
}

// Mock-net clients run with Mixnet Mode switched on, so every
// chain-mock send walks the fail-closed route resolver and the
// escalation orchestration instead of quietly consenting to clearnet.
// The address is never dialed: the transmit path pairs this slot
// state with arms that submit over the mock indexer's channel.
// Without the nym feature there is no mixnet and sends stay
// clearnet, so the same tests cover both routes across the
// feature matrix.
#[allow(unused_variables)]
async fn attach_mock_mixnet(lightclient: &mut LightClient) {
    #[cfg(feature = "nym")]
    lightclient
        .switch_on_mixnet_for_tests(crate::mocks::transmission::MOCK_SOCKS5_ADDR)
        .await;
}

impl ConductChain for MockNet {
    async fn setup() -> Self {
        Self::launch().await
    }

    fn lightserver_uri(&self) -> Option<http::Uri> {
        Some(self.indexer_uri.clone())
    }

    async fn create_faucet(&mut self) -> LightClient {
        let mut faucet = self.create_client().await;
        let address = get_base_address(&faucet, PoolType::Shielded(ShieldedPool::Orchard)).await;
        let funding = faucet_funding_transaction(vec![(&address, FAUCET_FUNDING, None)]).await;
        self.chain.write().await.mine_block(vec![funding]);
        faucet
            .sync_and_await()
            .await
            .expect("the faucet syncs its funding block");
        faucet
    }

    async fn zingo_config(&mut self) -> ClientConfig {
        self.config_builder()
            .set_wallet_config(WalletConfig::NewSeed {
                no_of_accounts: 1.try_into().expect("hard-coded non-zero"),
                chain_height: 1,
                wallet_settings: default_test_wallet_settings(),
            })
            .build()
            .unwrap()
    }

    async fn increase_chain_height(&mut self) {
        self.chain.write().await.mine_mempool();
    }

    fn confirmation_patience_blocks(&self) -> usize {
        1
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
        .ironwood_note(total + FAUCET_HEADROOM)
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

#[cfg(test)]
mod tests {
    use super::*;
    use tokio_stream::StreamExt as _;

    const REWARD: Zatoshis = Zatoshis::const_from_u64(1_000_000);
    const SPEND_VALUE: Zatoshis = Zatoshis::const_from_u64(990_000);
    const OVERSPEND_BY: Zatoshis = Zatoshis::const_from_u64(1);
    const PAYMENT: u64 = 10_000;
    const HEIGHT_ONE: BlockHeight = BlockHeight::from_u32(1);
    const HEIGHT_TWO: BlockHeight = BlockHeight::from_u32(2);

    fn external_orchard_address() -> String {
        let mut wallet =
            SyntheticWalletBuilder::new(zingo_test_vectors::seeds::HOSPITAL_MUSEUM_SEED).build();
        let (_, unified_address) = wallet
            .generate_unified_address(
                crate::wallet::keys::unified::ReceiverSelection::orchard_only(),
                zip32::AccountId::ZERO,
            )
            .unwrap();
        unified_address.encode(&wallet.chain_type())
    }

    fn external_transparent_address() -> String {
        let wallet =
            SyntheticWalletBuilder::new(zingo_test_vectors::seeds::HOSPITAL_MUSEUM_SEED).build();
        wallet
            .transparent_addresses()
            .values()
            .next()
            .expect("the synthetic wallet carries a transparent address")
            .clone()
    }

    async fn funding_bytes() -> Vec<u8> {
        faucet_funding_transaction(vec![(&external_orchard_address(), PAYMENT, None)]).await
    }

    fn lax_anchors() -> MockChain {
        let mut chain = MockChain::new();
        chain.rules.anchors = false;
        chain
    }

    fn pre_ironwood_activation_heights() -> ActivationHeights {
        ActivationHeights::builder()
            .set_overwinter(Some(1))
            .set_sapling(Some(1))
            .set_blossom(Some(1))
            .set_heartwood(Some(1))
            .set_canopy(Some(1))
            .set_nu5(Some(1))
            .set_nu6(Some(1))
            .set_nu6_1(Some(1))
            .set_nu6_2(Some(1))
            .set_nu6_3(Some(1_000))
            .build()
    }

    fn transparent_spend(
        chain: &MockChain,
        outpoint: OutPoint,
        value: Zatoshis,
        recipient: &str,
    ) -> Vec<u8> {
        let empty_script = Script::read([0u8].as_slice()).unwrap();
        let input = TxIn::from_parts(outpoint, empty_script, u32::MAX);
        let output = TxOut::new(value, taddr_script(recipient, &chain.chain_type()).unwrap());
        transparent_only_transaction(
            &chain.chain_type(),
            chain.next_height(),
            vec![input],
            vec![output],
        )
    }

    fn txid_of(chain: &MockChain, bytes: &[u8]) -> TxId {
        chain.parse_transaction(bytes, chain.next_height()).txid()
    }

    fn first_output(txid: TxId) -> OutPoint {
        OutPoint::new(*txid.as_ref(), 0)
    }

    #[test]
    fn zip317_check_rejects_fee_below_minimum_rate() {
        let size = 2_000;
        assert_eq!(
            check_zip317_mempool_policy(4, Zatoshis::const_from_u64(100), size),
            Err(Rejection::FeeBelowMinimumRate {
                fee: Zatoshis::const_from_u64(100),
                required: Zatoshis::const_from_u64(200),
            })
        );
        assert_eq!(
            check_zip317_mempool_policy(4, Zatoshis::const_from_u64(200), size),
            Ok(())
        );
    }

    #[test]
    fn zip317_check_rejects_unpaid_actions_above_block_limit() {
        assert_eq!(
            check_zip317_mempool_policy(BLOCK_UNPAID_ACTION_LIMIT + 2, MARGINAL_FEE, 100),
            Err(Rejection::UnpaidActions {
                unpaid: BLOCK_UNPAID_ACTION_LIMIT + 1,
                limit: BLOCK_UNPAID_ACTION_LIMIT
            })
        );
        assert_eq!(
            check_zip317_mempool_policy(BLOCK_UNPAID_ACTION_LIMIT + 1, MARGINAL_FEE, 100),
            Ok(())
        );
    }

    #[tokio::test]
    async fn faucet_transaction_is_rejected_for_unknown_anchor() {
        let mut chain = MockChain::new();
        let bytes = funding_bytes().await;
        let rejection = chain.submit_transaction(bytes).unwrap_err();
        assert!(
            matches!(rejection, Rejection::UnknownAnchor(_)),
            "{rejection}"
        );
        assert_eq!(chain.mempool_len(), 0);
    }

    #[tokio::test]
    async fn resubmission_with_revealed_nullifier_is_rejected() {
        let mut chain = lax_anchors();
        let bytes = funding_bytes().await;
        chain.submit_transaction(bytes.clone()).unwrap();
        let in_mempool = chain.submit_transaction(bytes.clone()).unwrap_err();
        assert!(matches!(in_mempool, Rejection::DuplicateNullifier(_)));
        chain.mine_mempool();
        let mined = chain.submit_transaction(bytes).unwrap_err();
        assert!(matches!(mined, Rejection::DuplicateNullifier(_)), "{mined}");
    }

    #[tokio::test]
    async fn reorg_removes_nullifiers_of_evicted_blocks() {
        let mut chain = lax_anchors();
        let bytes = funding_bytes().await;
        chain.mine_empty_blocks(2);
        chain.submit_transaction(bytes.clone()).unwrap();
        chain.mine_mempool();
        assert!(chain.submit_transaction(bytes.clone()).is_err());
        let evicted = chain.reorg_to(2);
        assert_eq!(evicted, vec![bytes.clone()]);
        chain.submit_transaction(bytes).unwrap();
    }

    #[tokio::test]
    async fn expired_mempool_transactions_are_evicted_when_a_block_is_mined() {
        let mut chain = lax_anchors();
        let bytes = funding_bytes().await;
        let expiry = chain.parse_transaction(&bytes, HEIGHT_ONE).expiry_height();
        chain.submit_transaction(bytes).unwrap();
        chain.mine_empty_blocks(u32::from(expiry) - 1);
        assert_eq!(chain.mempool_len(), 1);
        chain.mine_empty_blocks(1);
        assert_eq!(chain.mempool_len(), 0);
    }

    #[tokio::test]
    async fn expired_transaction_is_rejected() {
        let mut chain = lax_anchors();
        let bytes = funding_bytes().await;
        let expiry = chain.parse_transaction(&bytes, HEIGHT_ONE).expiry_height();
        assert!(expiry > NO_EXPIRY);
        chain.mine_empty_blocks(u32::from(expiry));
        let rejection = chain.submit_transaction(bytes).unwrap_err();
        assert_eq!(
            rejection,
            Rejection::Expired {
                expiry,
                next_height: expiry + 1
            }
        );
    }

    #[tokio::test]
    async fn transaction_with_wrong_branch_id_is_rejected() {
        let mut chain = MockChain::with_activation_heights(pre_ironwood_activation_heights());
        chain.rules.anchors = false;
        let bytes = funding_bytes().await;
        let rejection = chain.submit_transaction(bytes).unwrap_err();
        assert!(
            matches!(
                rejection,
                Rejection::BranchId { .. } | Rejection::Unparseable(_)
            ),
            "{rejection}"
        );
    }

    #[test]
    fn coinbase_outputs_mature_after_one_hundred_blocks() {
        let mut chain = MockChain::new();
        let miner = external_transparent_address();
        let coinbase = chain.mine_block_rewarding(&miner, REWARD, vec![]);
        let coinbase_outpoint = first_output(txid_of(&chain, &coinbase));
        assert!(
            chain
                .parse_transaction(&coinbase, HEIGHT_ONE)
                .transparent_bundle()
                .unwrap()
                .is_coinbase()
        );

        let spend = transparent_spend(&chain, coinbase_outpoint.clone(), SPEND_VALUE, &miner);
        assert_eq!(
            chain.submit_transaction(spend.clone()).unwrap_err(),
            Rejection::ImmatureCoinbase {
                mined_at: HEIGHT_ONE,
                next_height: HEIGHT_TWO
            }
        );
        chain.mine_empty_blocks(COINBASE_MATURITY_BLOCKS - 2);
        assert!(matches!(
            chain.submit_transaction(spend.clone()).unwrap_err(),
            Rejection::ImmatureCoinbase { .. }
        ));
        chain.mine_empty_blocks(1);
        chain.submit_transaction(spend.clone()).unwrap();
        assert_eq!(
            chain.submit_transaction(spend.clone()).unwrap_err(),
            Rejection::SpentInput(coinbase_outpoint.clone())
        );
        chain.mine_mempool();
        assert_eq!(
            chain.submit_transaction(spend).unwrap_err(),
            Rejection::SpentInput(coinbase_outpoint)
        );
    }

    #[test]
    fn unknown_transparent_input_is_rejected() {
        let mut chain = MockChain::new();
        let miner = external_transparent_address();
        let outpoint = OutPoint::new([7; 32], 0);
        let spend = transparent_spend(&chain, outpoint.clone(), SPEND_VALUE, &miner);
        assert_eq!(
            chain.submit_transaction(spend).unwrap_err(),
            Rejection::UnknownInput(outpoint)
        );
    }

    #[test]
    fn mempool_outputs_are_spendable_before_they_are_mined() {
        let mut chain = MockChain::new();
        let miner = external_transparent_address();
        let coinbase = chain.mine_block_rewarding(&miner, REWARD, vec![]);
        chain.mine_empty_blocks(COINBASE_MATURITY_BLOCKS);
        let first = transparent_spend(
            &chain,
            first_output(txid_of(&chain, &coinbase)),
            SPEND_VALUE,
            &miner,
        );
        let first_txid = chain.submit_transaction(first).unwrap();
        let chained_value = Zatoshis::const_from_u64(980_000);
        let second = transparent_spend(&chain, first_output(first_txid), chained_value, &miner);
        chain.submit_transaction(second.clone()).unwrap();
        assert_eq!(
            chain.submit_transaction(second).unwrap_err(),
            Rejection::SpentInput(first_output(first_txid))
        );
        assert_eq!(chain.mempool_len(), 2);
        chain.mine_mempool();
        let unspent = chain.unspent_outputs(&miner, HEIGHT_ONE).unwrap();
        assert_eq!(unspent.len(), 1);
        assert_eq!(
            unspent[0].value_zat,
            i64::from(ZatBalance::from(chained_value))
        );
    }

    #[test]
    fn outputs_above_inputs_are_rejected() {
        let mut chain = MockChain::new();
        let miner = external_transparent_address();
        let coinbase = chain.mine_block_rewarding(&miner, REWARD, vec![]);
        let coinbase_outpoint = first_output(txid_of(&chain, &coinbase));
        chain.mine_empty_blocks(COINBASE_MATURITY_BLOCKS);
        let overspend = transparent_spend(
            &chain,
            coinbase_outpoint,
            (REWARD + OVERSPEND_BY).unwrap(),
            &miner,
        );
        assert_eq!(
            chain.submit_transaction(overspend).unwrap_err(),
            Rejection::NegativeFee(OVERSPEND_BY)
        );
    }

    #[test]
    fn unspent_outputs_exclude_spent_outpoints() {
        let mut chain = MockChain::new();
        let miner = external_transparent_address();
        let coinbase = chain.mine_block_rewarding(&miner, REWARD, vec![]);
        let coinbase_txid = txid_of(&chain, &coinbase);
        let unspent = chain.unspent_outputs(&miner, HEIGHT_ONE).unwrap();
        assert_eq!(unspent.len(), 1);
        assert_eq!(unspent[0].value_zat, i64::from(ZatBalance::from(REWARD)));
        assert_eq!(unspent[0].txid, coinbase_txid.as_ref().to_vec());
        assert_eq!(unspent[0].height, 1);
        assert!(
            chain
                .unspent_outputs(&miner, HEIGHT_TWO)
                .unwrap()
                .is_empty()
        );

        chain.mine_empty_blocks(COINBASE_MATURITY_BLOCKS);
        let spend = transparent_spend(&chain, first_output(coinbase_txid), SPEND_VALUE, &miner);
        chain.submit_transaction(spend).unwrap();
        chain.mine_mempool();
        let unspent = chain.unspent_outputs(&miner, HEIGHT_ONE).unwrap();
        assert_eq!(unspent.len(), 1);
        assert_eq!(
            unspent[0].value_zat,
            i64::from(ZatBalance::from(SPEND_VALUE))
        );
        assert_eq!(unspent[0].height, u64::from(chain.tip()));
    }

    #[tokio::test]
    async fn mempool_stream_yields_new_transactions_and_closes_on_new_block() {
        let chain = Arc::new(RwLock::new(lax_anchors()));
        let service = MockIndexerService::new(chain.clone());
        let bytes = funding_bytes().await;
        let mut stream = service
            .get_mempool_stream(Request::new(Empty {}))
            .await
            .unwrap()
            .into_inner();
        chain
            .write()
            .await
            .submit_transaction(bytes.clone())
            .unwrap();
        let arrived = stream.next().await.unwrap().unwrap();
        assert_eq!(arrived.data, bytes);
        chain.write().await.mine_mempool();
        assert!(stream.next().await.is_none());

        let mut replayed = service
            .get_mempool_stream(Request::new(Empty {}))
            .await
            .unwrap()
            .into_inner();
        assert!(
            tokio::time::timeout(Duration::from_millis(50), replayed.next())
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn mempool_stream_yields_existing_mempool_on_subscription() {
        let chain = Arc::new(RwLock::new(lax_anchors()));
        let service = MockIndexerService::new(chain.clone());
        let bytes = funding_bytes().await;
        chain
            .write()
            .await
            .submit_transaction(bytes.clone())
            .unwrap();
        let mut stream = service
            .get_mempool_stream(Request::new(Empty {}))
            .await
            .unwrap()
            .into_inner();
        assert_eq!(stream.next().await.unwrap().unwrap().data, bytes);
    }

    #[tokio::test]
    async fn address_utxos_stream_applies_injected_stream_faults() {
        let chain = Arc::new(RwLock::new(MockChain::new()));
        let miner = external_transparent_address();
        {
            let mut chain = chain.write().await;
            chain.mine_block_rewarding(&miner, REWARD, vec![]);
            chain.mine_block_rewarding(&miner, REWARD, vec![]);
            chain
                .faults
                .inject(Rpc::AddressUtxos, Fault::TruncateStream { after: 1 });
        }
        let service = MockIndexerService::new(chain.clone());
        let arg = GetAddressUtxosArg {
            addresses: vec![miner],
            start_height: 1,
            max_entries: 0,
        };
        let mut stream = service
            .get_address_utxos_stream(Request::new(arg))
            .await
            .unwrap()
            .into_inner();
        assert!(stream.next().await.unwrap().is_ok());
        assert_eq!(
            stream.next().await.unwrap().unwrap_err().code(),
            Code::Unavailable
        );
        assert!(stream.next().await.is_none());
        assert_eq!(chain.read().await.faults.pending(Rpc::AddressUtxos), 0);
    }

    #[tokio::test]
    async fn injected_faults_fail_delay_and_truncate_rpcs() {
        let chain = Arc::new(RwLock::new(MockChain::new()));
        chain.write().await.mine_empty_blocks(3);
        let service = MockIndexerService::new(chain.clone());
        {
            let mut chain = chain.write().await;
            chain.faults.inject(
                Rpc::LatestBlock,
                Fault::Fail(Code::Unavailable, "mock outage".to_string()),
            );
            chain
                .faults
                .inject(Rpc::LatestBlock, Fault::Delay(Duration::from_millis(20)));
            chain
                .faults
                .inject(Rpc::BlockRange, Fault::TruncateStream { after: 1 });
            chain
                .faults
                .inject(Rpc::BlockRange, Fault::EndStream { after: 2 });
        }

        let outage = service
            .get_latest_block(Request::new(ChainSpec {}))
            .await
            .unwrap_err();
        assert_eq!(outage.code(), Code::Unavailable);
        let started = std::time::Instant::now();
        let tip = service
            .get_latest_block(Request::new(ChainSpec {}))
            .await
            .unwrap()
            .into_inner();
        assert!(started.elapsed() >= Duration::from_millis(20));
        assert_eq!(tip.height, 3);
        assert_eq!(chain.read().await.faults.pending(Rpc::LatestBlock), 0);

        let range = BlockRange {
            start: Some(BlockId {
                height: 1,
                hash: vec![],
            }),
            end: Some(BlockId {
                height: 3,
                hash: vec![],
            }),
            pool_types: vec![],
        };
        let mut truncated = service
            .get_block_range(Request::new(range.clone()))
            .await
            .unwrap()
            .into_inner();
        assert_eq!(truncated.next().await.unwrap().unwrap().height, 1);
        let eof = truncated.next().await.unwrap().unwrap_err();
        assert!(eof.message().contains(STREAM_EOF_MESSAGE));
        assert!(truncated.next().await.is_none());

        let mut ended = service
            .get_block_range(Request::new(range.clone()))
            .await
            .unwrap()
            .into_inner();
        assert_eq!(ended.next().await.unwrap().unwrap().height, 1);
        assert_eq!(ended.next().await.unwrap().unwrap().height, 2);
        assert!(ended.next().await.is_none());

        let mut whole = service
            .get_block_range(Request::new(range))
            .await
            .unwrap()
            .into_inner();
        let mut served = 0;
        while let Some(block) = whole.next().await {
            block.unwrap();
            served += 1;
        }
        assert_eq!(served, 3);
    }

    #[tokio::test]
    async fn send_transaction_returns_rejection_in_zaino_error_format() {
        let chain = Arc::new(RwLock::new(MockChain::new()));
        let service = MockIndexerService::new(chain.clone());
        let bytes = funding_bytes().await;
        let status = service
            .send_transaction(Request::new(RawTransaction {
                data: bytes,
                height: 0,
            }))
            .await
            .unwrap_err();
        assert_eq!(status.code(), Code::Internal);
        assert!(status.message().starts_with(ZAINO_SEND_ERROR_PREFIX));
        assert!(status.message().contains("anchor"));
        assert_eq!(
            crate::lightclient::transmit::classify_rejection(status.message()),
            crate::lightclient::transmit::RejectionClass::Transient
        );
        assert_eq!(chain.read().await.mempool_len(), 0);
    }

    #[tokio::test]
    async fn lightd_info_returns_sapling_activation_height_and_branch_id() {
        let chain = Arc::new(RwLock::new(MockChain::with_activation_heights(
            pre_ironwood_activation_heights(),
        )));
        let service = MockIndexerService::new(chain.clone());
        let info = service
            .get_lightd_info(Request::new(Empty {}))
            .await
            .unwrap()
            .into_inner();
        assert_eq!(info.sapling_activation_height, 1);
        let expected: u32 = BranchId::Nu6_2.into();
        assert_eq!(info.consensus_branch_id, format!("{expected:08x}"));
    }
}
