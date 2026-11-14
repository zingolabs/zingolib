//! Transparent-only transactions that pay a transparent recipient and
//! carry an OP_RETURN (null-data) output.
//!
//! The `zcash_client_backend` proposal pipeline cannot add a null-data
//! output. The transaction is assembled directly with the upstream
//! [`Builder`]. It spends one wallet-owned transparent output that a
//! preceding deshield funded to the exact amount. It has no change output
//! and no shielded bundle. See `LightClient::propose_send_with_op_return`
//! for the full flow.
//!
//! `LightWallet::op_return_send_fee` sizes the fee before the deshield.
//! `LightWallet::build_op_return_send` builds and records the
//! transaction.

use thiserror::Error;

use zcash_keys::keys::UnifiedSpendingKey;
use zcash_primitives::transaction::builder::{BuildConfig, Builder, BundlePadding};
use zcash_primitives::transaction::fees::zip317;
use zcash_primitives::transaction::{Transaction, TxId};
use zcash_protocol::consensus::{BlockHeight, BranchId};
use zcash_protocol::value::Zatoshis;
use zcash_transparent::address::TransparentAddress;
use zcash_transparent::builder::TransparentSigningSet;
use zcash_transparent::bundle::{OutPoint, TxOut};
use zcash_transparent::keys::TransparentKeyScope;

use pepper_sync::keys::transparent::TransparentAddressId;

use super::LightWallet;
use super::error::{KeyError, WalletError};
use crate::wallet::utils;

/// The maximum size, in bytes, of an OP_RETURN payload that relays on the
/// network. This is the same limit the upstream transaction builder
/// enforces. Checking it here rejects an oversized payload before any
/// wallet state changes.
pub const MAX_OP_RETURN_BYTES: usize = 80;

/// A validated OP_RETURN payload of at most [`MAX_OP_RETURN_BYTES`] bytes.
///
/// Construct with [`OpReturnData::new`]. The length is checked once, at
/// construction.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OpReturnData(Vec<u8>);

impl OpReturnData {
    /// Validates `data` and wraps it. Fails if `data` exceeds the relay
    /// limit.
    pub fn new(data: Vec<u8>) -> Result<Self, OpReturnDataError> {
        if data.len() > MAX_OP_RETURN_BYTES {
            return Err(OpReturnDataError::TooLong {
                len: data.len(),
                max: MAX_OP_RETURN_BYTES,
            });
        }
        Ok(Self(data))
    }

    /// The validated payload bytes.
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }
}

pub const OP_RETURN: u8 = 0x6a;
const MAX_DIRECT_PUSH: u8 = 0x4b;
pub(crate) const OP_PUSHDATA1: u8 = 0x4c;
const OP_PUSHDATA2: u8 = 0x4d;

pub(crate) fn null_data_payload(script: &[u8]) -> Option<Vec<u8>> {
    let Some((&OP_RETURN, mut rest)) = script.split_first() else {
        return None;
    };
    let mut payload = Vec::new();
    while let Some((&opcode, tail)) = rest.split_first() {
        let (length, tail) = match opcode {
            0..=MAX_DIRECT_PUSH => (usize::from(opcode), tail),
            OP_PUSHDATA1 => {
                let (&length, tail) = tail.split_first()?;
                (usize::from(length), tail)
            }
            OP_PUSHDATA2 => {
                let (length, tail) = tail.split_first_chunk()?;
                (usize::from(u16::from_le_bytes(*length)), tail)
            }
            _ => return None,
        };
        payload.extend_from_slice(tail.get(..length)?);
        rest = tail.get(length..)?;
    }
    Some(payload)
}

pub(crate) fn op_return_texts(transaction: &Transaction) -> Vec<String> {
    transaction
        .transparent_bundle()
        .into_iter()
        .flat_map(|bundle| bundle.vout.iter())
        .filter_map(|out| null_data_payload(out.script_pubkey().0.0.as_slice()))
        .filter_map(|payload| String::from_utf8(payload).ok())
        .collect()
}

