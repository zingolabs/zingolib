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

use rand::rngs::OsRng;
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
    /// The account's [`UnifiedSpendingKey`]. Callers derive the transparent
    /// signing key from it and leave the `secp256k1` types inferred. The
    /// `secp256k1` version used by `zcash_transparent` differs from the
    /// version zingolib depends on.
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
        );
        builder
            .add_transparent_p2pkh_input(pubkey, source_outpoint, source_txout)
            .map_err(|e| WalletError::TransparentBuild(format!("input: {e:?}")))?;
        builder
            .add_transparent_output(recipient, amount)
            .map_err(|e| WalletError::TransparentBuild(format!("recipient output: {e:?}")))?;
        builder
            .add_transparent_null_data_output::<std::convert::Infallible>(data.as_bytes())
            .map_err(|e| WalletError::TransparentBuild(format!("op_return: {e:?}")))?;

        let (sapling_output, sapling_spend) = crate::wallet::utils::read_sapling_params();
        let prover =
            zcash_proofs::prover::LocalTxProver::from_bytes(&sapling_spend, &sapling_output);
        let build_result = builder
            .build(
                &signing_set,
                &[],
                &[],
                OsRng,
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
            zingo_status::confirmation_status::ConfirmationStatus::Calculated(target_height),
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
mod tests {
    use zcash_protocol::value::Zatoshis;
    use zcash_transparent::address::TransparentAddress;
    use zcash_transparent::bundle::{OutPoint, TxOut};

    use super::*;
    use crate::testutils::synthetic_wallet::SyntheticWalletBuilder;
    use crate::wallet::LightWallet;

    const ACCOUNT: zip32::AccountId = zip32::AccountId::ZERO;
    const TIP: u32 = 20;
    /// A payload under the 80-byte limit.
    const PAYLOAD: &[u8] = b"zingolib op_return payload";

    #[test]
    fn accepts_payload_at_the_limit() {
        let data = vec![0u8; MAX_OP_RETURN_BYTES];
        assert!(OpReturnData::new(data).is_ok());
    }

    #[test]
    fn rejects_payload_over_the_limit() {
        let data = vec![0u8; MAX_OP_RETURN_BYTES + 1];
        assert_eq!(
            OpReturnData::new(data),
            Err(OpReturnDataError::TooLong {
                len: MAX_OP_RETURN_BYTES + 1,
                max: MAX_OP_RETURN_BYTES,
            })
        );
    }

    #[test]
    fn accepts_empty_payload() {
        assert_eq!(OpReturnData::new(vec![]).unwrap().as_bytes(), b"");
    }

    #[test]
    fn preserves_the_payload_bytes() {
        let data = OpReturnData::new(PAYLOAD.to_vec()).unwrap();
        assert_eq!(data.as_bytes(), PAYLOAD);
    }

    /// An arbitrary external transparent (P2PKH) recipient address.
    fn recipient() -> TransparentAddress {
        TransparentAddress::PublicKeyHash([2u8; 20])
    }

    fn synced_wallet() -> LightWallet {
        SyntheticWalletBuilder::new(zingo_test_vectors::seeds::HOSPITAL_MUSEUM_SEED)
            .tip(TIP)
            .build()
    }

    /// Reserves a source address and sizes the fee for `payload`. Returns
    /// every value the build needs.
    fn send_fixture(
        wallet: &mut LightWallet,
        amount: u64,
        payload: &[u8],
    ) -> (
        TransparentAddressId,
        TransparentAddress,
        TransparentAddress,
        OpReturnData,
        Zatoshis,
        BlockHeight,
        Zatoshis,
    ) {
        let amount = Zatoshis::const_from_u64(amount);
        let data = OpReturnData::new(payload.to_vec()).expect("payload within limit");
        let recipient = recipient();
        let (source_id, source) = wallet
            .generate_refund_addresses(1, ACCOUNT)
            .expect("wallet can reserve a refund address")
            .into_iter()
            .next()
            .expect("one address requested, one returned");
        let target_height = wallet
            .get_migration_heights()
            .expect("height read")
            .expect("synced wallet has heights")
            .0;
        let fee = wallet
            .op_return_send_fee(&recipient, &data, target_height)
            .expect("fee estimate");
        (
            source_id,
            source,
            recipient,
            data,
            amount,
            target_height,
            fee,
        )
    }

    /// The raw script bytes of a transparent output.
    fn script_bytes(out: &TxOut) -> Vec<u8> {
        out.script_pubkey().0.0.clone()
    }

    /// Reads raw bytes back into a `Transaction` for inspection.
    fn read_tx(wallet: &LightWallet, raw: &[u8], target_height: BlockHeight) -> Transaction {
        Transaction::read(
            raw,
            BranchId::for_height(&wallet.chain_type(), target_height),
        )
        .expect("bytes read back")
    }

    /// The send spends the source output, pays the recipient, carries the
    /// payload in an OP_RETURN, and has no change output.
    #[test]
    fn send_pays_recipient_carries_payload_and_has_no_change() {
        let mut wallet = synced_wallet();
        let amount_u64 = 100_000;
        let (source_id, source, recipient, data, amount, target_height, fee) =
            send_fixture(&mut wallet, amount_u64, PAYLOAD);

        let outpoint = OutPoint::new([7u8; 32], 0);
        let input_value = (amount + fee).expect("input value in range");
        let txout = TxOut::new(input_value, source.script().into());

        let raw = wallet
            .build_op_return_send_raw(
                ACCOUNT,
                source_id,
                outpoint,
                txout,
                &recipient,
                amount,
                &data,
                target_height,
            )
            .expect("send builds");
        let transaction = read_tx(&wallet, &raw, target_height);
        let bundle = transaction
            .transparent_bundle()
            .expect("a transparent transaction");

        assert_eq!(bundle.vin.len(), 1, "spends exactly the source output");
        assert_eq!(bundle.vin[0].prevout().n(), 0);
        assert_eq!(bundle.vin[0].prevout().hash(), &[7u8; 32]);

        assert_eq!(
            bundle.vout.len(),
            2,
            "recipient + OP_RETURN, no change output"
        );

        let recipient_script: zcash_transparent::address::Script = recipient.script().into();
        let recipient_out = bundle
            .vout
            .iter()
            .find(|out| *out.script_pubkey() == recipient_script)
            .expect("a recipient output");
        assert_eq!(
            recipient_out.value(),
            amount,
            "recipient is paid the amount"
        );

        let op_return_out = bundle
            .vout
            .iter()
            .find(|out| out.value() == Zatoshis::ZERO)
            .expect("a zero-value OP_RETURN output");
        let script = script_bytes(op_return_out);
        assert_eq!(script[0], 0x6a, "null-data script starts with OP_RETURN");
        assert!(
            script.windows(PAYLOAD.len()).any(|w| w == PAYLOAD),
            "the OP_RETURN carries the payload verbatim"
        );

        assert_eq!(
            (input_value - amount).and_then(|v| v - Zatoshis::ZERO),
            Some(fee),
            "input funds the payment and the fee exactly"
        );
    }

    /// The keyless fee is exact. An input worth amount plus that fee
    /// builds with no change output, for every push-encoding class of the
    /// payload and for a P2SH recipient. The builder's value-balance check
    /// rejects any other fee.
    #[test]
    fn keyless_fee_balances_the_built_transaction() {
        let mut wallet = synced_wallet();
        let amount = Zatoshis::const_from_u64(100_000);
        let p2sh = TransparentAddress::ScriptHash([3u8; 20]);
        let cases = [
            (recipient(), 0usize),
            (recipient(), 1),
            (recipient(), 75),
            (recipient(), 76),
            (recipient(), 80),
            (p2sh, 80),
        ];
        for (recipient, payload_len) in cases {
            let data = OpReturnData::new(vec![0xab; payload_len]).unwrap();
            let (source_id, source) = wallet.generate_refund_addresses(1, ACCOUNT).unwrap()[0];
            let target_height = wallet.get_migration_heights().unwrap().unwrap().0;
            let fee = wallet
                .op_return_send_fee(&recipient, &data, target_height)
                .unwrap();
            let txout = TxOut::new((amount + fee).unwrap(), source.script().into());
            let raw = wallet
                .build_op_return_send_raw(
                    ACCOUNT,
                    source_id,
                    OutPoint::new([7u8; 32], 0),
                    txout,
                    &recipient,
                    amount,
                    &data,
                    target_height,
                )
                .unwrap_or_else(|e| panic!("payload {payload_len} to {recipient:?}: {e}"));
            let transaction = read_tx(&wallet, &raw, target_height);
            assert_eq!(
                transaction.transparent_bundle().unwrap().vout.len(),
                2,
                "payload {payload_len}: recipient + OP_RETURN, no change"
            );
        }
    }

    /// The ZIP-317 fee is a positive multiple of the marginal fee. An
    /// 80-byte payload never costs less than a short one.
    #[test]
    fn fee_grows_with_payload_size() {
        let mut wallet = synced_wallet();
        let (_, _, _, _, _, _, short_fee) = send_fixture(&mut wallet, 100_000, b"short");
        let (_, _, _, _, _, _, max_fee) = send_fixture(&mut wallet, 100_000, &[0u8; 80]);

        assert!(
            u64::from(short_fee) >= 10_000,
            "at least the ZIP-317 grace floor"
        );
        assert_eq!(
            u64::from(short_fee) % 5_000,
            0,
            "a multiple of the marginal fee"
        );
        assert_eq!(
            u64::from(max_fee) % 5_000,
            0,
            "a multiple of the marginal fee"
        );
        assert!(
            max_fee >= short_fee,
            "a larger payload never costs less: {max_fee:?} vs {short_fee:?}"
        );
    }

    /// An input worth less than amount plus fee cannot balance. The build
    /// is refused.
    #[test]
    fn rejects_underfunded_input() {
        let mut wallet = synced_wallet();
        let (source_id, source, recipient, data, amount, target_height, fee) =
            send_fixture(&mut wallet, 100_000, PAYLOAD);

        let underfunded = ((amount + fee).unwrap() - Zatoshis::const_from_u64(1)).unwrap();
        let txout = TxOut::new(underfunded, source.script().into());

        let result = wallet.build_op_return_send_raw(
            ACCOUNT,
            source_id,
            OutPoint::new([7u8; 32], 0),
            txout,
            &recipient,
            amount,
            &data,
            target_height,
        );
        assert!(result.is_err(), "an underfunded send cannot balance");
    }

    /// An input worth more than amount plus fee cannot balance. The send
    /// has no change output to absorb the surplus. The build is refused.
    #[test]
    fn rejects_overfunded_input() {
        let mut wallet = synced_wallet();
        let (source_id, source, recipient, data, amount, target_height, fee) =
            send_fixture(&mut wallet, 100_000, PAYLOAD);

        let overfunded = (amount + (fee + Zatoshis::const_from_u64(1_000)).unwrap()).unwrap();
        let txout = TxOut::new(overfunded, source.script().into());

        let result = wallet.build_op_return_send_raw(
            ACCOUNT,
            source_id,
            OutPoint::new([7u8; 32], 0),
            txout,
            &recipient,
            amount,
            &data,
            target_height,
        );
        assert!(
            result.is_err(),
            "surplus with no change output cannot balance"
        );
    }

    /// An empty payload is a valid OP_RETURN. The build succeeds.
    #[test]
    fn accepts_empty_payload_send() {
        let mut wallet = synced_wallet();
        let (source_id, source, recipient, data, amount, target_height, fee) =
            send_fixture(&mut wallet, 100_000, b"");

        let txout = TxOut::new((amount + fee).unwrap(), source.script().into());
        let raw = wallet
            .build_op_return_send_raw(
                ACCOUNT,
                source_id,
                OutPoint::new([7u8; 32], 0),
                txout,
                &recipient,
                amount,
                &data,
                target_height,
            )
            .expect("empty-payload send builds");
        let transaction = read_tx(&wallet, &raw, target_height);
        assert_eq!(
            transaction
                .transparent_bundle()
                .expect("transparent")
                .vout
                .len(),
            2,
            "recipient + empty OP_RETURN"
        );
    }
}
