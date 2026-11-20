use zcash_primitives::block::BlockHash;
use zcash_protocol::TxId;
use zcash_protocol::consensus::BlockHeight;
use zingo_common_components::protocol::ActivationHeights;

/// The network types a lightclient can connect to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChainType {
    /// Mainnet
    Mainnet,
    /// Testnet
    Testnet,
    /// Regtest
    Regtest(ActivationHeights),
}

impl std::fmt::Display for ChainType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let chain = match self {
            ChainType::Mainnet => "mainnet",
            ChainType::Testnet => "testnet",
            ChainType::Regtest(_) => "regtest",
        };
        write!(f, "{chain}")
    }
}

impl TryFrom<&str> for ChainType {
    type Error = InvalidChainType;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        match value {
            "mainnet" | "main" => Ok(ChainType::Mainnet),
            "testnet" | "test" => Ok(ChainType::Testnet),
            "regtest" => Ok(ChainType::Regtest(ActivationHeights::default())),
            _ => Err(InvalidChainType(value.to_string())),
        }
    }
}

pub(crate) mod consealed {
    use zcash_protocol::consensus::{
        BlockHeight, MAIN_NETWORK, NetworkType, NetworkUpgrade, Parameters, TEST_NETWORK,
    };

    use super::ChainType;

    impl Parameters for ChainType {
        fn network_type(&self) -> NetworkType {
            match self {
                ChainType::Mainnet => NetworkType::Main,
                ChainType::Testnet => NetworkType::Test,
                ChainType::Regtest(_) => NetworkType::Regtest,
            }
        }

        fn activation_height(&self, nu: NetworkUpgrade) -> Option<BlockHeight> {
            match self {
                ChainType::Mainnet => MAIN_NETWORK.activation_height(nu),
                ChainType::Testnet => TEST_NETWORK.activation_height(nu),
                ChainType::Regtest(activation_heights) => match nu {
                    NetworkUpgrade::Overwinter => {
                        activation_heights.overwinter().map(BlockHeight::from_u32)
                    }
                    NetworkUpgrade::Sapling => {
                        activation_heights.sapling().map(BlockHeight::from_u32)
                    }
                    NetworkUpgrade::Blossom => {
                        activation_heights.blossom().map(BlockHeight::from_u32)
                    }
                    NetworkUpgrade::Heartwood => {
                        activation_heights.heartwood().map(BlockHeight::from_u32)
                    }
                    NetworkUpgrade::Canopy => {
                        activation_heights.canopy().map(BlockHeight::from_u32)
                    }
                    NetworkUpgrade::Nu5 => activation_heights.nu5().map(BlockHeight::from_u32),
                    NetworkUpgrade::Nu6 => activation_heights.nu6().map(BlockHeight::from_u32),
                    NetworkUpgrade::Nu6_1 => activation_heights.nu6_1().map(BlockHeight::from_u32),
                    NetworkUpgrade::Nu6_2 => activation_heights.nu6_2().map(BlockHeight::from_u32),
                    NetworkUpgrade::Nu6_3 => activation_heights.nu6_3().map(BlockHeight::from_u32),
                    NetworkUpgrade::Nu7 => activation_heights.nu7().map(BlockHeight::from_u32),
                },
            }
        }
    }
}

/// Invalid chain type.
#[derive(thiserror::Error, Debug)]
#[error("Invalid chain type '{0}'. Expected one of: 'mainnet', 'testnet' or 'regtest'.")]
pub struct InvalidChainType(String);

/// A note commitment tree state as served by an indexer for a block: each shielded pool's
/// serialized commitment tree, already hex-decoded, with a validated height and hash.
///
/// Carrying wire bytes unvalidated this far would mean every consumer re-derives validity
/// (decodable hex? right-sized hash? a height that fits?) on every touch; this type validates
/// once, at the boundary, so a `TreeState` is trusted structurally wherever it is passed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreeState {
    /// The height of the block this tree state is as of.
    pub height: BlockHeight,
    /// The hash of the block this tree state is as of.
    pub hash: BlockHash,
    /// The Sapling note commitment tree, hex-decoded. Empty below Sapling activation, or when
    /// the server omits it.
    pub sapling_tree: Vec<u8>,
    /// The Orchard note commitment tree, hex-decoded. Empty below Orchard activation, or when
    /// the server omits it.
    pub orchard_tree: Vec<u8>,
    /// The Ironwood note commitment tree, hex-decoded. Empty below Ironwood activation, or when
    /// the server omits it.
    pub ironwood_tree: Vec<u8>,
}

