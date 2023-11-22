use super::*;
use zcash_note_encryption::Domain;

impl LightClient {
    fn add_nonchange_notes<'a, 'b, 'c>(
        &'a self,
        transaction_metadata: &'b TransactionMetadata,
        unified_spend_auth: &'c crate::wallet::keys::unified::WalletCapability,
    ) -> impl Iterator<Item = JsonValue> + 'b
    where
        'a: 'b,
        'c: 'b,
    {
        self.add_wallet_notes_in_transaction_to_list_inner::<'a, 'b, 'c, zcash_primitives::sapling::note_encryption::SaplingDomain<zingoconfig::ChainType>>(
            transaction_metadata,
            unified_spend_auth,
        )
        .chain(
            self.add_wallet_notes_in_transaction_to_list_inner::<'a, 'b, 'c, orchard::note_encryption::OrchardDomain>(
                transaction_metadata,
                unified_spend_auth,
            ),
        )
    }

    fn add_wallet_notes_in_transaction_to_list_inner<'a, 'b, 'c, D>(
        &'a self,
        transaction_metadata: &'b TransactionMetadata,
        unified_spend_auth: &'c crate::wallet::keys::unified::WalletCapability,
    ) -> impl Iterator<Item = JsonValue> + 'b
    where
        'a: 'b,
        'c: 'b,
        D: crate::wallet::traits::DomainWalletExt,
        D::WalletNote: 'b,
        <D as Domain>::Recipient: crate::wallet::traits::Recipient,
        <D as Domain>::Note: PartialEq + Clone,
    {
        D::WalletNote::transaction_metadata_notes(transaction_metadata).iter().filter(|nd| !nd.is_change()).enumerate().map(|(i, nd)| {
                    let block_height: u32 = transaction_metadata.block_height.into();
                    object! {
                        "block_height" => block_height,
                        "unconfirmed"  => !transaction_metadata.status.is_confirmed(),
                        "datetime"     => transaction_metadata.datetime,
                        "position"     => i,
                        "txid"         => format!("{}", transaction_metadata.txid),
                        "amount"       => nd.value() as i64,
                        "zec_price"    => transaction_metadata.price.map(|p| (p * 100.0).round() / 100.0),
                        "address"      => LightWallet::note_address::<D>(&self.config.chain, nd, unified_spend_auth),
                        "memo"         => LightWallet::memo_str(nd.memo().clone())
                    }

                })
    }

    /// This fn is _only_ called insde a block conditioned on "is_outgoing_transaction"
    fn append_change_notes(
        wallet_transaction: &TransactionMetadata,
        received_utxo_value: u64,
    ) -> JsonValue {
        // TODO:  Understand why sapling and orchard have an "is_change" filter, but transparent does not
        // It seems like this already depends on an invariant where all outgoing utxos are change.
        // This should never be true _AFTER SOME VERSION_ since we only send change to orchard.
        // If I sent a transaction to a foreign transparent address wouldn't this "total_change" value
        // be wrong?
        let total_change = wallet_transaction
            .sapling_notes
            .iter()
            .filter(|nd| nd.is_change)
            .map(|nd| nd.note.value().inner())
            .sum::<u64>()
            + wallet_transaction
                .orchard_notes
                .iter()
                .filter(|nd| nd.is_change)
                .map(|nd| nd.note.value().inner())
                .sum::<u64>()
            + received_utxo_value;

        // Collect outgoing metadata
        let outgoing_json = wallet_transaction
            .outgoing_tx_data
            .iter()
            .map(|om| {
                object! {
                    // Is this address ever different than the address in the containing struct
                    // this is the full UA.
                    "address" => om.recipient_ua.clone().unwrap_or(om.to_address.clone()),
                    "value"   => om.value,
                    "memo"    => LightWallet::memo_str(Some(om.memo.clone()))
                }
            })
            .collect::<Vec<JsonValue>>();

        let block_height: u32 = wallet_transaction.block_height.into();
        object! {
            "block_height" => block_height,
            "unconfirmed"  => !wallet_transaction.status.is_confirmed(),
            "datetime"     => wallet_transaction.datetime,
            "txid"         => format!("{}", wallet_transaction.txid),
            "zec_price"    => wallet_transaction.price.map(|p| (p * 100.0).round() / 100.0),
            "amount"       => total_change as i64 - wallet_transaction.total_value_spent() as i64,
            "outgoing_metadata" => outgoing_json,
        }
    }
    pub async fn do_list_transactions(&self) -> JsonValue {
        // Create a list of TransactionItems from wallet transactions
        let mut consumer_ui_notes = self
            .wallet
            .transaction_context.transaction_metadata_set
            .read()
            .await
            .current
            .iter()
            .flat_map(|(txid, wallet_transaction)| {
                let mut consumer_notes_by_tx: Vec<JsonValue> = vec![];

                let total_transparent_received = wallet_transaction.transparent_notes.iter().map(|u| u.value).sum::<u64>();
                if wallet_transaction.is_outgoing_transaction() {
                    // If money was spent, create a consumer_ui_note. For this, we'll subtract
                    // all the change notes + Utxos
                    consumer_notes_by_tx.push(Self::append_change_notes(wallet_transaction, total_transparent_received));
                }

                // For each note that is not a change, add a consumer_ui_note.
                consumer_notes_by_tx.extend(self.add_nonchange_notes(wallet_transaction, &self.wallet.wallet_capability()));

                // TODO:  determine if all notes are either Change-or-NotChange, if that's the case
                // add a sanity check that asserts all notes are processed by this point

                // Get the total transparent value received in this transaction
                // Again we see the assumption that utxos are incoming.
                let net_transparent_value = total_transparent_received as i64 - wallet_transaction.total_transparent_value_spent as i64;
                let address = wallet_transaction.transparent_notes.iter().map(|utxo| utxo.address.clone()).collect::<Vec<String>>().join(",");
                if net_transparent_value > 0 {
                    if let Some(transaction) = consumer_notes_by_tx.iter_mut().find(|transaction| transaction["txid"] == txid.to_string()) {
                        // If this transaction is outgoing:
                        // Then we've already accounted for the entire balance.

                        if !wallet_transaction.is_outgoing_transaction() {
                            // If not, we've added sapling/orchard, and need to add transparent
                            let old_amount = transaction.remove("amount").as_i64().unwrap();
                            transaction.insert("amount", old_amount + net_transparent_value).unwrap();
                        }
                    } else {
                        // Create an input transaction for the transparent value as well.
                        let block_height: u32 = wallet_transaction.block_height.into();
                        consumer_notes_by_tx.push(object! {
                            "block_height" => block_height,
                            "unconfirmed"  => !wallet_transaction.status.is_confirmed(),
                            "datetime"     => wallet_transaction.datetime,
                            "txid"         => format!("{}", txid),
                            "amount"       => net_transparent_value,
                            "zec_price"    => wallet_transaction.price.map(|p| (p * 100.0).round() / 100.0),
                            "address"      => address,
                            "memo"         => None::<String>
                        })
                    }
                }

                consumer_notes_by_tx
            })
            .collect::<Vec<JsonValue>>();

        let match_by_txid =
            |a: &JsonValue, b: &JsonValue| a["txid"].to_string().cmp(&b["txid"].to_string());
        consumer_ui_notes.sort_by(match_by_txid);
        consumer_ui_notes.dedup_by(|a, b| {
            if match_by_txid(a, b) == cmp::Ordering::Equal {
                let val_b = b.remove("amount").as_i64().unwrap();
                b.insert(
                    "amount",
                    JsonValue::from(val_b + a.remove("amount").as_i64().unwrap()),
                )
                .unwrap();
                let memo_b = b.remove("memo").to_string();
                b.insert("memo", [a.remove("memo").to_string(), memo_b].join(", "))
                    .unwrap();
                for (key, a_val) in a.entries_mut() {
                    let b_val = b.remove(key);
                    if b_val == JsonValue::Null {
                        b.insert(key, a_val.clone()).unwrap();
                    } else {
                        if a_val != &b_val {
                            log::error!("{key}: {a_val} does not match {key}: {b_val}");
                        }
                        b.insert(key, b_val).unwrap()
                    }
                }

                true
            } else {
                false
            }
        });
        consumer_ui_notes.sort_by(|a, b| {
            if a["block_height"] == b["block_height"] {
                a["txid"].as_str().cmp(&b["txid"].as_str())
            } else {
                a["block_height"].as_i32().cmp(&b["block_height"].as_i32())
            }
        });

        JsonValue::Array(consumer_ui_notes)
    }
}
#[cfg(test)]
mod tests {
    use tokio::runtime::Runtime;
    use zingo_testutils::data::seeds::CHIMNEY_BETTER_SEED;
    use zingoconfig::{ChainType, ZingoConfig};

