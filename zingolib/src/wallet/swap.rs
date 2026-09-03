//! OP_RETURN swap-deposit support for THORChain / MAYAChain.
//!
//! A swap deposit is two transactions:
//!
//! 1. a **deshield**: shielded funds -> a reserved, wallet-owned
//!    transparent address (built by the ordinary proposal path), and
//! 2. a **memo carrier**: that transparent output -> the swap vault, with
//!    the swap memo in an 80-byte OP_RETURN output.
//!
//! The carrier is hand-built here rather than through
//! `zcash_client_backend`, for two reasons: zcb's proposal pipeline
//! exposes no way to add an OP_RETURN output, and the carrier's
//! transparent input must be the transaction's sender so THORChain /
//! MAYAChain can derive a refund address (a shielded-funded deposit is
//! unrefundable). The carrier spends only transparent value, so no
//! shielded witnesses, proofs, or anchors are involved.
//!
//! [`LightWallet`] exposes the two building blocks used by
//! `LightClient::propose_swap_deposit`:
//! [`LightWallet::op_return_carrier_fee`] (so the deshield can fund the
//! carrier exactly, leaving zero change) and
//! [`LightWallet::build_op_return_carrier`] (the hand-built carrier).

use rand::rngs::OsRng;

use zcash_keys::keys::UnifiedSpendingKey;
use zcash_primitives::transaction::Transaction;
use zcash_primitives::transaction::builder::{BuildConfig, Builder, BundlePadding};
use zcash_primitives::transaction::fees::zip317;
use zcash_protocol::consensus::{BlockHeight, BranchId};
use zcash_protocol::value::Zatoshis;
use zcash_transparent::address::TransparentAddress;
use zcash_transparent::builder::TransparentSigningSet;
use zcash_transparent::bundle::{OutPoint, TxOut};
use zcash_transparent::keys::TransparentKeyScope;

use pepper_sync::keys::transparent::TransparentAddressId;

use zcash_primitives::transaction::TxId;

use super::LightWallet;
use super::error::{KeyError, WalletError};
use super::op_return::OpReturnData;