/// A [`TreeState`] field that failed to parse from its wire representation.
#[derive(Debug, thiserror::Error)]
pub enum InvalidTreeState {
    /// The block height does not fit a [`BlockHeight`].
    #[error("invalid block height")]
    InvalidHeight,
    /// The block hash is not valid hex.
    #[error("block hash is not valid hex: {0}")]
    InvalidHashHex(hex::FromHexError),
    /// The block hash did not decode to exactly 32 bytes.
    #[error("invalid block hash length")]
    InvalidHashLength,
    /// A shielded pool's tree is not valid hex.
    #[error("{field} tree is not valid hex: {source}")]
    InvalidTreeHex {
        /// The pool whose tree failed to decode.
        field: &'static str,
        /// The underlying hex decode error.
        source: hex::FromHexError,
    },
}

impl TreeState {
    /// Parses a tree state from its wire parts: a block height, a byte-reversed hex block
    /// hash, and three hex-encoded (possibly empty) serialized commitment trees.
    ///
    /// Rejects a height that does not fit a [`BlockHeight`], hex that fails to decode, and a
    /// hash that does not decode to exactly 32 bytes.
    pub fn from_wire_parts(
        height: u64,
        hash_hex: &str,
        sapling_tree_hex: &str,
        orchard_tree_hex: &str,
        ironwood_tree_hex: &str,
    ) -> Result<Self, InvalidTreeState> {
        let height = BlockHeight::try_from(height).map_err(|_| InvalidTreeState::InvalidHeight)?;

        let mut hash_bytes = hex::decode(hash_hex).map_err(InvalidTreeState::InvalidHashHex)?;
        // Block-hash hex strings are byte-reversed on the wire.
        hash_bytes.reverse();
        let hash =
            BlockHash::try_from_slice(&hash_bytes).ok_or(InvalidTreeState::InvalidHashLength)?;

        let decode_tree = |field: &'static str, hex_str: &str| -> Result<Vec<u8>, InvalidTreeState> {
            hex::decode(hex_str).map_err(|source| InvalidTreeState::InvalidTreeHex { field, source })
        };

        Ok(Self {
            height,
            hash,
            sapling_tree: decode_tree("sapling", sapling_tree_hex)?,
            orchard_tree: decode_tree("orchard", orchard_tree_hex)?,
            ironwood_tree: decode_tree("ironwood", ironwood_tree_hex)?,
        })
    }
}

/// A completed shard's Merkle root, as served by an indexer: the raw 32-byte root and the
/// block that completed it, already validated rather than carried as unchecked wire bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubtreeRoot {
    /// The 32-byte Merkle root of the subtree. Not yet parsed into a pool-specific hash type,
    /// since that decoding depends on which shielded pool the caller requested roots for, which
    /// this wire message does not itself carry.
    pub root_hash: [u8; 32],
    /// The hash of the block that completed this subtree.
    pub completing_block_hash: BlockHash,
    /// The height of the block that completed this subtree in the main chain.
    pub completing_block_height: BlockHeight,
}

/// A [`SubtreeRoot`] field that failed to parse from its wire representation.
#[derive(Debug, thiserror::Error)]
pub enum InvalidSubtreeRoot {
    /// The root hash was not exactly 32 bytes.
    #[error("subtree root hash is not 32 bytes")]
    InvalidRootHashLength,
    /// The completing block hash was not exactly 32 bytes.
    #[error("completing block hash is not 32 bytes")]
    InvalidCompletingBlockHashLength,
    /// The completing block height does not fit a [`BlockHeight`].
    #[error("completing block height does not fit a block height")]
    InvalidCompletingBlockHeight,
}

