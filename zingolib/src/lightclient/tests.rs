use bip0039::Mnemonic;
use ff::Field;
use json::JsonValue;
use rand::rngs::OsRng;
use tokio::runtime::Runtime;
use zcash_client_backend::address::RecipientAddress;

use orchard::keys::SpendingKey as OrchardSpendingKey;

use zcash_client_backend::encoding::encode_payment_address;
use zcash_note_encryption::EphemeralKeyBytes;
use zcash_primitives::consensus::{BlockHeight, BranchId, TestNetwork};
use zcash_primitives::memo::Memo;
use zcash_primitives::merkle_tree::IncrementalWitness;
use zcash_primitives::sapling::keys::DiversifiableFullViewingKey as SaplingFvk;
use zcash_primitives::sapling::note_encryption::sapling_note_encryption;
use zcash_primitives::sapling::{Note, Rseed};
use zcash_primitives::transaction::components::amount::DEFAULT_FEE;
use zcash_primitives::transaction::components::{OutputDescription, GROTH_PROOF_SIZE};
use zcash_primitives::transaction::Transaction;
use zcash_primitives::zip32::{ExtendedFullViewingKey, ExtendedSpendingKey as SaplingSpendingKey};

use crate::apply_scenario;
use crate::blaze::block_witness_data::CommitmentTreesForBlock;
use crate::lightclient::testmocks;

use crate::compact_formats::{CompactSaplingOutput, CompactTx};
use crate::lightclient::checkpoints;
use crate::lightclient::LightClient;
use crate::wallet::data::{ReceivedSaplingNoteAndMetadata, TransactionMetadata};
use crate::wallet::keys::extended_transparent::ExtendedPrivKey;
use crate::wallet::keys::unified::{
    get_transparent_secretkey_pubkey_taddr, Capability, WalletCapability,
};
use crate::wallet::traits::{ReadableWriteable, ReceivedNoteAndMetadata};
use crate::wallet::{LightWallet, WalletBase};

use zingoconfig::{ChainType, ZingoConfig};

