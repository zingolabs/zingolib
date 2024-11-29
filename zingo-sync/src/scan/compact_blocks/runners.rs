//! Temporary copy of LRZ batch runners while we wait for their exposition and update LRZ

use std::fmt;
use std::mem;
use std::sync::atomic::AtomicUsize;
use std::{collections::HashMap, hash::Hash};

use crossbeam_channel as channel;

use orchard::note_encryption::CompactAction;
use orchard::note_encryption::OrchardDomain;
use sapling_crypto::note_encryption::CompactOutputDescription;
use sapling_crypto::note_encryption::SaplingDomain;

use zcash_client_backend::proto::compact_formats::CompactBlock;
use zcash_client_backend::scanning::ScanError;
use zcash_client_backend::ShieldedProtocol;
use zcash_note_encryption::{batch, BatchDomain, Domain, ShieldedOutput, COMPACT_NOTE_SIZE};
use zcash_primitives::consensus;
use zcash_primitives::transaction::components::sapling::zip212_enforcement;
use zcash_primitives::{block::BlockHash, transaction::TxId};

use memuse::DynamicUsage;

use crate::keys::KeyId;
use crate::keys::ScanningKeyOps as _;
use crate::keys::ScanningKeys;
use crate::primitives::OutputId;

type TaggedSaplingBatch = TrialDecryptBatch<
    SaplingDomain,
    sapling_crypto::note_encryption::CompactOutputDescription,
    CompactDecryptor,
>;
type TaggedSaplingBatchRunner<Tasks> = BatchRunner<
    SaplingDomain,
    TrialDecryptBatch<
        SaplingDomain,
        sapling_crypto::note_encryption::CompactOutputDescription,
        CompactDecryptor,
    >,
    Tasks,
>;

type TaggedOrchardBatch =
    TrialDecryptBatch<OrchardDomain, orchard::note_encryption::CompactAction, CompactDecryptor>;
type TaggedOrchardBatchRunner<Tasks> = BatchRunner<
    OrchardDomain,
    TrialDecryptBatch<OrchardDomain, orchard::note_encryption::CompactAction, CompactDecryptor>,
    Tasks,
>;

pub(crate) trait SaplingTasks: Tasks<TaggedSaplingBatch> {}
impl<T: Tasks<TaggedSaplingBatch>> SaplingTasks for T {}

pub(crate) trait OrchardTasks: Tasks<TaggedOrchardBatch> {}
impl<T: Tasks<TaggedOrchardBatch>> OrchardTasks for T {}

pub(crate) struct BatchRunners<TS: SaplingTasks, TO: OrchardTasks> {
    pub(crate) sapling: TaggedSaplingBatchRunner<TS>,
    pub(crate) orchard: TaggedOrchardBatchRunner<TO>,
}

impl<TS, TO> BatchRunners<TS, TO>
where
    TS: SaplingTasks,
    TO: OrchardTasks,
{
    pub(crate) fn for_keys(batch_size_threshold: usize, scanning_keys: &ScanningKeys) -> Self {
        BatchRunners {
            sapling: BatchRunner::new(
                batch_size_threshold,
                scanning_keys
                    .sapling()
                    .iter()
                    .map(|(id, key)| (*id, key.prepare()))
                    .unzip(),
            ),
            orchard: BatchRunner::new(
                batch_size_threshold,
                scanning_keys
                    .orchard()
                    .iter()
                    .map(|(id, key)| (*id, key.prepare()))
                    .unzip(),
            ),
        }
    }

    pub(crate) fn flush(&mut self) {
        self.sapling.flush();
        self.orchard.flush();
    }

    #[tracing::instrument(skip_all, fields(height = block.height))]
    pub(crate) fn add_block<P>(&mut self, params: &P, block: CompactBlock) -> Result<(), ScanError>
    where
        P: consensus::Parameters + Send + 'static,
    {
        let block_hash = block.hash();
        let block_height = block.height();
        let zip212_enforcement = zip212_enforcement(params, block_height);

        for tx in block.vtx.into_iter() {
            let txid = tx.txid();

            self.sapling.add_widgets(
                ResultKey(block_hash, txid),
                tx.outputs
                    .iter()
                    .enumerate()
                    .map(|(i, output)| {
                        CompactOutputDescription::try_from(output)
                            .map_err(|_| ScanError::EncodingInvalid {
                                at_height: block_height,
                                txid,
                                pool_type: ShieldedProtocol::Sapling,
                                index: i,
                            })
                            .map(|output| (SaplingDomain::new(zip212_enforcement), output))
                    })
                    .collect::<Result<Vec<_>, _>>()?
                    .into_iter(),
            );

            self.orchard.add_widgets(
                ResultKey(block_hash, txid),
                tx.actions
                    .iter()
                    .enumerate()
                    .map(|(i, action)| {
                        CompactAction::try_from(action)
                            .map_err(|_| ScanError::EncodingInvalid {
                                at_height: block_height,
                                txid,
                                pool_type: ShieldedProtocol::Orchard,
                                index: i,
                            })
                            .map(|action| (OrchardDomain::for_compact_action(&action), action))
                    })
                    .collect::<Result<Vec<_>, _>>()?
                    .into_iter(),
            );
        }

        Ok(())
    }
}

