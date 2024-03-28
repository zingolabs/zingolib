use crate::{
    error::{ZingoLibError, ZingoLibResult},
    wallet::{
        data::OutgoingTxData,
        keys::{address_from_pubkeyhash, unified::WalletCapability},
        notes::ShieldedNoteInterface,
        traits::{
            self as zingo_traits, Bundle as _, DomainWalletExt, Recipient as _,
            ShieldedOutputExt as _, Spend as _, ToBytes as _,
        },
        transactions::TxMapAndMaybeTrees,
    },
};
use orchard::note_encryption::OrchardDomain;
use sapling_crypto::note_encryption::SaplingDomain;
use std::{collections::HashSet, convert::TryInto, sync::Arc};
use tokio::sync::RwLock;
use zcash_client_backend::address::{Address, UnifiedAddress};
use zcash_note_encryption::try_output_recovery_with_ovk;
use zcash_primitives::{
    memo::{Memo, MemoBytes},
    transaction::{Transaction, TxId},
};
use zingo_memo::{parse_zingo_memo, ParsedMemo};
use zingo_status::confirmation_status::ConfirmationStatus;
use zingoconfig::ZingoConfig;

#[derive(Clone)]
pub struct TransactionContext {
    pub config: ZingoConfig,
    pub(crate) key: Arc<WalletCapability>,
    pub transaction_metadata_set: Arc<RwLock<TxMapAndMaybeTrees>>,
}

impl TransactionContext {
    pub fn new(
        config: &ZingoConfig,
        key: Arc<WalletCapability>,
        transaction_metadata_set: Arc<RwLock<TxMapAndMaybeTrees>>,
    ) -> Self {
        Self {
            config: config.clone(),
            key,
            transaction_metadata_set,
        }
    }
}

/// only contains code for scanning full transactions. most of our data is gleaned from compact transactions, scanned differently
pub mod scanning;
