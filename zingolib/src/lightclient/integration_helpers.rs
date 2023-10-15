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
                        "unconfirmed" => transaction_metadata.unconfirmed,
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
            "unconfirmed" => wallet_transaction.unconfirmed,
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

                let total_transparent_received = wallet_transaction.received_utxos.iter().map(|u| u.value).sum::<u64>();
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
                let address = wallet_transaction.received_utxos.iter().map(|utxo| utxo.address.clone()).collect::<Vec<String>>().join(",");
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
                            "unconfirmed"  => wallet_transaction.unconfirmed,
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
    async fn list_sapling_notes(
        &self,
        all_notes: bool,
        anchor_height: BlockHeight,
    ) -> (Vec<JsonValue>, Vec<JsonValue>, Vec<JsonValue>) {
        let mut unspent_sapling_notes: Vec<JsonValue> = vec![];
        let mut spent_sapling_notes: Vec<JsonValue> = vec![];
        let mut pending_sapling_notes: Vec<JsonValue> = vec![];
        // Collect Sapling notes
        self.wallet.transaction_context.transaction_metadata_set.read().await.current.iter()
                .flat_map( |(transaction_id, transaction_metadata)| {
                    transaction_metadata.sapling_notes.iter().filter_map(move |note_metadata|
                        if !all_notes && note_metadata.spent.is_some() {
                            None
                        } else {
                            let address = LightWallet::note_address::<zcash_primitives::sapling::note_encryption::SaplingDomain<zingoconfig::ChainType>>(&self.config.chain, note_metadata, &self.wallet.wallet_capability());
                            let spendable = transaction_metadata.block_height <= anchor_height && note_metadata.spent.is_none() && note_metadata.unconfirmed_spent.is_none();

                            let created_block:u32 = transaction_metadata.block_height.into();
                            Some(object!{
                                "created_in_block"   => created_block,
                                "datetime"           => transaction_metadata.datetime,
                                "created_in_txid"    => format!("{}", transaction_id.clone()),
                                "value"              => note_metadata.note.value().inner(),
                                "unconfirmed"        => transaction_metadata.unconfirmed,
                                "is_change"          => note_metadata.is_change,
                                "address"            => address,
                                "spendable"          => spendable,
                                "spent"              => note_metadata.spent.map(|(spent_transaction_id, _)| format!("{}", spent_transaction_id)),
                                "spent_at_height"    => note_metadata.spent.map(|(_, h)| h),
                                "unconfirmed_spent"  => note_metadata.unconfirmed_spent.map(|(spent_transaction_id, _)| format!("{}", spent_transaction_id)),
                            })
                        }
                    )
                })
                .for_each( |note| {
                    if note["spent"].is_null() && note["unconfirmed_spent"].is_null() {
                        unspent_sapling_notes.push(note);
                    } else if !note["spent"].is_null() {
                        spent_sapling_notes.push(note);
                    } else {
                        pending_sapling_notes.push(note);
                    }
                });
        (
            unspent_sapling_notes,
            spent_sapling_notes,
            pending_sapling_notes,
        )
    }
    async fn list_orchard_notes(
        &self,
        all_notes: bool,
        anchor_height: BlockHeight,
    ) -> (Vec<JsonValue>, Vec<JsonValue>, Vec<JsonValue>) {
        let mut unspent_orchard_notes: Vec<JsonValue> = vec![];
        let mut spent_orchard_notes: Vec<JsonValue> = vec![];
        let mut pending_orchard_notes: Vec<JsonValue> = vec![];
        self.wallet.transaction_context.transaction_metadata_set.read().await.current.iter()
                .flat_map( |(transaction_id, transaction_metadata)| {
                    transaction_metadata.orchard_notes.iter().filter_map(move |orch_note_metadata|
                        if !all_notes && orch_note_metadata.is_spent() {
                            None
                        } else {
                            let address = LightWallet::note_address::<orchard::note_encryption::OrchardDomain>(&self.config.chain, orch_note_metadata, &self.wallet.wallet_capability());
                            let spendable = transaction_metadata.block_height <= anchor_height && orch_note_metadata.spent.is_none() && orch_note_metadata.unconfirmed_spent.is_none();

                            let created_block:u32 = transaction_metadata.block_height.into();
                            Some(object!{
                                "created_in_block"   => created_block,
                                "datetime"           => transaction_metadata.datetime,
                                "created_in_txid"    => format!("{}", transaction_id),
                                "value"              => orch_note_metadata.note.value().inner(),
                                "unconfirmed"        => transaction_metadata.unconfirmed,
                                "is_change"          => orch_note_metadata.is_change,
                                "address"            => address,
                                "spendable"          => spendable,
                                "spent"              => orch_note_metadata.spent.map(|(spent_transaction_id, _)| format!("{}", spent_transaction_id)),
                                "spent_at_height"    => orch_note_metadata.spent.map(|(_, h)| h),
                                "unconfirmed_spent"  => orch_note_metadata.unconfirmed_spent.map(|(spent_transaction_id, _)| format!("{}", spent_transaction_id)),
                            })
                        }
                    )
                })
                .for_each( |note| {
                    if note["spent"].is_null() && note["unconfirmed_spent"].is_null() {
                        unspent_orchard_notes.push(note);
                    } else if !note["spent"].is_null() {
                        spent_orchard_notes.push(note);
                    } else {
                        pending_orchard_notes.push(note);
                    }
                });
        (
            unspent_orchard_notes,
            spent_orchard_notes,
            pending_orchard_notes,
        )
    }
    async fn list_transparent_notes(
        &self,
        all_notes: bool,
    ) -> (Vec<JsonValue>, Vec<JsonValue>, Vec<JsonValue>) {
        let mut unspent_transparent_notes: Vec<JsonValue> = vec![];
        let mut spent_transparent_notes: Vec<JsonValue> = vec![];
        let mut pending_transparent_notes: Vec<JsonValue> = vec![];

        self.wallet.transaction_context.transaction_metadata_set.read().await.current.iter()
                .flat_map( |(transaction_id, wtx)| {
                    wtx.received_utxos.iter().filter_map(move |utxo|
                        if !all_notes && utxo.spent.is_some() {
                            None
                        } else {
                            let created_block:u32 = wtx.block_height.into();
                            let recipient = zcash_client_backend::address::RecipientAddress::decode(&self.config.chain, &utxo.address);
                            let taddr = match recipient {
                            Some(zcash_client_backend::address::RecipientAddress::Transparent(taddr)) => taddr,
                                _otherwise => panic!("Read invalid taddr from wallet-local Utxo, this should be impossible"),
                            };

                            Some(object!{
                                "created_in_block"   => created_block,
                                "datetime"           => wtx.datetime,
                                "created_in_txid"    => format!("{}", transaction_id),
                                "value"              => utxo.value,
                                "scriptkey"          => hex::encode(utxo.script.clone()),
                                "is_change"          => false, // TODO: Identify notes as change if we send change to our own taddrs
                                "address"            => self.wallet.wallet_capability().get_ua_from_contained_transparent_receiver(&taddr).map(|ua| ua.encode(&self.config.chain)),
                                "spent_at_height"    => utxo.spent_at_height,
                                "spent"              => utxo.spent.map(|spent_transaction_id| format!("{}", spent_transaction_id)),
                                "unconfirmed_spent"  => utxo.unconfirmed_spent.map(|(spent_transaction_id, _)| format!("{}", spent_transaction_id)),
                            })
                        }
                    )
                })
                .for_each( |utxo| {
                    if utxo["spent"].is_null() && utxo["unconfirmed_spent"].is_null() {
                        unspent_transparent_notes.push(utxo);
                    } else if !utxo["spent"].is_null() {
                        spent_transparent_notes.push(utxo);
                    } else {
                        pending_transparent_notes.push(utxo);
                    }
                });

        (
            unspent_transparent_notes,
            spent_transparent_notes,
            pending_transparent_notes,
        )
    }

    /// Return a list of notes, if `all_notes` is false, then only return unspent notes
    ///  * TODO:  This fn does not handle failure it must be promoted to return a Result
    ///  * TODO:  The Err variant of the result must be a proper type
    ///  * TODO:  remove all_notes bool
    ///  * TODO:   This fn must (on success) return an Ok(Vec\<Notes\>) where Notes is a 3 variant enum....
    ///  * TODO:   type-associated to the variants of the enum must impl From\<Type\> for JsonValue
    pub async fn do_list_notes(&self, all_notes: bool) -> JsonValue {
        let anchor_height = BlockHeight::from_u32(self.wallet.get_anchor_height().await);

        let (mut unspent_sapling_notes, mut spent_sapling_notes, mut pending_sapling_notes) =
            self.list_sapling_notes(all_notes, anchor_height).await;
        let (mut unspent_orchard_notes, mut spent_orchard_notes, mut pending_orchard_notes) =
            self.list_orchard_notes(all_notes, anchor_height).await;
        let (
            mut unspent_transparent_notes,
            mut spent_transparent_notes,
            mut pending_transparent_notes,
        ) = self.list_transparent_notes(all_notes).await;

        unspent_sapling_notes.sort_by_key(|note| note["created_in_block"].as_u64());
        spent_sapling_notes.sort_by_key(|note| note["created_in_block"].as_u64());
        pending_sapling_notes.sort_by_key(|note| note["created_in_block"].as_u64());
        unspent_orchard_notes.sort_by_key(|note| note["created_in_block"].as_u64());
        spent_orchard_notes.sort_by_key(|note| note["created_in_block"].as_u64());
        pending_orchard_notes.sort_by_key(|note| note["created_in_block"].as_u64());
        unspent_transparent_notes.sort_by_key(|note| note["created_in_block"].as_u64());
        pending_transparent_notes.sort_by_key(|note| note["created_in_block"].as_u64());
        spent_transparent_notes.sort_by_key(|note| note["created_in_block"].as_u64());

        let mut res = object! {
            "unspent_sapling_notes" => unspent_sapling_notes,
            "pending_sapling_notes" => pending_sapling_notes,
            "unspent_orchard_notes" => unspent_orchard_notes,
            "pending_orchard_notes" => pending_orchard_notes,
            "utxos"         => unspent_transparent_notes,
            "pending_utxos" => pending_transparent_notes,
        };

        if all_notes {
            res["spent_sapling_notes"] = JsonValue::Array(spent_sapling_notes);
            res["spent_orchard_notes"] = JsonValue::Array(spent_orchard_notes);
            res["spent_utxos"] = JsonValue::Array(spent_transparent_notes);
        }

        res
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