apply_scenario! {multiple_incoming_same_transaction 10}
async fn multiple_incoming_same_transaction(scenario: NBlockFCBLScenario) {
    let NBlockFCBLScenario {
        data,
        lightclient,
        mut fake_compactblock_list,
        ..
    } = scenario;

    let fvk1: SaplingFvk = (&*lightclient.wallet.wallet_capability().read().await)
        .try_into()
        .unwrap();
    let value = 100_000;
    // 2. Construct the Fake transaction.
    let to = fvk1.default_address().1;

    // Create fake note for the account
    let td = testmocks::new_transactiondata();
    let mut compact_transaction = CompactTx::default();
    // Add 4 outputs
    for i in 0..4 {
        let mut rng = OsRng;
        let value = value + i;
        let note = Note {
            g_d: to.diversifier().g_d().unwrap(),
            pk_d: to.pk_d().clone(),
            value,
            rseed: Rseed::BeforeZip212(jubjub::Fr::random(rng)),
        };

        let encryptor = sapling_note_encryption::<_, TestNetwork>(
            None,
            note.clone(),
            to.clone(),
            Memo::default().into(),
            &mut rng,
        );

        let mut rng = OsRng;
        let rcv = jubjub::Fr::random(&mut rng);
        let cv = ValueCommitment {
            value,
            randomness: rcv.clone(),
        };

        let cmu = note.cmu();
        let _od = OutputDescription {
            cv: cv.commitment().into(),
            cmu: note.cmu(),
            ephemeral_key: EphemeralKeyBytes::from(encryptor.epk().to_bytes()),
            enc_ciphertext: encryptor.encrypt_note_plaintext(),
            out_ciphertext: encryptor.encrypt_outgoing_plaintext(
                &cv.commitment().into(),
                &cmu,
                &mut rng,
            ),
            zkproof: [0; GROTH_PROOF_SIZE],
        };

        let mut cmu = vec![];
        cmu.extend_from_slice(&note.cmu().to_repr());
        let mut epk = vec![];
        epk.extend_from_slice(&encryptor.epk().to_bytes());
        let enc_ciphertext = encryptor.encrypt_note_plaintext();

        // Create a fake CompactBlock containing the note
        let mut cout = CompactSaplingOutput::default();
        cout.cmu = cmu;
        cout.epk = epk;
        cout.ciphertext = enc_ciphertext[..52].to_vec();
        compact_transaction.outputs.push(cout);
    }

    let transaction = td.freeze().unwrap();
    compact_transaction.hash = transaction.txid().clone().as_ref().to_vec();

    // Add and mine the block
    fake_compactblock_list.transactions.push((
        transaction,
        fake_compactblock_list.next_height,
        vec![],
    ));
    fake_compactblock_list
        .add_empty_block()
        .add_transactions(vec![compact_transaction]);
    mine_pending_blocks(&mut fake_compactblock_list, &data, &lightclient).await;
    assert_eq!(lightclient.wallet.last_synced_height().await, 11);

    // 2. Check the notes - that we recieved 4 notes
    let notes = lightclient.do_list_notes(true).await;
    let transactions = lightclient.do_list_transactions(false).await;

    if let JsonValue::Array(mut unspent_notes) = notes["unspent_sapling_notes"].clone() {
        unspent_notes.sort_by_cached_key(|j| j["value"].as_u64().unwrap());

        for i in 0..4 {
            assert_eq!(unspent_notes[i]["created_in_block"].as_u64().unwrap(), 11);
            assert_eq!(
                unspent_notes[i]["value"].as_u64().unwrap(),
                value + i as u64
            );
            assert_eq!(unspent_notes[i]["is_change"].as_bool().unwrap(), false);
            assert_eq!(
                unspent_notes[i]["address"],
                lightclient
                    .wallet
                    .wallet_capability()
                    .read()
                    .await
                    .addresses()[0]
                    .encode(&lightclient.config.chain)
            );
        }
    } else {
        panic!("unspent notes not found");
    }

    assert_eq!(transactions.len(), 1);
    assert_eq!(transactions[0]["block_height"].as_u64().unwrap(), 11);
    assert_eq!(
        transactions[0]["address"],
        lightclient.do_addresses().await[0]["address"]
    );
    assert_eq!(
        transactions[0]["amount"].as_u64().unwrap(),
        // 4 values + 0 + 1 + 2 + 3
        value * 4 + 6
    );

    // 3. Send a big transaction, so all the value is spent
    let sent_value = value * 3 + u64::from(DEFAULT_FEE);
    mine_numblocks_each_with_two_sap_txs(&mut fake_compactblock_list, &data, &lightclient, 5).await; // make the funds spentable
    let sent_transaction_id = lightclient
        .test_do_send(vec![(EXT_ZADDR, sent_value, None)])
        .await
        .unwrap();

    // 4. Mine the sent transaction
    fake_compactblock_list.add_pending_sends(&data).await;
    mine_pending_blocks(&mut fake_compactblock_list, &data, &lightclient).await;

    // 5. Check the notes - that we spent all 4 notes
    let notes = lightclient.do_list_notes(true).await;
    let transactions = lightclient.do_list_transactions(false).await;
    for i in 0..4 {
        assert_eq!(
            notes["spent_sapling_notes"][i]["spent"],
            sent_transaction_id
        );
        assert_eq!(
            notes["spent_sapling_notes"][i]["spent_at_height"]
                .as_u64()
                .unwrap(),
            17
        );
    }
    assert_eq!(transactions[1]["txid"], sent_transaction_id);
    assert_eq!(transactions[1]["block_height"], 17 as u32);
    assert_eq!(
        transactions[1]["amount"].as_i64().unwrap(),
        -(sent_value as i64) - i64::from(DEFAULT_FEE)
    );
    assert_eq!(
        transactions[1]["outgoing_metadata"][0]["address"],
        EXT_ZADDR.to_string()
    );
    assert_eq!(
        transactions[1]["outgoing_metadata"][0]["value"]
            .as_u64()
            .unwrap(),
        sent_value
    );
    assert_eq!(
        transactions[1]["outgoing_metadata"][0]["memo"].is_null(),
        true
    );
}