impl SubtreeRoot {
    /// Parses a subtree root from its wire parts, rejecting a root hash or completing block
    /// hash that is not exactly 32 bytes, and a completing block height that does not fit a
    /// [`BlockHeight`].
    pub fn from_wire_parts(
        root_hash: Vec<u8>,
        completing_block_hash: Vec<u8>,
        completing_block_height: u64,
    ) -> Result<Self, InvalidSubtreeRoot> {
        let root_hash: [u8; 32] = root_hash
            .try_into()
            .map_err(|_| InvalidSubtreeRoot::InvalidRootHashLength)?;
        let completing_block_hash = BlockHash::try_from_slice(&completing_block_hash)
            .ok_or(InvalidSubtreeRoot::InvalidCompletingBlockHashLength)?;
        let completing_block_height = BlockHeight::try_from(completing_block_height)
            .map_err(|_| InvalidSubtreeRoot::InvalidCompletingBlockHeight)?;
        Ok(Self {
            root_hash,
            completing_block_hash,
            completing_block_height,
        })
    }
}

/// A [`CompactBlock`] (or one of its nested parts) that failed to parse from its wire
/// representation: a byte field of the wrong length, or a height that does not fit a
/// [`BlockHeight`].
#[derive(Debug, thiserror::Error)]
pub enum InvalidCompactBlock {
    /// The block height does not fit a [`BlockHeight`].
    #[error("invalid block height")]
    InvalidHeight,
    /// A fixed-length field was not the expected number of bytes.
    #[error("{field} is not {expected} bytes (found {found})")]
    InvalidFieldLength {
        /// The dotted struct.field name of the offending field, e.g. `"CompactBlock.hash"`.
        field: &'static str,
        /// The length the field must have.
        expected: usize,
        /// The length actually found.
        found: usize,
    },
}

fn fixed_bytes<const N: usize>(
    field: &'static str,
    bytes: Vec<u8>,
) -> Result<[u8; N], InvalidCompactBlock> {
    let found = bytes.len();
    bytes.try_into().map_err(|_| InvalidCompactBlock::InvalidFieldLength {
        field,
        expected: N,
        found,
    })
}

/// Information about the state of the chain as of the end of a block: each shielded pool's
/// note commitment tree size.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChainMetadata {
    /// The size of the Sapling note commitment tree as of the end of this block.
    pub sapling_commitment_tree_size: u32,
    /// The size of the Orchard note commitment tree as of the end of this block.
    pub orchard_commitment_tree_size: u32,
    /// The size of the Ironwood note commitment tree as of the end of this block.
    pub ironwood_commitment_tree_size: u32,
}

/// A Sapling Spend Description's nullifier, as carried in a compact block.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CompactSaplingSpend {
    /// The nullifier of the note being spent.
    pub nullifier: [u8; 32],
}

impl CompactSaplingSpend {
    /// Parses a compact Sapling spend from its wire nullifier bytes.
    pub fn from_wire(nullifier: Vec<u8>) -> Result<Self, InvalidCompactBlock> {
        Ok(Self {
            nullifier: fixed_bytes("CompactSaplingSpend.nf", nullifier)?,
        })
    }
}

/// A Sapling Output Description's compact fields: the note commitment, the ephemeral public
/// key, and the first 52 bytes of the encrypted ciphertext.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompactSaplingOutput {
    /// The note commitment u-coordinate. Not fixed-length-validated here: the deprecated
    /// nullifiers-only block fetch legitimately omits output data, so a server may send this
    /// empty. [`crate::chain`]'s consumer validates the length only when it actually needs to
    /// decrypt the output.
    pub cmu: Vec<u8>,
    /// The ephemeral public key. See the note on [`Self::cmu`].
    pub ephemeral_key: Vec<u8>,
    /// The first 52 bytes of the encrypted ciphertext. See the note on [`Self::cmu`].
    pub ciphertext: Vec<u8>,
}

