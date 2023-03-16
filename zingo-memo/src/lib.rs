use zcash_client_backend::address::UnifiedAddress;
use zcash_primitives::consensus::{BlockHeight, MAIN_NETWORK};

pub mod memo_serde;
pub mod utils;

/// A parsed memo. Currently there are two versions of this protocol.
///
/// Version zero is a list of UAs. The main use-case for this is to record the
/// UAs sent to, as the blockchain only records the pool-specific receiver
/// corresponding to the key we are sending to.
///
/// Version one extends version zero with a vec of heights and indexes.
/// The intended use for this format is to encode the height-on-chain, and
/// index-in-block of a set of transactions. This can be used to note previous transactions
/// of note, allowing for a rescan to find the highest of these memos, and learn
/// all previous transactions of note (potentially chaining multiple memos), and
/// remove the need to trial-decrypt any transactions not noted this way.
#[non_exhaustive]
#[derive(Debug)]
pub enum ParsedMemo {
    Version0 {
        uas: Vec<UnifiedAddress>,
    },
    Version1 {
        uas: Vec<UnifiedAddress>,
        transaction_heights_and_indexes: Vec<(BlockHeight, usize)>,
    },
}

impl PartialEq for ParsedMemo {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (ParsedMemo::Version0 { uas: uas_self }, ParsedMemo::Version0 { uas: uas_other }) => {
                uas_self
                    .iter()
                    .map(|ua| ua.encode(&MAIN_NETWORK))
                    .eq(uas_other.iter().map(|ua| ua.encode(&MAIN_NETWORK)))
            }
            (
                ParsedMemo::Version1 {
                    uas: uas_self,
                    transaction_heights_and_indexes: transaction_heights_and_indexes_self,
                },
                ParsedMemo::Version1 {
                    uas: uas_other,
                    transaction_heights_and_indexes: transaction_heights_and_indexes_other,
                },
            ) => {
                uas_self
                    .iter()
                    .map(|ua| ua.encode(&MAIN_NETWORK))
                    .eq(uas_other.iter().map(|ua| ua.encode(&MAIN_NETWORK)))
                    && transaction_heights_and_indexes_self == transaction_heights_and_indexes_other
            }
            _ => false,
        }
    }
}

#[cfg(test)]
mod test_vectors;