apply_scenario! {sapling_incoming_multisapling_outgoing 10}
async fn sapling_incoming_multisapling_outgoing(scenario: NBlockFCBLScenario) {
    let NBlockFCBLScenario {
        data,
        lightclient,
        mut fake_compactblock_list,
        ..
    } = scenario;
    // 2. Send an incoming transaction to fill the wallet
    let fvk1 = (&*lightclient.wallet.wallet_capability().read().await)
        .try_into()
        .unwrap();
    let value = 100_000;
    let (_transaction, _height, _) =
        fake_compactblock_list.create_sapling_coinbase_transaction(&fvk1, value);
    mine_pending_blocks(&mut fake_compactblock_list, &data, &lightclient).await;
    mine_numblocks_each_with_two_sap_txs(&mut fake_compactblock_list, &data, &lightclient, 5).await;

    // 3. send a transaction to multiple addresses
    let tos = vec![
        (EXT_ZADDR, 1, Some("ext1-1".to_string())),
        (EXT_ZADDR, 2, Some("ext1-2".to_string())),
        (EXT_ZADDR2, 20, Some("ext2-20".to_string())),
    ];
    let sent_transaction_id = lightclient.test_do_send(tos.clone()).await.unwrap();
    fake_compactblock_list.add_pending_sends(&data).await;
    mine_pending_blocks(&mut fake_compactblock_list, &data, &lightclient).await;

    // 4. Check the outgoing transaction list
    let list = lightclient.do_list_transactions(false).await;

    assert_eq!(list[1]["block_height"].as_u64().unwrap(), 17);
    assert_eq!(list[1]["txid"], sent_transaction_id);
    assert_eq!(
        list[1]["amount"].as_i64().unwrap(),
        -i64::from(DEFAULT_FEE) - (tos.iter().map(|(_, a, _)| *a).sum::<u64>() as i64)
    );

    for (addr, amt, memo) in &tos {
        // Find the correct value, since the outgoing metadata can be shuffled
        let jv = list[1]["outgoing_metadata"]
            .members()
            .find(|j| j["value"].as_u64().unwrap() == *amt)
            .unwrap();
        assert_eq!(jv["memo"], *memo.as_ref().unwrap());
        assert_eq!(jv["address"], addr.to_string());
    }
}

