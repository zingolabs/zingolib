use std::collections::BTreeMap;

use zcash_client_backend::data_api::locking::LockOwner;
use zcash_client_backend::wallet::OutputRef;
use zcash_protocol::consensus::BlockHeight;

/// Who holds one advisory output lock, and the last height at which it binds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct OutputLock {
    owner: LockOwner,
    expiry_height: BlockHeight,
}

/// Every advisory output lock the wallet holds, for the life of the process.
#[derive(Debug, Clone, Default)]
pub struct OutputLocks(BTreeMap<OutputRef, OutputLock>);

impl OutputLocks {
    /// Locks every output for `owner` until `expiry_height`, or locks none and
    /// names the first output whose lock another owner still holds at `chain_tip`.
    pub fn acquire(
        &mut self,
        outputs: &[OutputRef],
        owner: LockOwner,
        expiry_height: BlockHeight,
        chain_tip: BlockHeight,
    ) -> Result<usize, OutputRef> {
        if let Some(held) = outputs.iter().find(|output| {
            self.0
                .get(output)
                .is_some_and(|lock| lock.owner != owner && lock.expiry_height > chain_tip)
        }) {
            return Err(*held);
        }
        for output in outputs {
            self.0.insert(
                *output,
                OutputLock {
                    owner,
                    expiry_height,
                },
            );
        }
        Ok(outputs.len())
    }

    /// Releases `output` when `owner` holds its lock, reporting whether a lock went away.
    pub fn release(&mut self, output: &OutputRef, owner: LockOwner) -> bool {
        if self.0.get(output).is_some_and(|lock| lock.owner == owner) {
            self.0.remove(output);
            return true;
        }
        false
    }

    /// Discards `output`'s lock whoever holds it, reporting whether a lock went away.
    pub fn discard(&mut self, output: &OutputRef) -> bool {
        self.0.remove(output).is_some()
    }

    /// Every locked output paired with the last height at which its lock binds.
    pub fn iter(&self) -> impl Iterator<Item = (OutputRef, BlockHeight)> + '_ {
        self.0
            .iter()
            .map(|(output, lock)| (*output, lock.expiry_height))
    }
}