/// An Orchard (or Ironwood) Action's compact fields.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompactOrchardAction {
    /// The nullifier of the input note. Always populated: this is the one field the
    /// deprecated nullifiers-only block fetch exists to serve.
    pub nullifier: [u8; 32],
    /// The x-coordinate of the note commitment for the output note. Not fixed-length-validated
    /// here: an Orchard action carries both spend and output data in one message, so the
    /// nullifiers-only fetch serves this (and the two fields below) empty rather than omitting
    /// the action outright. The consumer validates the length only when it actually needs to
    /// decrypt the output.
    pub cmx: Vec<u8>,
    /// An encoding of an ephemeral Pallas public key. See the note on [`Self::cmx`].
    pub ephemeral_key: Vec<u8>,
    /// The first 52 bytes of the encrypted ciphertext. See the note on [`Self::cmx`].
    pub ciphertext: Vec<u8>,
}

impl CompactOrchardAction {
    /// Parses a compact Orchard (or Ironwood) action from its wire fields, validating only the
    /// nullifier: see the field docs for why the rest are not validated here.
    pub fn from_wire(
        nullifier: Vec<u8>,
        cmx: Vec<u8>,
        ephemeral_key: Vec<u8>,
        ciphertext: Vec<u8>,
    ) -> Result<Self, InvalidCompactBlock> {
        Ok(Self {
            nullifier: fixed_bytes("CompactOrchardAction.nullifier", nullifier)?,
            cmx,
            ephemeral_key,
            ciphertext,
        })
    }
}

/// A transparent input, naming the output it spends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CompactTxIn {
    /// The txid of the transaction that created the output being spent.
    pub prevout_txid: TxId,
    /// The index of the output being spent in that transaction's `vout`.
    pub prevout_index: u32,
}

impl CompactTxIn {
    /// Parses a compact transparent input from its wire fields.
    pub fn from_wire(
        prevout_txid: Vec<u8>,
        prevout_index: u32,
    ) -> Result<Self, InvalidCompactBlock> {
        Ok(Self {
            prevout_txid: TxId::from_bytes(fixed_bytes("CompactTxIn.prevout_txid", prevout_txid)?),
            prevout_index,
        })
    }
}

/// A transparent output being created by the transaction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TxOut {
    /// The value of the output, in zatoshis.
    pub value: u64,
    /// The script pubkey that must be satisfied in order to spend this output.
    pub script_pub_key: Vec<u8>,
}

/// A compact transaction: the minimum shielded and transparent data a wallet needs to detect
/// payments to it and spends from it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompactTx {
    /// The index of the transaction within the block.
    pub index: u64,
    /// The transaction's id, in protocol (not byte-reversed) order.
    pub txid: TxId,
    /// The transaction fee, when the server can provide it; zero otherwise.
    pub fee: u32,
    /// The Sapling spends.
    pub spends: Vec<CompactSaplingSpend>,
    /// The Sapling outputs.
    pub outputs: Vec<CompactSaplingOutput>,
    /// The Orchard actions.
    pub actions: Vec<CompactOrchardAction>,
    /// The Ironwood actions.
    pub ironwood_actions: Vec<CompactOrchardAction>,
    /// The transparent inputs.
    pub vin: Vec<CompactTxIn>,
    /// The transparent outputs.
    pub vout: Vec<TxOut>,
}

impl CompactTx {
    /// Parses a compact transaction's own scalar fields, taking its nested components
    /// (spends, outputs, actions, transparent inputs and outputs) already parsed.
    pub fn from_wire_parts(
        index: u64,
        txid: Vec<u8>,
        fee: u32,
        spends: Vec<CompactSaplingSpend>,
        outputs: Vec<CompactSaplingOutput>,
        actions: Vec<CompactOrchardAction>,
        ironwood_actions: Vec<CompactOrchardAction>,
        vin: Vec<CompactTxIn>,
        vout: Vec<TxOut>,
    ) -> Result<Self, InvalidCompactBlock> {
        Ok(Self {
            index,
            txid: TxId::from_bytes(fixed_bytes("CompactTx.txid", txid)?),
            fee,
            spends,
            outputs,
            actions,
            ironwood_actions,
            vin,
            vout,
        })
    }
}