/*
apply_scenario! {sapling_incoming_viewkey 10}
async fn sapling_incoming_viewkey(scenario: NBlockFCBLScenario) {
    let NBlockFCBLScenario {
        data,
        config,
        lightclient,
        mut fake_compactblock_list,
        ..
    } = scenario;
    assert_eq!(
        lightclient.do_balance().await["sapling_balance"]
            .as_u64()
            .unwrap(),
        0
    );

    // 2. Create a new Viewkey and import it
    let iextsk = SaplingSpendingKey::master(&[1u8; 32]);
    let ifvk = SaplingFvk::from(&iextsk);
    let iaddr = encode_payment_address(config.hrp_sapling_address(), &ifvk.default_address().1);
    let addrs = lightclient
        .do_import_sapling_full_view_key(
            encode_extended_full_viewing_key(config.hrp_sapling_viewing_key(), &ifvk),
            1,
        )
        .await
        .unwrap();
    // Make sure address is correct
    assert_eq!(addrs[0], iaddr);

    let value = 100_000;
    let (transaction, _height, _) =
        fake_compactblock_list.create_sapling_coinbase_transaction(&ifvk, value);
    let txid = transaction.txid();
    mine_pending_blocks(&mut fake_compactblock_list, &data, &lightclient).await;
    mine_numblocks_each_with_two_sap_txs(&mut fake_compactblock_list, &data, &lightclient, 5).await;

    // 3. Test that we have the transaction
    let list = lightclient.do_list_transactions(false).await;
    assert_eq!(
        lightclient.do_balance().await["sapling_balance"]
            .as_u64()
            .unwrap(),
        value
    );

    log::debug!("{}", lightclient.do_balance().await);
    assert!(!lightclient
        .wallet
        .keys()
        .read()
        .await
        .have_sapling_spending_key(&ifvk));
    assert_eq!(
        lightclient.do_balance().await["spendable_sapling_balance"]
            .as_u64()
            .unwrap(),
        0
    );
    assert_eq!(list[0]["txid"], txid.to_string());
    assert_eq!(list[0]["amount"].as_u64().unwrap(), value);
    assert_eq!(list[0]["address"], iaddr);

    // 4. Also do a rescan, just for fun
    mine_numblocks_each_with_two_sap_txs(&mut fake_compactblock_list, &data, &lightclient, 10)
        .await;
    lightclient.do_rescan().await.unwrap();
    // Test all the same values
    let list = lightclient.do_list_transactions(false).await;
    assert_eq!(
        lightclient.do_balance().await["sapling_balance"]
            .as_u64()
            .unwrap(),
        value
    );
    assert_eq!(
        lightclient.do_balance().await["spendable_sapling_balance"]
            .as_u64()
            .unwrap(),
        0
    );
    assert_eq!(list[0]["txid"], txid.to_string());
    assert_eq!(list[0]["amount"].as_u64().unwrap(), value);
    assert_eq!(list[0]["address"], iaddr);

    // 5. Import the corresponding spending key.
    let sk_addr = lightclient
        .do_import_sapling_spend_key(
            encode_extended_spending_key(config.hrp_sapling_private_key(), &iextsk),
            1,
        )
        .await
        .unwrap();

    assert_eq!(sk_addr[0], iaddr);
    assert_eq!(
        lightclient.do_balance().await["sapling_balance"]
            .as_u64()
            .unwrap(),
        value
    );
    assert_eq!(
        lightclient.do_balance().await["spendable_sapling_balance"]
            .as_u64()
            .unwrap(),
        0
    );

    // 6. Rescan to make the funds spendable (i.e., update witnesses)
    lightclient.do_rescan().await.unwrap();
    assert_eq!(
        lightclient.do_balance().await["sapling_balance"]
            .as_u64()
            .unwrap(),
        value
    );
    assert_eq!(
        lightclient.do_balance().await["spendable_sapling_balance"]
            .as_u64()
            .unwrap(),
        value
    );

    // 7. Spend funds from the now-imported private key.
    let sent_value = 3000;
    let outgoing_memo = "Outgoing Memo".to_string();

    let sent_transaction_id = lightclient
        .test_do_send(vec![(EXT_ZADDR, sent_value, Some(outgoing_memo.clone()))])
        .await
        .unwrap();
    fake_compactblock_list.add_pending_sends(&data).await;
    mine_pending_blocks(&mut fake_compactblock_list, &data, &lightclient).await;

    // 8. Make sure transaction is present
    let list = lightclient.do_list_transactions(false).await;
    assert_eq!(list[1]["txid"], sent_transaction_id);
    assert_eq!(
        list[1]["amount"].as_i64().unwrap(),
        -((sent_value + u64::from(DEFAULT_FEE)) as i64)
    );
    assert_eq!(
        list[1]["outgoing_metadata"][0]["address"],
        EXT_ZADDR.to_string()
    );
    assert_eq!(
        list[1]["outgoing_metadata"][0]["value"].as_u64().unwrap(),
        sent_value
    );
}*/