/// A decrypted transaction output.
pub(crate) struct DecryptedOutput<D: Domain, M> {
    /// The tag corresponding to the incoming viewing key used to decrypt the note.
    pub(crate) ivk_tag: KeyId,
    /// The recipient of the note.
    pub(crate) recipient: D::Recipient,
    /// The note!
    pub(crate) note: D::Note,
    /// The memo field, or `()` if this is a decrypted compact output.
    pub(crate) memo: M,
}

impl<D: Domain, M> fmt::Debug for DecryptedOutput<D, M>
where
    D::IncomingViewingKey: fmt::Debug,
    D::Recipient: fmt::Debug,
    D::Note: fmt::Debug,
    M: fmt::Debug,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DecryptedOutput")
            .field("ivk_tag", &self.ivk_tag)
            .field("recipient", &self.recipient)
            .field("note", &self.note)
            .field("memo", &self.memo)
            .finish()
    }
}

/// A decryptor of transaction outputs.
pub(crate) trait Decryptor<D: BatchDomain, Output> {
    type Memo;

    // Once we reach MSRV 1.75.0, this can return `impl Iterator`.
    fn batch_decrypt(
        tags: &[KeyId],
        ivks: &[D::IncomingViewingKey],
        outputs: &[(D, Output)],
    ) -> Vec<Option<DecryptedOutput<D, Self::Memo>>>;
}

/// A decryptor of outputs as encoded in compact blocks.
pub(crate) struct CompactDecryptor;

impl<D: BatchDomain, Output: ShieldedOutput<D, COMPACT_NOTE_SIZE>> Decryptor<D, Output>
    for CompactDecryptor
{
    type Memo = ();

    fn batch_decrypt(
        tags: &[KeyId],
        ivks: &[D::IncomingViewingKey],
        outputs: &[(D, Output)],
    ) -> Vec<Option<DecryptedOutput<D, Self::Memo>>> {
        batch::try_compact_note_decryption(ivks, outputs)
            .into_iter()
            .map(|res| {
                res.map(|((note, recipient), ivk_idx)| DecryptedOutput {
                    ivk_tag: tags[ivk_idx],
                    recipient,
                    note,
                    memo: (),
                })
            })
            .collect()
    }
}

/// The receiver for the result of batch scanning.
/// This wrapper type is only needed to allow a dynamic usage impl
/// that would otherwise violate the orphan rule
struct BatchReceiver<T>(channel::Receiver<(usize, T)>);

impl<T> DynamicUsage for BatchReceiver<T> {
    fn dynamic_usage(&self) -> usize {
        // We count the memory usage of items in the channel on the receiver side.
        let num_items = self.0.len();

        // We know we use unbounded channels, so the items in the channel are stored as a
        // linked list. `crossbeam_channel` allocates memory for the linked list in blocks
        // of 31 items.
        const ITEMS_PER_BLOCK: usize = 31;
        let num_blocks = num_items.div_ceil(ITEMS_PER_BLOCK);

        // The structure of a block is:
        // - A pointer to the next block.
        // - For each slot in the block:
        //   - Space for an item.
        //   - The state of the slot, stored as an AtomicUsize.
        const PTR_SIZE: usize = std::mem::size_of::<usize>();
        let item_size = std::mem::size_of::<(usize, T)>();
        const ATOMIC_USIZE_SIZE: usize = std::mem::size_of::<AtomicUsize>();
        let block_size = PTR_SIZE + ITEMS_PER_BLOCK * (item_size + ATOMIC_USIZE_SIZE);

        num_blocks * block_size
    }

    fn dynamic_usage_bounds(&self) -> (usize, Option<usize>) {
        let usage = self.dynamic_usage();
        (usage, Some(usage))
    }
}