/// A compact block: the minimum data a wallet needs to detect payments, detect spends, update
/// witnesses, and spend transparent UTXOs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompactBlock {
    /// The height of this block.
    pub height: BlockHeight,
    /// The hash of this block.
    pub hash: BlockHash,
    /// The hash of this block's predecessor.
    pub prev_hash: BlockHash,
    /// The Unix epoch time the block was mined, or zero if the server does not report it.
    pub time: u32,
    /// The full serialized block header, as returned by the `getblock` RPC, or empty if the
    /// server does not report it.
    pub header: Vec<u8>,
    /// The block's compact transactions.
    pub vtx: Vec<CompactTx>,
    /// The state of the chain as of the end of this block, when the server reports it.
    pub chain_metadata: Option<ChainMetadata>,
}

impl CompactBlock {
    /// Parses a compact block's own scalar fields, taking its transactions already parsed.
    pub fn from_wire_parts(
        height: u64,
        hash: Vec<u8>,
        prev_hash: Vec<u8>,
        time: u32,
        header: Vec<u8>,
        vtx: Vec<CompactTx>,
        chain_metadata: Option<ChainMetadata>,
    ) -> Result<Self, InvalidCompactBlock> {
        Ok(Self {
            height: BlockHeight::try_from(height).map_err(|_| InvalidCompactBlock::InvalidHeight)?,
            hash: BlockHash::try_from_slice(&hash).ok_or(InvalidCompactBlock::InvalidFieldLength {
                field: "CompactBlock.hash",
                expected: 32,
                found: hash.len(),
            })?,
            prev_hash: BlockHash::try_from_slice(&prev_hash).ok_or(
                InvalidCompactBlock::InvalidFieldLength {
                    field: "CompactBlock.prev_hash",
                    expected: 32,
                    found: prev_hash.len(),
                },
            )?,
            time,
            header,
            vtx,
            chain_metadata,
        })
    }
}

/// Where a transaction stands relative to the main chain, as reported by a [`RawTransaction`]'s
/// wire height field. The wire field overloads one `u64` with three meanings (mempool, mined on
/// a fork, mined at a height); this type makes each one explicit instead of leaving every
/// consumer to re-derive the sentinel values.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RawTransactionStatus {
    /// The transaction is in the mempool, not yet mined.
    Mempool,
    /// The transaction was mined, but on a fork that is not the main chain.
    MinedOffMainChain,
    /// The transaction was mined on the main chain at this height.
    Mined(BlockHeight),
}

/// A [`RawTransaction`] that failed to parse from its wire representation: a mined height that
/// is neither of the wire format's two sentinel values, nor fits a [`BlockHeight`].
#[derive(Debug, thiserror::Error)]
#[error("mined height does not fit a block height")]
pub struct InvalidRawTransaction;

/// A full serialized transaction, as served by an indexer, with its mined status already
/// parsed out of the wire format's overloaded height field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawTransaction {
    /// The serialized representation of the transaction.
    pub data: Vec<u8>,
    /// Where this transaction stands relative to the main chain.
    pub status: RawTransactionStatus,
}

impl RawTransaction {
    /// Parses a raw transaction from its wire parts, decoding the wire height's sentinel
    /// values (`0` for mempool, `u64::MAX` for mined-off-main-chain) into [`RawTransactionStatus`].
    pub fn from_wire_parts(data: Vec<u8>, height: u64) -> Result<Self, InvalidRawTransaction> {
        let status = match height {
            0 => RawTransactionStatus::Mempool,
            u64::MAX => RawTransactionStatus::MinedOffMainChain,
            height => RawTransactionStatus::Mined(
                BlockHeight::try_from(height).map_err(|_| InvalidRawTransaction)?,
            ),
        };
        Ok(Self { data, status })
    }
}

/// A [`GetAddressUtxosReply`] that failed to parse from its wire representation.
#[derive(Debug, thiserror::Error)]
pub enum InvalidGetAddressUtxosReply {
    /// The txid was not exactly 32 bytes.
    #[error("txid is not 32 bytes (found {0})")]
    InvalidTxidLength(usize),
    /// The height does not fit a [`BlockHeight`].
    #[error("invalid height")]
    InvalidHeight,
}