    use crate::{lightclient::LightClient, wallet::WalletBase};

    #[test]
    fn new_wallet_from_phrase() {
        let temp_dir = tempfile::Builder::new().prefix("test").tempdir().unwrap();
        let data_dir = temp_dir
            .into_path()
            .canonicalize()
            .expect("This path is available.");

        let wallet_name = data_dir.join("zingo-wallet.dat");
        let config = ZingoConfig::create_unconnected(ChainType::FakeMainnet, Some(data_dir));
        let lc = LightClient::create_from_wallet_base(
            WalletBase::MnemonicPhrase(CHIMNEY_BETTER_SEED.to_string()),
            &config,
            0,
            false,
        )
        .unwrap();
        assert_eq!(
        format!(
            "{:?}",
            LightClient::create_from_wallet_base(
                WalletBase::MnemonicPhrase(CHIMNEY_BETTER_SEED.to_string()),
                &config,
                0,
                false
            )
            .err()
            .unwrap()
        ),
        format!(
            "{:?}",
            std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                format!("Cannot create a new wallet from seed, because a wallet already exists at:\n{:?}", wallet_name),
            )
        )
    );

        // The first t address and z address should be derived
        Runtime::new().unwrap().block_on(async move {
            let addresses = lc.do_addresses().await;
            assert_eq!(
                "zs1q6xk3q783t5k92kjqt2rkuuww8pdw2euzy5rk6jytw97enx8fhpazdv3th4xe7vsk6e9sfpawfg"
                    .to_string(),
                addresses[0]["receivers"]["sapling"]
            );
            assert_eq!(
                "t1eQ63fwkQ4n4Eo5uCrPGaAV8FWB2tmx7ui",
                addresses[0]["receivers"]["transparent"]
            );
        });
    }
}