/// A tracker for the batch scanning tasks that are currently running.
///
/// This enables a [`BatchRunner`] to be optionally configured to track heap memory usage.
pub(crate) trait Tasks<Item> {
    type Task: Task;
    fn new() -> Self;
    fn add_task(&self, item: Item) -> Self::Task;
    fn run_task(&self, item: Item) {
        let task = self.add_task(item);
        rayon::spawn_fifo(|| task.run());
    }
}

/// A batch scanning task.
pub(crate) trait Task: Send + 'static {
    fn run(self);
}

impl<Item: Task> Tasks<Item> for () {
    type Task = Item;
    fn new() -> Self {}
    fn add_task(&self, item: Item) -> Self::Task {
        // Return the item itself as the task; we aren't tracking anything about it, so
        // there is no need to wrap it in a newtype.
        item
    }
}

/// A batch of outputs to trial decrypt.
pub(crate) struct TrialDecryptBatch<D: BatchDomain, Output, Dec: Decryptor<D, Output>> {
    tags: Vec<KeyId>,
    ivks: Vec<D::IncomingViewingKey>,
    /// We currently store outputs and repliers as parallel vectors, because
    /// [`batch::try_note_decryption`] accepts a slice of domain/output pairs
    /// rather than a value that implements `IntoIterator`, and therefore we
    /// can't just use `map` to select the parts we need in order to perform
    /// batch decryption. Ideally the domain, output, and output replier would
    /// all be part of the same struct, which would also track the output index
    /// (that is captured in the outer `OutputIndex` of each `OutputReplier`).
    outputs: Vec<(D, Output)>,
    repliers: Vec<(
        usize,
        channel::Sender<(usize, DecryptedOutput<D, Dec::Memo>)>,
    )>,
}

impl<D, Output, Dec> DynamicUsage for TrialDecryptBatch<D, Output, Dec>
where
    D: BatchDomain + DynamicUsage,
    D::IncomingViewingKey: DynamicUsage,
    Output: DynamicUsage,
    Dec: Decryptor<D, Output>,
{
    fn dynamic_usage(&self) -> usize {
        self.tags.dynamic_usage() + self.ivks.dynamic_usage() + self.outputs.dynamic_usage()
    }

    fn dynamic_usage_bounds(&self) -> (usize, Option<usize>) {
        let (tags_lower, tags_upper) = self.tags.dynamic_usage_bounds();
        let (ivks_lower, ivks_upper) = self.ivks.dynamic_usage_bounds();
        let (outputs_lower, outputs_upper) = self.outputs.dynamic_usage_bounds();

        (
            tags_lower + ivks_lower + outputs_lower,
            // The following is more concise, but harder to read
            // [tags_upper, ivks_upper, outputs_upper]
            //    .into_iter().try_fold(0, |a, b| Some(a, b?))
            match (tags_upper, ivks_upper, outputs_upper) {
                (Some(a), Some(b), Some(c)) => Some(a + b + c),
                _ => None,
            },
        )
    }
}

pub(crate) trait Batch<D>: Task + Sized
where
    D: BatchDomain<IncomingViewingKey: Send, Recipient: Send, Memo: Send>,
{
    /// The data needed to create the batch
    type Initial;

    /// The items to be processed, in the batch
    type Input;

    /// The result of processing the items
    type ResultVal: Send;

    /// As we may return more than one result, we want a unique
    /// identifier for each
    type ResultKey: Hash + Eq;

    /// The key used to identify a batch
    type BatchKey: Hash + Eq + DynamicUsage;

    fn new(init: Self::Initial) -> Self;

    fn inputs(&self) -> &Vec<Self::Input>;
    fn inputs_mut(&mut self) -> &mut Vec<Self::Input>;

    fn repliers_mut(&mut self) -> &mut Vec<(usize, channel::Sender<(usize, Self::ResultVal)>)>;

    fn is_empty(&self) -> bool {
        self.inputs().is_empty()
    }

    /// Adds the given inputs to this batch.
    ///
    /// `replier` will be called with the result of every output.
    fn add_widgets(
        &mut self,
        widgets: impl ExactSizeIterator<Item = Self::Input>,
        replier: channel::Sender<(usize, Self::ResultVal)>,
    ) {
        let widget_len = widgets.len();
        self.inputs_mut().extend(widgets);
        self.repliers_mut()
            .extend((0..widget_len).map(|output_index| (output_index, replier.clone())));
    }
    fn init_from_runner<T: Tasks<Self>>(runner: &BatchRunner<D, Self, T>) -> Self::Initial;

    fn reskey_from_batchkeyval(batchkey: &Self::BatchKey, reply_index: usize) -> Self::ResultKey;
}