/// Error constructing an [`OpReturnData`].
#[derive(Debug, Error, PartialEq, Eq)]
pub enum OpReturnDataError {
    /// Payload exceeds the OP_RETURN relay limit.
    #[error("OP_RETURN payload is {len} bytes, exceeds the {max}-byte limit")]
    TooLong { len: usize, max: usize },
}

/// The build configuration for a transparent-only transaction. Every
/// shielded anchor is absent.
fn transparent_only_build_config() -> BuildConfig {
    BuildConfig::Standard {
        sapling_anchor: None,
        orchard_anchor: None,
        ironwood_anchor: None,
        orchard_padding: BundlePadding::DEFAULT,
        ironwood_padding: BundlePadding::DEFAULT,
    }
}

impl LightWallet {
    fn unified_spending_key(
        &self,
        account_id: zip32::AccountId,
    ) -> Result<UnifiedSpendingKey, WalletError> {
        Ok(self
            .unified_key_store
            .get(&account_id)
            .ok_or(KeyError::NoAccountKeys)?
            .try_into()?)
    }

    /// The ZIP-317 fee for the OP_RETURN send. The transaction has one
    /// P2PKH input, one output to `recipient`, and one null-data output
    /// carrying `data`.
    ///
    /// The upstream fee rule sizes the transaction from its input and
    /// output sizes. No keys are needed. A watch-only wallet can size the
    /// fee.
    pub(crate) fn op_return_send_fee(
        &self,
        recipient: &TransparentAddress,
        data: &OpReturnData,
        target_height: BlockHeight,
    ) -> Result<Zatoshis, WalletError> {
        use zcash_primitives::transaction::fees::FeeRule as _;
        use zcash_primitives::transaction::fees::transparent::{InputSize, OutputView as _};
        use zcash_transparent::builder::TransparentBuilder;

        let mut outputs = TransparentBuilder::empty();
        outputs.add_output(recipient, Zatoshis::ZERO).map_err(|e| {
            WalletError::TransparentBuild(format!("fee estimate recipient output: {e:?}"))
        })?;
        outputs
            .add_null_data_output(data.as_bytes())
            .map_err(|e| WalletError::TransparentBuild(format!("fee estimate op_return: {e:?}")))?;

        zip317::FeeRule::standard()
            .fee_required(
                &self.chain_type,
                target_height,
                [InputSize::STANDARD_P2PKH],
                outputs.outputs().iter().map(|out| out.serialized_size()),
                0,
                0,
                0,
                0,
            )
            .map_err(|e| WalletError::TransparentBuild(format!("fee estimate: {e:?}")))
    }