apply_scenario! {aborted_resync 10}
async fn aborted_resync(scenario: NBlockFCBLScenario) {
    let NBlockFCBLScenario {
        data,
        lightclient,
        mut fake_compactblock_list,
        ..
    } = scenario;
    // 2. Send an incoming transaction to fill the wallet
    let fvk1 = (&*lightclient.wallet.wallet_capability().read().await)
        .try_into()
        .unwrap();
    let zvalue = 100_000;
    let (_ztransaction, _height, _) =
        fake_compactblock_list.create_sapling_coinbase_transaction(&fvk1, zvalue);
    mine_pending_blocks(&mut fake_compactblock_list, &data, &lightclient).await;
    mine_numblocks_each_with_two_sap_txs(&mut fake_compactblock_list, &data, &lightclient, 5).await;

    // 3. Send an incoming t-address transaction
    let (_sk, Some(pk), Some(taddr)) = get_transparent_secretkey_pubkey_taddr(&lightclient).await else { panic!() };
    let tvalue = 200_000;

    let mut fake_transaction = FakeTransaction::new(true);
    fake_transaction.add_t_output(&pk, taddr.clone(), tvalue);
    let (_ttransaction, _) = fake_compactblock_list.add_fake_transaction(fake_transaction);
    mine_pending_blocks(&mut fake_compactblock_list, &data, &lightclient).await;

    // 4. Send a transaction to both external t-addr and external z addr and mine it
    let sent_zvalue = 80_000;
    let sent_tvalue = 140_000;
    let sent_zmemo = "Ext z".to_string();
    let tos = vec![
        (EXT_ZADDR, sent_zvalue, Some(sent_zmemo.clone())),
        (EXT_TADDR, sent_tvalue, None),
    ];
    let sent_transaction_id = lightclient.test_do_send(tos).await.unwrap();

    fake_compactblock_list.add_pending_sends(&data).await;
    mine_pending_blocks(&mut fake_compactblock_list, &data, &lightclient).await;
    mine_numblocks_each_with_two_sap_txs(&mut fake_compactblock_list, &data, &lightclient, 5).await;

    let notes_before = lightclient.do_list_notes(true).await;
    let list_before = lightclient.do_list_transactions(false).await;
    let witness_before = lightclient
        .wallet
        .transaction_context
        .transaction_metadata_set
        .read()
        .await
        .current
        .get(&TransactionMetadata::new_txid(
            &hex::decode(sent_transaction_id.clone())
                .unwrap()
                .into_iter()
                .rev()
                .collect(),
        ))
        .unwrap()
        .orchard_notes
        .get(0)
        .unwrap()
        .witnesses
        .clone();

    // 5. Now, we'll manually remove some of the blocks in the wallet, pretending that the sync was aborted in the middle.
    // We'll remove the top 20 blocks, so now the wallet only has the first 3 blocks
    lightclient.wallet.blocks.write().await.drain(0..20);
    assert_eq!(lightclient.wallet.last_synced_height().await, 3);

    // 6. Do a sync again
    lightclient.do_sync(true).await.unwrap();
    assert_eq!(lightclient.wallet.last_synced_height().await, 23);

    // 7. Should be exactly the same
    let notes_after = lightclient.do_list_notes(true).await;
    let list_after = lightclient.do_list_transactions(false).await;
    let witness_after = lightclient
        .wallet
        .transaction_context
        .transaction_metadata_set
        .read()
        .await
        .current
        .get(&TransactionMetadata::new_txid(
            &hex::decode(sent_transaction_id)
                .unwrap()
                .into_iter()
                .rev()
                .collect(),
        ))
        .unwrap()
        .orchard_notes
        .get(0)
        .unwrap()
        .witnesses
        .clone();

    assert_eq!(notes_before, notes_after);
    assert_eq!(list_before, list_after);
    assert_eq!(witness_before.top_height, witness_after.top_height);
    assert_eq!(witness_before.len(), witness_after.len());
    for i in 0..witness_before.len() {
        let mut before_bytes = vec![];
        witness_before
            .get(i)
            .unwrap()
            .write(&mut before_bytes)
            .unwrap();

        let mut after_bytes = vec![];
        witness_after
            .get(i)
            .unwrap()
            .write(&mut after_bytes)
            .unwrap();

        assert_eq!(hex::encode(before_bytes), hex::encode(after_bytes));
    }
}