impl<D, Output, Dec> Batch<D> for TrialDecryptBatch<D, Output, Dec>
where
    D: BatchDomain + Send + 'static,
    D::IncomingViewingKey: Send + Clone,
    D::Memo: Send,
    D::Note: Send,
    D::Recipient: Send,
    Output: Send + 'static,
    Dec: Decryptor<D, Output> + 'static,
    Dec::Memo: Send,
{
    type Initial = (Vec<KeyId>, Vec<D::IncomingViewingKey>);
    type Input = (D, Output);
    type ResultVal = DecryptedOutput<D, Dec::Memo>;
    type ResultKey = OutputId;
    type BatchKey = ResultKey;

    fn new((tags, ivks): (Vec<KeyId>, Vec<D::IncomingViewingKey>)) -> Self {
        assert_eq!(tags.len(), ivks.len());
        Self {
            tags,
            ivks,
            outputs: vec![],
            repliers: vec![],
        }
    }

    fn inputs(&self) -> &Vec<Self::Input> {
        &self.outputs
    }

    fn inputs_mut(&mut self) -> &mut Vec<Self::Input> {
        &mut self.outputs
    }

    fn repliers_mut(&mut self) -> &mut Vec<(usize, channel::Sender<(usize, Self::ResultVal)>)> {
        &mut self.repliers
    }

    fn init_from_runner<T: Tasks<Self>>(runner: &BatchRunner<D, Self, T>) -> Self::Initial {
        (
            runner.accumulating_batch.tags.clone(),
            runner.accumulating_batch.ivks.clone(),
        )
    }

    fn reskey_from_batchkeyval(batkey: &Self::BatchKey, reply_index: usize) -> Self::ResultKey {
        Self::ResultKey::from_parts(batkey.1, reply_index)
    }
}

impl<D, Output, Dec> Task for TrialDecryptBatch<D, Output, Dec>
where
    D: BatchDomain + Send + 'static,
    D::IncomingViewingKey: Send,
    D::Memo: Send,
    D::Note: Send,
    D::Recipient: Send,
    Output: Send + 'static,
    Dec: Decryptor<D, Output> + 'static,
    Dec::Memo: Send,
{
    /// Runs the batch of trial decryptions, and reports the results.
    fn run(self) {
        // Deconstruct self so we can consume the pieces individually.
        let Self {
            tags,
            ivks,
            outputs,
            repliers,
        } = self;

        assert_eq!(outputs.len(), repliers.len());

        let decryption_results = Dec::batch_decrypt(&tags, &ivks, &outputs);
        for (decryption_result, (index, sender)) in
            decryption_results.into_iter().zip(repliers.into_iter())
        {
            // If `decryption_result` is `None` then we will just drop `replier`,
            // indicating to the parent `BatchRunner` that this output was not for us.
            if let Some(value) = decryption_result {
                let result = (index, value);

                if sender.send(result).is_err() {
                    tracing::debug!("BatchRunner was dropped before batch finished");
                    break;
                }
            }
        }
    }
}

/// A `HashMap` key for looking up the result of a batch scanning a specific transaction.
#[derive(PartialEq, Eq, Hash)]
pub(crate) struct ResultKey(pub BlockHash, pub TxId);

impl DynamicUsage for ResultKey {
    #[inline(always)]
    fn dynamic_usage(&self) -> usize {
        0
    }

    #[inline(always)]
    fn dynamic_usage_bounds(&self) -> (usize, Option<usize>) {
        (0, Some(0))
    }
}

/// Logic to run batches of tasks on the global threadpool.
pub(crate) struct BatchRunner<D, B, T>
where
    D: BatchDomain<IncomingViewingKey: Send, Recipient: Send, Memo: Send>,
    B: Batch<D>,
    T: Tasks<B>,
{
    batch_size_threshold: usize,
    // The batch currently being accumulated.
    accumulating_batch: B,
    // The running batches.
    running_tasks: T,
    // Receivers for the results of the running batches.
    pending_results: HashMap<B::BatchKey, BatchReceiver<B::ResultVal>>,
}