/// An unspent transparent output, as served by an indexer's address-UTXO lookup.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GetAddressUtxosReply {
    /// The transparent address this output pays.
    pub address: String,
    /// The txid of the transaction that created this output.
    pub txid: TxId,
    /// The index of this output within that transaction.
    pub index: i32,
    /// The script pubkey that must be satisfied in order to spend this output.
    pub script: Vec<u8>,
    /// The value of the output, in zatoshis.
    pub value: i64,
    /// The height at which the output was mined.
    pub height: BlockHeight,
}

impl GetAddressUtxosReply {
    /// Parses an address-UTXO reply from its wire parts, rejecting a txid that is not exactly
    /// 32 bytes and a height that does not fit a [`BlockHeight`].
    pub fn from_wire_parts(
        address: String,
        txid: Vec<u8>,
        index: i32,
        script: Vec<u8>,
        value: i64,
        height: u64,
    ) -> Result<Self, InvalidGetAddressUtxosReply> {
        let found = txid.len();
        Ok(Self {
            address,
            txid: TxId::from_bytes(
                txid.try_into()
                    .map_err(|_| InvalidGetAddressUtxosReply::InvalidTxidLength(found))?,
            ),
            index,
            script,
            value,
            height: BlockHeight::try_from(height)
                .map_err(|_| InvalidGetAddressUtxosReply::InvalidHeight)?,
        })
    }
}

// The request types below are built by the caller, not received from a server, so there is
// nothing in them to validate: a wire-format conversion is always valid by construction. They
// exist so pepper-sync's own code never names a wire type, not to guard against malformed
// input the way the types above do for data a server sends.

/// Identifies a block to a server, by height and, optionally, hash.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BlockId {
    /// The block's height.
    pub height: BlockHeight,
    /// The block's hash, when the caller wants to identify the block by hash instead of (or as
    /// well as) height.
    pub hash: Option<BlockHash>,
}

/// A [`BlockId`] received from a server that failed to parse: its height does not fit a
/// [`BlockHeight`].
#[derive(Debug, thiserror::Error)]
#[error("invalid block height")]
pub struct InvalidBlockId;

impl BlockId {
    /// A [`BlockId`] that identifies a block by height alone.
    pub fn from_height(height: BlockHeight) -> Self {
        Self { height, hash: None }
    }

    /// Parses a block id received from a server. A hash that is not exactly 32 bytes is
    /// treated as absent rather than rejected, since `hash` is an optional secondary
    /// identifier here, not the field this value is primarily validated on.
    pub fn from_wire_parts(height: u64, hash: Vec<u8>) -> Result<Self, InvalidBlockId> {
        Ok(Self {
            height: BlockHeight::try_from(height).map_err(|_| InvalidBlockId)?,
            hash: BlockHash::try_from_slice(&hash),
        })
    }
}

/// A range of blocks to request from a server, with ascending order for `start <= end` and
/// descending order for `start > end`, optionally scoped to specific shielded pools.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockRange {
    /// The first block of the range.
    pub start: BlockId,
    /// The last block of the range.
    pub end: BlockId,
    /// The pools to include compact data for. Empty means every pool.
    pub pool_types: Vec<zcash_protocol::PoolType>,
}

/// The wire format's `PoolType` enum encoding for `pool`.
pub fn pool_type_wire_value(pool: zcash_protocol::PoolType) -> i32 {
    use zcash_protocol::{PoolType, ShieldedPool};
    match pool {
        PoolType::Transparent => 1,
        PoolType::Shielded(ShieldedPool::Sapling) => 2,
        PoolType::Shielded(ShieldedPool::Orchard) => 3,
        PoolType::Shielded(ShieldedPool::Ironwood) => 4,
    }
}

/// Identifies a transaction to fetch from a server, by its position in a block or by txid.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TxFilter {
    /// The block the transaction is in, when identifying it by position.
    pub block: Option<BlockId>,
    /// The transaction's index within that block.
    pub index: u64,
    /// The transaction's id, when identifying it by txid.
    pub hash: Option<TxId>,
}