apply_scenario! {no_change 10}
async fn no_change(scenario: NBlockFCBLScenario) {
    let NBlockFCBLScenario {
        data,
        lightclient,
        mut fake_compactblock_list,
        ..
    } = scenario;
    // 2. Send an incoming transaction to fill the wallet
    let fvk1 = (&*lightclient.wallet.wallet_capability().read().await)
        .try_into()
        .unwrap();
    let zvalue = 100_000;
    let (_ztransaction, _height, _) =
        fake_compactblock_list.create_sapling_coinbase_transaction(&fvk1, zvalue);
    mine_pending_blocks(&mut fake_compactblock_list, &data, &lightclient).await;
    mine_numblocks_each_with_two_sap_txs(&mut fake_compactblock_list, &data, &lightclient, 5).await;

    // 3. Send an incoming t-address transaction
    let (_sk, Some(pk), Some(taddr)) = get_transparent_secretkey_pubkey_taddr(&lightclient).await else { panic!() };
    let tvalue = 200_000;

    let mut fake_transaction = FakeTransaction::new(true);
    fake_transaction.add_t_output(&pk, taddr.clone(), tvalue);
    let (_t_transaction, _) = fake_compactblock_list.add_fake_transaction(fake_transaction);
    mine_pending_blocks(&mut fake_compactblock_list, &data, &lightclient).await;

    // 4. Send a transaction to both external t-addr and external z addr and mine it
    let sent_zvalue = tvalue + zvalue - u64::from(DEFAULT_FEE);
    let tos = vec![(EXT_ZADDR, sent_zvalue, None)];
    let sent_transaction_id = lightclient.test_do_send(tos).await.unwrap();

    fake_compactblock_list.add_pending_sends(&data).await;
    mine_pending_blocks(&mut fake_compactblock_list, &data, &lightclient).await;

    let notes = lightclient.do_list_notes(true).await;
    assert_eq!(notes["unspent_sapling_notes"].len(), 0);
    assert_eq!(notes["pending_sapling_notes"].len(), 0);
    assert_eq!(notes["utxos"].len(), 0);
    assert_eq!(notes["pending_utxos"].len(), 0);

    assert_eq!(notes["spent_sapling_notes"].len(), 1);
    assert_eq!(notes["spent_utxos"].len(), 1);
    assert_eq!(
        notes["spent_sapling_notes"][0]["spent"],
        sent_transaction_id
    );
    assert_eq!(notes["spent_utxos"][0]["spent"], sent_transaction_id);
}