impl<D, Output, Dec, T> DynamicUsage for BatchRunner<D, TrialDecryptBatch<D, Output, Dec>, T>
where
    D: BatchDomain<IncomingViewingKey: Send, Recipient: Send, Memo: Send> + DynamicUsage,
    D::IncomingViewingKey: DynamicUsage,
    Output: DynamicUsage,
    Dec: Decryptor<D, Output>,
    T: Tasks<TrialDecryptBatch<D, Output, Dec>> + DynamicUsage,
    TrialDecryptBatch<D, Output, Dec>: Batch<D>,
{
    fn dynamic_usage(&self) -> usize {
        self.accumulating_batch.dynamic_usage()
            + self.running_tasks.dynamic_usage()
            + self.pending_results.dynamic_usage()
    }

    fn dynamic_usage_bounds(&self) -> (usize, Option<usize>) {
        let running_usage = self.running_tasks.dynamic_usage();

        let bounds = (
            self.accumulating_batch.dynamic_usage_bounds(),
            self.pending_results.dynamic_usage_bounds(),
        );
        (
            bounds.0 .0 + running_usage + bounds.1 .0,
            bounds
                .0
                 .1
                .zip(bounds.1 .1)
                .map(|(a, b)| a + running_usage + b),
        )
    }
}

impl<D, B, T> BatchRunner<D, B, T>
where
    D: BatchDomain<IncomingViewingKey: Send, Recipient: Send, Memo: Send>,
    B: Batch<D>,
    T: Tasks<B>,
{
    /// Constructs a new batch runner
    pub(crate) fn new(batch_size_threshold: usize, init: B::Initial) -> Self {
        Self {
            batch_size_threshold,
            accumulating_batch: B::new(init),
            running_tasks: T::new(),
            pending_results: HashMap::default(),
        }
    }
}

impl<D, B, T> BatchRunner<D, B, T>
where
    D: BatchDomain<IncomingViewingKey: Send, Recipient: Send, Memo: Send> + Send + 'static,
    D::IncomingViewingKey: Clone + Send,
    D::Memo: Send,
    D::Note: Send,
    D::Recipient: Send,
    B: Batch<D, Initial = (Vec<KeyId>, Vec<D::IncomingViewingKey>)>,
    T: Tasks<B>,
{
    /// Batches the given inputs.
    ///
    /// `block_tag` is the hash of the block that triggered this txid being added to the
    /// batch, or the all-zeros hash to indicate that no block triggered it (i.e. it was a
    /// mempool change).
    ///
    /// If after adding the given outputs, the accumulated batch size is at least the size
    /// threshold that was set via `Self::new`, `Self::flush` is called. Subsequent calls
    /// to `Self::add_outputs` will be accumulated into a new batch.
    pub(crate) fn add_widgets(
        &mut self,
        key: B::BatchKey,
        inputs: impl ExactSizeIterator<Item = B::Input>,
    ) {
        let (tx, rx) = channel::unbounded();
        self.accumulating_batch.add_widgets(inputs, tx);
        self.pending_results.insert(key, BatchReceiver(rx));

        if self.accumulating_batch.inputs().len() >= self.batch_size_threshold {
            self.flush();
        }
    }

    /// Runs the currently accumulated batch on the global threadpool.
    ///
    /// Subsequent calls to `Self::add_outputs` will be accumulated into a new batch.
    pub(crate) fn flush(&mut self) {
        if !self.accumulating_batch.is_empty() {
            let mut batch = B::new(B::init_from_runner(self));
            mem::swap(&mut batch, &mut self.accumulating_batch);
            self.running_tasks.run_task(batch);
        }
    }

    /// Collects the pending decryption results for the given transaction.
    ///
    /// `block_tag` is the hash of the block that triggered this txid being added to the
    /// batch, or the all-zeros hash to indicate that no block triggered it (i.e. it was a
    /// mempool change).
    pub(crate) fn collect_results(
        &mut self,
        key: &B::BatchKey,
    ) -> HashMap<B::ResultKey, B::ResultVal> {
        self.pending_results
            .remove(key)
            // We won't have a pending result if the transaction didn't have outputs of
            // this runner's kind.
            .map(|BatchReceiver(rx)| {
                // This iterator will end once the channel becomes empty and disconnected.
                // We created one sender per output, and each sender is dropped after the
                // batch it is in completes (and in the case of successful decryptions,
                // after the decrypted note has been sent to the channel). Completion of
                // the iterator therefore corresponds to complete knowledge of the outputs
                // of this transaction that could be decrypted.
                rx.into_iter()
                    .map(|(output_index, value)| {
                        (B::reskey_from_batchkeyval(key, output_index), value)
                    })
                    .collect()
            })
            .unwrap_or_default()
    }
}