/// Requests unspent transparent outputs for a set of addresses, from a starting height.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GetAddressUtxosArg {
    /// The transparent addresses to look up.
    pub addresses: Vec<String>,
    /// The height to start returning UTXOs from.
    pub start_height: BlockHeight,
    /// The maximum number of entries to return, or zero for unlimited.
    pub max_entries: u32,
}

/// Requests a shielded pool's subtree roots, from a starting index.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GetSubtreeRootsArg {
    /// The index to start returning subtree roots from.
    pub start_index: u32,
    /// The shielded pool to return subtree roots for, as the wire format's shielded-protocol
    /// encoding.
    pub shielded_protocol: i32,
    /// The maximum number of entries to return, or zero for unlimited.
    pub max_entries: u32,
}

/// Requests a transparent address's transactions in a block range.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransparentAddressBlockFilter {
    /// The transparent address to look up.
    pub address: String,
    /// The block range to search, or `None` for the whole chain.
    pub range: Option<BlockRange>,
}

/// An indexer's self-reported identity and chain diagnostics, as returned by `GetLightdInfo`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LightdInfo {
    /// The server's software version.
    pub version: String,
    /// The server's vendor string.
    pub vendor: String,
    /// Whether the server supports transparent addresses.
    pub taddr_support: bool,
    /// The chain name ("main", "test", "regtest").
    pub chain_name: String,
    /// The Sapling activation height of the chain.
    pub sapling_activation_height: u64,
    /// The consensus branch ID the server reports.
    pub consensus_branch_id: String,
    /// The server's view of the chain tip height.
    pub block_height: u64,
    /// The git commit the server was built from.
    pub git_commit: String,
    /// The version of the lightwallet protocol served by this server.
    pub lightwallet_protocol_version: String,
}

/// A backend's ZIP 317 marginal fee report, as returned by `GetStandardFee`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StandardFee {
    /// The standard fee in zatoshis per logical action.
    pub fee_zat: u64,
    /// The backend's fee-estimator version, where 0 is the static ZIP 317 marginal fee.
    pub estimator_version: u32,
}

#[cfg(test)]
mod tests {
    use super::ChainType;
    use zingo_common_components::protocol::ActivationHeights;

    #[test]
    fn test_chaintypes_from_strings() {
        assert_eq!(
            ChainType::try_from("mainnet").expect("'mainnet' names the main network"),
            ChainType::Mainnet
        );
        assert_eq!(
            ChainType::try_from("testnet").expect("'testnet' names the test network"),
            ChainType::Testnet
        );
        // the following 2 variants match the string from get_lightd_info, from the LightdInfo struct
        // as referenced in librustzcash/zcash_client_backend/src/proto/service.rs 17a4830c70
        // and provided by an indexer (previously called lightwallet)
        assert_eq!(
            ChainType::try_from("main").expect("'main' names the main network"),
            ChainType::Mainnet
        );
        assert_eq!(
            ChainType::try_from("test").expect("'test' names the test network"),
            ChainType::Testnet
        );
        assert_eq!(
            ChainType::try_from("regtest").expect("'regtest' names the regtest network"),
            ChainType::Regtest(ActivationHeights::default())
        );
        assert!(ChainType::try_from("invalid").is_err());
        let err = ChainType::try_from("badvalue").expect_err("'badvalue' names no network");
        assert_eq!(err.0, "badvalue");
        assert!(ChainType::try_from("").is_err());
        assert!(ChainType::try_from("Mainnet").is_err());
        assert!(ChainType::try_from("MAINNET").is_err());
        assert!(ChainType::try_from("Testnet").is_err());
        assert!(ChainType::try_from("Regtest").is_err());
        assert!(ChainType::try_from("mainnet ").is_err()); // trailing space
        assert!(ChainType::try_from(" mainnet").is_err()); // leading space
        assert!(ChainType::try_from("regtест").is_err()); // lookalike chars
    }
}