#[tokio::test]
async fn recover_at_checkpoint() {
    // 1. Wait for test server to start
    let (data, config, ready_receiver, stop_transmitter, h1) = create_test_server().await;
    ready_receiver.await.unwrap();

    // Get checkpoint at 1220000
    let (ckpt_height, hash, tree) = checkpoints::get_all_main_checkpoints()
        .into_iter()
        .find(|(h, _, _)| *h == 1220000)
        .unwrap();
    // Manually insert the checkpoint at -100, so the test server can return it.
    data.write()
        .await
        .tree_states
        .push(CommitmentTreesForBlock::from_pre_orchard_checkpoint(
            ckpt_height,
            hash.to_string(),
            tree.to_string(),
        ));

    // 2. Mine 110 blocks after 1220000
    let mut fcbl = FakeCompactBlockList::new(0);
    fcbl.next_height = ckpt_height + 1;
    {
        let blk = fcbl.add_empty_block();
        blk.block.prev_hash = hex::decode(hash).unwrap().into_iter().rev().collect();
    }
    let cbs = fcbl
        .create_and_append_randtx_blocks(109)
        .into_compact_blocks();
    data.write().await.add_blocks(cbs.clone());

    // 4. Test1: create a new lightclient, restoring at exactly the checkpoint
    let lc = LightClient::test_new(
        &config,
        WalletBase::MnemonicPhrase(TEST_SEED.to_string()),
        ckpt_height,
    )
    .await
    .unwrap();
    //lc.init_logging().unwrap();
    assert_eq!(
        json::parse(lc.do_info().await.as_str()).unwrap()["latest_block_height"]
            .as_u64()
            .unwrap(),
        ckpt_height + 110
    );

    lc.do_sync(true).await.unwrap();

    // Check the trees
    assert_eq!(
        lc.wallet
            .blocks
            .read()
            .await
            .first()
            .map(|b| b.clone())
            .unwrap()
            .height,
        1220110
    );

    // 5: Test2: Delete the old wallet
    //           Create a new wallet, restoring at checkpoint + 100

    let wallet_name = &format!("{}/zingo-wallet.dat", config.data_dir.clone().unwrap());
    let _wallet_remove = std::process::Command::new("rm")
        .args(["-f", wallet_name])
        .output()
        .expect("Wallet should always be removed.");
    let lc = LightClient::test_new(
        &config,
        WalletBase::MnemonicPhrase(TEST_SEED.to_string()),
        ckpt_height + 100,
    )
    .await
    .unwrap();

    assert_eq!(
        json::parse(lc.do_info().await.as_str()).unwrap()["latest_block_height"]
            .as_u64()
            .unwrap(),
        ckpt_height + 110
    );

    lc.do_sync(true).await.unwrap();

    // Check the trees
    assert_eq!(
        lc.wallet
            .blocks
            .read()
            .await
            .first()
            .map(|b| b.clone())
            .unwrap()
            .height,
        1220110
    );
    // assert_eq!(
    //     tree_to_string(
    //         &lc.wallet
    //             .blocks
    //             .read()
    //             .await
    //             .first()
    //             .map(|b| b.clone())
    //             .unwrap()
    //             .tree()
    //             .unwrap()
    //     ),
    //     tree_to_string(&tree)
    // );

    // Shutdown everything cleanly
    clean_shutdown(stop_transmitter, h1).await;
}

apply_scenario! {witness_clearing 10}

#[ignore]
#[test]
fn test_read_wallet_from_buffer() {
    //Block_on needed because read_from_buffer starts a tokio::Runtime, which panics when called in async code
    //as you cannot create a Runtime inside a Runtime
    let mut buf = Vec::new();
    let config = ZingoConfig::create_unconnected(ChainType::FakeMainnet, None);
    Runtime::new().unwrap().block_on(async {
        let wallet =
            crate::wallet::LightWallet::new(config.clone(), WalletBase::FreshEntropy, 0).unwrap();
        wallet.write(&mut buf).await.unwrap();
    });
    let client = LightClient::read_wallet_from_buffer(&config, &buf[..]).unwrap();
    Runtime::new().unwrap().block_on(async {
        let _wallet = client.wallet;
        todo!("Make meaningfull assertions here")
    });
}

apply_scenario! {read_write_block_data 10}
async fn read_write_block_data(scenario: NBlockFCBLScenario) {
    let NBlockFCBLScenario {
        mut fake_compactblock_list,
        ..
    } = scenario;
    for block in fake_compactblock_list.blocks.drain(..) {
        let block_bytes: &mut [u8] = &mut [];
        let cb = crate::wallet::data::BlockData::new(block.block);
        cb.write(&mut *block_bytes).unwrap();
        assert_eq!(
            cb,
            crate::wallet::data::BlockData::read(&*block_bytes).unwrap()
        );
    }
}

pub const EXT_ZADDR: &str =
    "zs1va5902apnzlhdu0pw9r9q7ca8s4vnsrp2alr6xndt69jnepn2v2qrj9vg3wfcnjyks5pg65g9dc";
pub const EXT_ZADDR2: &str =
    "zs1fxgluwznkzm52ux7jkf4st5znwzqay8zyz4cydnyegt2rh9uhr9458z0nk62fdsssx0cqhy6lyv";
pub const TEST_SEED: &str = "chimney better bulb horror rebuild whisper improve intact letter giraffe brave rib appear bulk aim burst snap salt hill sad merge tennis phrase raise";