    /// Builds and records the OP_RETURN send. The transaction spends
    /// `source_outpoint` to pay `amount` to `recipient` and carries `data`
    /// in a null-data output. The input value must equal `amount` plus the
    /// fee from [`Self::op_return_send_fee`]. The transaction has no change
    /// output. The builder rejects any other input value.
    ///
    /// Returns the txid. The transaction is recorded with `Calculated`
    /// status for `transmit_transactions`.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn build_op_return_send(
        &mut self,
        account_id: zip32::AccountId,
        source_address_id: TransparentAddressId,
        source_outpoint: OutPoint,
        source_txout: TxOut,
        recipient: &TransparentAddress,
        amount: Zatoshis,
        data: &OpReturnData,
        target_height: BlockHeight,
    ) -> Result<TxId, WalletError> {
        let raw_tx = self.build_op_return_send_raw(
            account_id,
            source_address_id,
            source_outpoint,
            source_txout,
            recipient,
            amount,
            data,
            target_height,
        )?;
        self.record_transparent_transaction(&raw_tx, target_height)
    }

    /// Builds and signs the OP_RETURN send without recording it. Returns
    /// the raw transaction bytes. [`Self::build_op_return_send`] adds
    /// persistence.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn build_op_return_send_raw(
        &self,
        account_id: zip32::AccountId,
        source_address_id: TransparentAddressId,
        source_outpoint: OutPoint,
        source_txout: TxOut,
        recipient: &TransparentAddress,
        amount: Zatoshis,
        data: &OpReturnData,
        target_height: BlockHeight,
    ) -> Result<Vec<u8>, WalletError> {
        let usk = self.unified_spending_key(account_id)?;
        let secret_key = usk
            .transparent()
            .derive_secret_key(
                TransparentKeyScope::from(source_address_id.scope()),
                source_address_id.address_index(),
            )
            .map_err(|e| {
                WalletError::TransparentBuild(format!("transparent key derivation: {e}"))
            })?;
        let mut signing_set = TransparentSigningSet::new();
        let pubkey = signing_set.add_key(secret_key);

        let mut builder = Builder::new(
            self.chain_type,
            target_height,
            transparent_only_build_config(),
        )
        .with_expiry_height(crate::wallet::expiry::tx_expiry_height(
            &self.chain_type,
            target_height,
        ));
        builder
            .add_transparent_p2pkh_input(pubkey, source_outpoint, source_txout)
            .map_err(|e| WalletError::TransparentBuild(format!("input: {e:?}")))?;
        builder
            .add_transparent_output(recipient, amount)
            .map_err(|e| WalletError::TransparentBuild(format!("recipient output: {e:?}")))?;
        builder
            .add_transparent_null_data_output::<std::convert::Infallible>(data.as_bytes())
            .map_err(|e| WalletError::TransparentBuild(format!("op_return: {e:?}")))?;

        let prover = utils::LazySaplingProver::default();
        let build_result = builder
            .build(
                &signing_set,
                &[],
                &[],
                crate::utils::system_rng(),
                &prover,
                &prover,
                &zip317::FeeRule::standard(),
            )
            .map_err(|e| WalletError::TransparentBuild(format!("build: {e:?}")))?;

        let mut raw_tx = Vec::new();
        build_result
            .transaction()
            .write(&mut raw_tx)
            .map_err(WalletError::TransactionWrite)?;
        Ok(raw_tx)
    }

    /// Finds the transparent output that pays `address` in the wallet
    /// transaction `txid`. Returns the outpoint and the coin.
    pub(crate) fn find_transparent_output(
        &self,
        txid: TxId,
        address: &TransparentAddress,
    ) -> Result<(OutPoint, TxOut), WalletError> {
        let wallet_transaction = self
            .wallet_transactions
            .get(&txid)
            .ok_or(WalletError::TransactionNotFound(txid))?;
        let bundle = wallet_transaction
            .transaction()
            .transparent_bundle()
            .ok_or(WalletError::DeshieldOutputNotFound)?;
        let script: zcash_transparent::address::Script = address.script().into();
        let (index, txout) = bundle
            .vout
            .iter()
            .enumerate()
            .find(|(_, out)| *out.script_pubkey() == script)
            .ok_or(WalletError::DeshieldOutputNotFound)?;
        Ok((OutPoint::new(txid.into(), index as u32), txout.clone()))
    }

    /// Records a calculated transparent transaction in the wallet. Marks
    /// the spent coins and stores the transaction with `Calculated` status
    /// for `transmit_transactions`. This is the same procedure
    /// [`crate::wallet::migration`] uses for its calculated transactions.
    fn record_transparent_transaction(
        &mut self,
        raw_tx: &[u8],
        target_height: BlockHeight,
    ) -> Result<TxId, WalletError> {
        let transaction = Transaction::read(
            raw_tx,
            BranchId::for_height(&self.chain_type, target_height),
        )
        .map_err(WalletError::TransactionRead)?;
        let txid = transaction.txid();

        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system clock after unix epoch")
            .as_secs() as u32;
        let chain_type = self.chain_type;
        let ufvks = pepper_sync::wallet::traits::SyncWallet::get_unified_full_viewing_keys(self)?;
        match pepper_sync::scan_pending_transaction(
            &chain_type,
            &ufvks,
            self,
            transaction,
            zl_common::status::ConfirmationStatus::Calculated(target_height),
            timestamp,
        ) {
            Ok(()) => (),
            Err(pepper_sync::error::SyncError::ScanError(e)) => return Err(e.into()),
            Err(pepper_sync::error::SyncError::WalletError(e)) => return Err(e),
            Err(_) => {
                panic!("`scan_pending_transaction` should only return scan or wallet errors")
            }
        }
        self.save_required = true;

        Ok(txid)
    }
}

#[cfg(test)]
mod tests;