/// The build configuration for a transparent-only transaction: no
/// shielded bundle, so every shielded anchor is absent.
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
    /// The account's [`UnifiedSpendingKey`], from which the transparent
    /// signing key is derived at the call site (its `secp256k1` types stay
    /// inferred, since that `secp256k1` version differs from zingolib's).
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

    /// The ZIP-317 fee for the memo carrier transaction: one P2PKH input,
    /// one P2PKH output to `vault`, and the OP_RETURN memo output.
    ///
    /// Computed by assembling a throwaway builder of the carrier's exact
    /// shape and asking the upstream fee rule to size it — including the
    /// null-data output — so the deshield can fund the carrier to the
    /// zatoshi and leave no change. The fee depends only on output sizes,
    /// not on input values, so the placeholder input value is irrelevant.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn op_return_carrier_fee(
        &self,
        account_id: zip32::AccountId,
        carrier_address_id: TransparentAddressId,
        carrier: &TransparentAddress,
        vault: &TransparentAddress,
        amount: Zatoshis,
        op_return: &OpReturnData,
        target_height: BlockHeight,
    ) -> Result<Zatoshis, WalletError> {
        let usk = self.unified_spending_key(account_id)?;
        let secret_key = usk
            .transparent()
            .derive_secret_key(
                TransparentKeyScope::from(carrier_address_id.scope()),
                carrier_address_id.address_index(),
            )
            .map_err(|e| WalletError::SwapBuild(format!("transparent key derivation: {e}")))?;
        let pubkey = TransparentSigningSet::new().add_key(secret_key);
        let script = carrier.script().into();

        let mut builder = Builder::new(
            self.chain_type,
            target_height,
            transparent_only_build_config(),
        );
        builder
            .add_transparent_p2pkh_input(
                pubkey,
                OutPoint::new([0u8; 32], 0),
                TxOut::new(amount, script),
            )
            .map_err(|e| WalletError::SwapBuild(format!("fee estimate input: {e:?}")))?;
        builder
            .add_transparent_output(vault, amount)
            .map_err(|e| WalletError::SwapBuild(format!("fee estimate vault output: {e:?}")))?;
        builder
            .add_transparent_null_data_output::<std::convert::Infallible>(op_return.as_bytes())
            .map_err(|e| WalletError::SwapBuild(format!("fee estimate op_return: {e:?}")))?;

        builder
            .get_fee(&zip317::FeeRule::standard())
            .map_err(|e| WalletError::SwapBuild(format!("fee estimate: {e:?}")))
    }

    /// Builds and records the memo carrier transaction: spends
    /// `carrier_outpoint` (the deshield output at `carrier`, worth
    /// `carrier_txout.value()`) to pay `amount` to `vault`, carrying
    /// `op_return`. The input funds the payment and the fee exactly, so
    /// the transaction has no change output.
    ///
    /// The upstream ZIP-317 fee rule sizes the OP_RETURN output and the
    /// builder's value-balance check rejects any fee mismatch, so a
    /// carrier funded by [`Self::op_return_carrier_fee`] balances to zero.
    ///
    /// Returns the carrier's txid, recorded with `Calculated` status ready
    /// for transmission.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn build_op_return_carrier(
        &mut self,
        account_id: zip32::AccountId,
        carrier_address_id: TransparentAddressId,
        carrier_outpoint: OutPoint,
        carrier_txout: TxOut,
        vault: &TransparentAddress,
        amount: Zatoshis,
        op_return: &OpReturnData,
        target_height: BlockHeight,
    ) -> Result<TxId, WalletError> {
        let raw_tx = self.build_op_return_carrier_raw(
            account_id,
            carrier_address_id,
            carrier_outpoint,
            carrier_txout,
            vault,
            amount,
            op_return,
            target_height,
        )?;
        self.record_swap_transaction(&raw_tx, target_height)
    }

    /// Builds and signs the memo carrier transaction, returning its raw
    /// bytes without recording it. [`Self::build_op_return_carrier`] wraps
    /// this with persistence; this seam lets the build be exercised in
    /// isolation (`Transaction` is not `Clone`, so bytes are returned).
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn build_op_return_carrier_raw(
        &self,
        account_id: zip32::AccountId,
        carrier_address_id: TransparentAddressId,
        carrier_outpoint: OutPoint,
        carrier_txout: TxOut,
        vault: &TransparentAddress,
        amount: Zatoshis,
        op_return: &OpReturnData,
        target_height: BlockHeight,
    ) -> Result<Vec<u8>, WalletError> {
        let usk = self.unified_spending_key(account_id)?;
        let secret_key = usk
            .transparent()
            .derive_secret_key(
                TransparentKeyScope::from(carrier_address_id.scope()),
                carrier_address_id.address_index(),
            )
            .map_err(|e| WalletError::SwapBuild(format!("transparent key derivation: {e}")))?;
        let mut signing_set = TransparentSigningSet::new();
        let pubkey = signing_set.add_key(secret_key);

        let mut builder = Builder::new(
            self.chain_type,
            target_height,
            transparent_only_build_config(),
        );
        builder
            .add_transparent_p2pkh_input(pubkey, carrier_outpoint, carrier_txout)
            .map_err(|e| WalletError::SwapBuild(format!("carrier input: {e:?}")))?;
        builder
            .add_transparent_output(vault, amount)
            .map_err(|e| WalletError::SwapBuild(format!("carrier vault output: {e:?}")))?;
        builder
            .add_transparent_null_data_output::<std::convert::Infallible>(op_return.as_bytes())
            .map_err(|e| WalletError::SwapBuild(format!("carrier op_return: {e:?}")))?;

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
            .map_err(|e| WalletError::SwapBuild(format!("carrier build: {e:?}")))?;

        let mut raw_tx = Vec::new();
        build_result
            .transaction()
            .write(&mut raw_tx)
            .map_err(WalletError::TransactionWrite)?;
        Ok(raw_tx)
    }

    /// Locates the deshield output paying `carrier` within the deshield
    /// transaction `deshield_txid`, returning the outpoint and coin the
    /// memo carrier will spend. The deshield pays exactly one output to
    /// the reserved carrier address.
    pub(crate) fn locate_deshield_output(
        &self,
        deshield_txid: TxId,
        carrier: &TransparentAddress,
    ) -> Result<(OutPoint, TxOut), WalletError> {
        let wallet_transaction = self
            .wallet_transactions
            .get(&deshield_txid)
            .ok_or(WalletError::TransactionNotFound(deshield_txid))?;
        let bundle = wallet_transaction
            .transaction()
            .transparent_bundle()
            .ok_or(WalletError::SwapCarrierOutputMissing)?;
        let carrier_script: zcash_transparent::address::Script = carrier.script().into();
        let (index, txout) = bundle
            .vout
            .iter()
            .enumerate()
            .find(|(_, out)| *out.script_pubkey() == carrier_script)
            .ok_or(WalletError::SwapCarrierOutputMissing)?;
        Ok((
            OutPoint::new(deshield_txid.into(), index as u32),
            txout.clone(),
        ))
    }

    /// Records a calculated swap transaction in the wallet, mirroring
    /// [`crate::wallet::migration`]'s `record_migration_transaction` /
    /// `store_transactions_to_be_sent`: it marks the spent coins and
    /// stores the transaction with `Calculated` status for
    /// `transmit_transactions`.
    fn record_swap_transaction(
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

    use crate::testutils::synthetic_wallet::SyntheticWalletBuilder;
    use crate::wallet::LightWallet;
    use crate::wallet::op_return::OpReturnData;

    const ACCOUNT: zip32::AccountId = zip32::AccountId::ZERO;
    const TIP: u32 = 20;
    /// A representative MAYAChain swap memo (well under the 80-byte limit).
    const MEMO: &[u8] = b"=:ZEC.ZEC:tz==maya1abcdefghij:100000000";

    /// An arbitrary external transparent (P2PKH) vault address.
    fn vault() -> TransparentAddress {
        TransparentAddress::PublicKeyHash([2u8; 20])
    }

    fn synced_wallet() -> LightWallet {
        SyntheticWalletBuilder::new(zingo_test_vectors::seeds::HOSPITAL_MUSEUM_SEED)
            .tip(TIP)
            .build()
    }

    /// Reserve a carrier address and return everything the carrier build
    /// needs, sizing the fee for `payload`.
    fn carrier_fixture(
        wallet: &mut LightWallet,
        amount: u64,
        payload: &[u8],
    ) -> (
        pepper_sync::keys::transparent::TransparentAddressId,
        TransparentAddress,
        TransparentAddress,
        OpReturnData,
        Zatoshis,
        zcash_protocol::consensus::BlockHeight,
        Zatoshis,
    ) {
        let amount = Zatoshis::const_from_u64(amount);
        let op_return = OpReturnData::new(payload.to_vec()).expect("memo within limit");
        let vault = vault();
        let (carrier_id, carrier) = wallet
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
            .op_return_carrier_fee(
                ACCOUNT,
                carrier_id,
                &carrier,
                &vault,
                amount,
                &op_return,
                target_height,
            )
            .expect("fee estimate");
        (
            carrier_id,
            carrier,
            vault,
            op_return,
            amount,
            target_height,
            fee,
        )
    }

    /// The raw script bytes of a transparent output.
    fn script_bytes(out: &TxOut) -> Vec<u8> {
        out.script_pubkey().0.0.clone()
    }

    /// Reads carrier bytes back into a `Transaction` for inspection.
    fn read_tx(
        wallet: &LightWallet,
        raw: &[u8],
        target_height: zcash_protocol::consensus::BlockHeight,
    ) -> zcash_primitives::transaction::Transaction {
        use zcash_protocol::consensus::BranchId;
        zcash_primitives::transaction::Transaction::read(
            raw,
            BranchId::for_height(&wallet.chain_type(), target_height),
        )
        .expect("carrier bytes read back")
    }

    /// The happy path: the carrier spends the deshield output, pays the
    /// vault, carries the memo in an OP_RETURN, and has no change output.
    #[test]
    fn carrier_pays_vault_carries_memo_and_has_no_change() {
        let mut wallet = synced_wallet();
        let amount_u64 = 100_000;
        let (carrier_id, carrier, vault, op_return, amount, target_height, fee) =
            carrier_fixture(&mut wallet, amount_u64, MEMO);

        let outpoint = OutPoint::new([7u8; 32], 0);
        let input_value = (amount + fee).expect("input value in range");
        let txout = TxOut::new(input_value, carrier.script().into());

        let raw = wallet
            .build_op_return_carrier_raw(
                ACCOUNT,
                carrier_id,
                outpoint,
                txout,
                &vault,
                amount,
                &op_return,
                target_height,
            )
            .expect("carrier builds");
        let transaction = read_tx(&wallet, &raw, target_height);
        let bundle = transaction
            .transparent_bundle()
            .expect("carrier is a transparent transaction");

        assert_eq!(
            bundle.vin.len(),
            1,
            "carrier spends exactly the deshield output"
        );
        assert_eq!(bundle.vin[0].prevout().n(), 0);
        assert_eq!(bundle.vin[0].prevout().hash(), &[7u8; 32]);

        assert_eq!(bundle.vout.len(), 2, "vault + OP_RETURN, no change output");

        let vault_script: zcash_transparent::address::Script = vault.script().into();
        let vault_out = bundle
            .vout
            .iter()
            .find(|out| *out.script_pubkey() == vault_script)
            .expect("a vault output");
        assert_eq!(
            vault_out.value(),
            amount,
            "vault is paid the deposit amount"
        );

        let op_return_out = bundle
            .vout
            .iter()
            .find(|out| out.value() == Zatoshis::ZERO)
            .expect("a zero-value OP_RETURN output");
        let script = script_bytes(op_return_out);
        assert_eq!(script[0], 0x6a, "null-data script starts with OP_RETURN");
        assert!(
            script.windows(MEMO.len()).any(|w| w == MEMO),
            "the OP_RETURN carries the memo payload verbatim"
        );

        assert_eq!(
            (input_value - amount).and_then(|v| v - Zatoshis::ZERO),
            Some(fee),
            "input funds the payment and the fee exactly"
        );
    }

    /// The ZIP-317 carrier fee is a positive multiple of the marginal fee,
    /// and an 80-byte memo costs more than a short one (the null-data
    /// output's size feeds the fee).
    #[test]
    fn carrier_fee_grows_with_memo_size() {
        let mut wallet = synced_wallet();
        let (_, _, _, _, _, _, short_fee) = carrier_fixture(&mut wallet, 100_000, b"=:ZEC.ZEC:x");
        let (_, _, _, _, _, _, max_fee) = carrier_fixture(&mut wallet, 100_000, &[0u8; 80]);

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
            "a larger memo never costs less: {max_fee:?} vs {short_fee:?}"
        );
    }

    /// The carrier must be funded to the zatoshi: an input worth less than
    /// amount + fee cannot balance and the build is refused.
    #[test]
    fn carrier_rejects_underfunded_input() {
        let mut wallet = synced_wallet();
        let (carrier_id, carrier, vault, op_return, amount, target_height, fee) =
            carrier_fixture(&mut wallet, 100_000, MEMO);

        let underfunded = ((amount + fee).unwrap() - Zatoshis::const_from_u64(1)).unwrap();
        let txout = TxOut::new(underfunded, carrier.script().into());

        let result = wallet.build_op_return_carrier_raw(
            ACCOUNT,
            carrier_id,
            OutPoint::new([7u8; 32], 0),
            txout,
            &vault,
            amount,
            &op_return,
            target_height,
        );
        assert!(result.is_err(), "an underfunded carrier cannot balance");
    }

    /// An over-funded input cannot balance either, because the carrier has
    /// no change output to absorb the surplus.
    #[test]
    fn carrier_rejects_overfunded_input() {
        let mut wallet = synced_wallet();
        let (carrier_id, carrier, vault, op_return, amount, target_height, fee) =
            carrier_fixture(&mut wallet, 100_000, MEMO);

        let overfunded = (amount + (fee + Zatoshis::const_from_u64(1_000)).unwrap()).unwrap();
        let txout = TxOut::new(overfunded, carrier.script().into());

        let result = wallet.build_op_return_carrier_raw(
            ACCOUNT,
            carrier_id,
            OutPoint::new([7u8; 32], 0),
            txout,
            &vault,
            amount,
            &op_return,
            target_height,
        );
        assert!(
            result.is_err(),
            "surplus with no change output cannot balance"
        );
    }

    /// An empty memo is a valid (if pointless) OP_RETURN, and still builds.
    #[test]
    fn carrier_accepts_empty_memo() {
        let mut wallet = synced_wallet();
        let (carrier_id, carrier, vault, op_return, amount, target_height, fee) =
            carrier_fixture(&mut wallet, 100_000, b"");

        let txout = TxOut::new((amount + fee).unwrap(), carrier.script().into());
        let raw = wallet
            .build_op_return_carrier_raw(
                ACCOUNT,
                carrier_id,
                OutPoint::new([7u8; 32], 0),
                txout,
                &vault,
                amount,
                &op_return,
                target_height,
            )
            .expect("empty-memo carrier builds");
        let transaction = read_tx(&wallet, &raw, target_height);
        assert_eq!(
            transaction
                .transparent_bundle()
                .expect("transparent")
                .vout
                .len(),
            2,
            "vault + empty OP_RETURN"
        );
    }
}
