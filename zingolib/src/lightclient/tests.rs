use bip0039::Mnemonic;
use ff::{Field, PrimeField};
use group::GroupEncoding;
use json::JsonValue;
use rand::rngs::OsRng;
use tokio::runtime::Runtime;
use zcash_client_backend::address::RecipientAddress;

use orchard::keys::{FullViewingKey as OrchardFvk, SpendingKey as OrchardSpendingKey};

use zcash_address::unified::{Address as UAddress, Encoding, Fvk, Receiver, Ufvk};
use zcash_client_backend::encoding::encode_payment_address;
use zcash_note_encryption::EphemeralKeyBytes;
use zcash_primitives::consensus::{BlockHeight, BranchId, Parameters, TestNetwork};
use zcash_primitives::memo::Memo;
use zcash_primitives::merkle_tree::IncrementalWitness;
use zcash_primitives::sapling::keys::DiversifiableFullViewingKey as SaplingFvk;
use zcash_primitives::sapling::note_encryption::sapling_note_encryption;
use zcash_primitives::sapling::{Note, Rseed, ValueCommitment};
use zcash_primitives::transaction::components::amount::DEFAULT_FEE;
use zcash_primitives::transaction::components::{OutputDescription, GROTH_PROOF_SIZE};
use zcash_primitives::transaction::Transaction;
use zcash_primitives::zip32::{ExtendedFullViewingKey, ExtendedSpendingKey as SaplingSpendingKey};

use crate::apply_scenario;
use crate::blaze::block_witness_data::CommitmentTreesForBlock;
use crate::blaze::test_utils::{FakeCompactBlockList, FakeTransaction};
use crate::lightclient::testmocks;

use crate::compact_formats::{CompactSaplingOutput, CompactTx};
use crate::lightclient::checkpoints;
use crate::lightclient::test_server::{
    clean_shutdown, create_test_server, mine_numblocks_each_with_two_sap_txs, mine_pending_blocks,
    NBlockFCBLScenario,
};
use crate::lightclient::LightClient;
use crate::wallet::data::{ReceivedSaplingNoteAndMetadata, TransactionMetadata};
use crate::wallet::keys::extended_transparent::{ExtendedPrivKey, ExtendedPubKey};
use crate::wallet::keys::unified::ReceiverSelection;
use crate::wallet::keys::{
    address_from_pubkeyhash,
    unified::{get_transparent_secretkey_pubkey_taddr, Capability, WalletCapability},
};
use crate::wallet::traits::{ReadableWriteable, ReceivedNoteAndMetadata};
use crate::wallet::{LightWallet, WalletBase};

use zingoconfig::{ChainType, ZingoConfig};

#[test]
fn new_wallet_from_phrase() {
    let temp_dir = tempfile::Builder::new().prefix("test").tempdir().unwrap();
    let data_dir = temp_dir
        .into_path()
        .canonicalize()
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();

    let config = ZingoConfig::create_unconnected(ChainType::FakeMainnet, Some(data_dir));
    let lc = LightClient::new_from_wallet_base(
        WalletBase::MnemonicPhrase(TEST_SEED.to_string()),
        &config,
        0,
        false,
    )
    .unwrap();
    assert_eq!(
        format!(
            "{:?}",
            LightClient::new_from_wallet_base(
                WalletBase::MnemonicPhrase(TEST_SEED.to_string()),
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
                format!("Cannot create a new wallet from seed, because a wallet already exists"),
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

#[test]
#[ignore]
fn new_wallet_from_sapling_esk() {
    let temp_dir = tempfile::Builder::new().prefix("test").tempdir().unwrap();
    let data_dir = temp_dir
        .into_path()
        .canonicalize()
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();

    let config = ZingoConfig::create_unconnected(ChainType::FakeMainnet, Some(data_dir));
    let sk = "secret-extended-key-main1qvpa0qr8qqqqpqxn4l054nzxpxzp3a8r2djc7sekdek5upce8mc2j2z0arzps4zv940qeg706hd0wq6g5snzvhp332y6vhwyukdn8dhekmmsk7fzvzkqm6ypc99uy63tpesqwxhpre78v06cx8k5xpp9mrhtgqs5dvp68cqx2yrvthflmm2ynl8c0506dekul0f6jkcdmh0292lpphrksyc5z3pxwws97zd5els3l2mjt2s7hntap27mlmt6w0drtfmz36vz8pgu7ec0twfrq";
    // This will panic.
    // TODO: add Sapling spending key import
    let lc = LightClient::new_from_wallet_base(WalletBase::Ufvk(sk.to_string()), &config, 0, false)
        .unwrap();
    Runtime::new().unwrap().block_on(async move {
        let addresses = lc.do_addresses().await;
        assert_eq!(addresses["sapling_addresses"].len(), 1);
        assert_eq!(addresses["transparent_addresses"].len(), 1);
        assert_eq!(
            "zs1q6xk3q783t5k92kjqt2rkuuww8pdw2euzy5rk6jytw97enx8fhpazdv3th4xe7vsk6e9sfpawfg"
                .to_string(),
            addresses["sapling_addresses"][0]
        );

        // New address should be derived from the seed
        lc.do_new_address("z").await.unwrap();

        let addresses = lc.do_addresses().await;
        assert_eq!(addresses["sapling_addresses"].len(), 2);
        assert_ne!(
            "zs1q6xk3q783t5k92kjqt2rkuuww8pdw2euzy5rk6jytw97enx8fhpazdv3th4xe7vsk6e9sfpawfg"
                .to_string(),
            addresses["sapling_addresses"][1]
        );
    });
}
/*
#[test]
fn import_orchard_spending_key() {
    let temp_dir = tempfile::Builder::new().prefix("test").tempdir().unwrap();
    let data_dir = temp_dir
        .into_path()
        .canonicalize()
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();

    let config = ZingoConfig::create_unconnected(Network::FakeMainnet, Some(data_dir));
    Runtime::new().unwrap().block_on(async move {
        let lc = LightClient::test_new(&config, Some(TEST_SEED.to_string()), 0)
            .await
            .unwrap();
        lc.do_new_address("o").await.unwrap();
        let new_address = lc
            .wallet
            .add_imported_orchard_spending_key(
                "secret-orchard-sk-main10vj29mt2ezeyc8y5ut6knfcdptg3umdsjk4v8zge6fdmt2kepycqs6j2g8"
                    .to_string(),
                0,
            )
            .await;
        assert_eq!(new_address, "Error: Key already exists");
    });
}*/

#[test]
#[ignore]
fn new_wallet_from_zvk() {
    let temp_dir = tempfile::Builder::new().prefix("test").tempdir().unwrap();
    let data_dir = temp_dir
        .into_path()
        .canonicalize()
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();

    let config = ZingoConfig::create_unconnected(ChainType::FakeMainnet, Some(data_dir));
    let vk = "zxviews1qvpa0qr8qqqqpqxn4l054nzxpxzp3a8r2djc7sekdek5upce8mc2j2z0arzps4zv9kdvg28gjzvxd47ant6jn4svln5psw3htx93cq93ahw4e7lptrtlq7he5r6p6rcm3s0z6l24ype84sgqfrmghu449htrjspfv6qg2zfx2yrvthflmm2ynl8c0506dekul0f6jkcdmh0292lpphrksyc5z3pxwws97zd5els3l2mjt2s7hntap27mlmt6w0drtfmz36vz8pgu7ecrxzsls";
    // This will panic
    // TODO: add Sapling FVKs import
    let lc = LightClient::new_from_wallet_base(WalletBase::Ufvk(vk.to_string()), &config, 0, false)
        .unwrap();

    Runtime::new().unwrap().block_on(async move {
        let addresses = lc.do_addresses().await;
        assert_eq!(addresses["sapling_addresses"].len(), 1);
        assert_eq!(addresses["transparent_addresses"].len(), 1);
        assert_eq!(
            "zs1q6xk3q783t5k92kjqt2rkuuww8pdw2euzy5rk6jytw97enx8fhpazdv3th4xe7vsk6e9sfpawfg"
                .to_string(),
            addresses["sapling_addresses"][0]
        );

        // New address should be derived from the seed
        lc.do_new_address("z").await.unwrap();

        let addresses = lc.do_addresses().await;
        assert_eq!(addresses["sapling_addresses"].len(), 2);
        assert_ne!(
            "zs1q6xk3q783t5k92kjqt2rkuuww8pdw2euzy5rk6jytw97enx8fhpazdv3th4xe7vsk6e9sfpawfg"
                .to_string(),
            addresses["sapling_addresses"][1]
        );
    });
}

apply_scenario! {sapling_incoming_sapling_outgoing 1}
async fn sapling_incoming_sapling_outgoing(scenario: NBlockFCBLScenario) {
    let NBlockFCBLScenario {
        data,
        lightclient,
        mut fake_compactblock_list,
        ..
    } = scenario;
    // 2. Send an incoming transaction to fill the wallet
    // Note:  This creates a new block via ->add_fake_transaction->add_empty_block
    let fvk1 = SaplingFvk::try_from(&*lightclient.wallet.wallet_capability().read().await).unwrap();
    let value = 100_000;
    let (transaction, _height, _) =
        fake_compactblock_list.create_sapling_coinbase_transaction(&fvk1, value);
    let mineraddr_funding_txid = transaction.txid();
    // This is to mine the block used to add the coinbase transaction?
    mine_pending_blocks(&mut fake_compactblock_list, &data, &lightclient).await;

    assert_eq!(lightclient.wallet.last_synced_height().await, 2);

    // 3. Check the balance is correct, and we received the incoming transaction from ?outside?
    let b = lightclient.do_balance().await;
    let addresses = lightclient.do_addresses().await;
    assert_eq!(b["sapling_balance"].as_u64().unwrap(), value);
    assert_eq!(b["unverified_sapling_balance"].as_u64().unwrap(), 0);
    assert_eq!(b["spendable_sapling_balance"].as_u64().unwrap(), value);
    assert_eq!(
        addresses[0]["receivers"]["sapling"],
        encode_payment_address(
            lightclient.config.chain.hrp_sapling_payment_address(),
            lightclient
                .wallet
                .wallet_capability()
                .read()
                .await
                .addresses()[0]
                .sapling()
                .unwrap()
        ),
    );

    let list = lightclient.do_list_transactions(false).await;
    if let JsonValue::Array(list) = list {
        assert_eq!(list.len(), 1);
        let mineraddress_transaction = list[0].clone();

        assert_eq!(
            mineraddress_transaction["txid"],
            mineraddr_funding_txid.to_string()
        );
        assert_eq!(mineraddress_transaction["amount"].as_u64().unwrap(), value);
        assert_eq!(
            mineraddress_transaction["address"],
            lightclient
                .wallet
                .wallet_capability()
                .read()
                .await
                .addresses()[0]
                .encode(&lightclient.config.chain)
        );
        assert_eq!(
            mineraddress_transaction["block_height"].as_u64().unwrap(),
            2
        );
    } else {
        panic!("Expecting an array");
    }

    // 4. Send z-to-z transaction to external z address with a memo
    let sent_value = 2000;
    let outgoing_memo = "Outgoing Memo".to_string();

    let sent_transaction_id = lightclient
        .test_do_send(vec![(EXT_ZADDR, sent_value, Some(outgoing_memo.clone()))])
        .await
        .unwrap();

    // 5. Check the unconfirmed transaction is present
    // 5.1 Check notes

    let notes = lightclient.do_list_notes(true).await;
    // Has a new (unconfirmed) unspent note (the change)
    assert_eq!(notes["unspent_orchard_notes"].len(), 1);
    assert_eq!(
        notes["unspent_orchard_notes"][0]["created_in_txid"],
        sent_transaction_id
    );
    assert_eq!(
        notes["unspent_orchard_notes"][0]["unconfirmed"]
            .as_bool()
            .unwrap(),
        true
    );

    assert_eq!(notes["spent_sapling_notes"].len(), 0);
    assert_eq!(notes["pending_sapling_notes"].len(), 1);
    assert_eq!(
        notes["pending_sapling_notes"][0]["created_in_txid"],
        mineraddr_funding_txid.to_string()
    );
    assert_eq!(
        notes["pending_sapling_notes"][0]["unconfirmed_spent"],
        sent_transaction_id
    );
    assert_eq!(notes["pending_sapling_notes"][0]["spent"].is_null(), true);
    assert_eq!(
        notes["pending_sapling_notes"][0]["spent_at_height"].is_null(),
        true
    );

    // Check transaction list
    let list = lightclient.do_list_transactions(false).await;

    assert_eq!(list.len(), 2);
    let send_transaction = list
        .members()
        .find(|transaction| transaction["txid"] == sent_transaction_id)
        .unwrap();

    assert_eq!(send_transaction["txid"], sent_transaction_id);
    assert_eq!(
        send_transaction["amount"].as_i64().unwrap(),
        -(sent_value as i64 + i64::from(DEFAULT_FEE))
    );
    assert_eq!(send_transaction["unconfirmed"].as_bool().unwrap(), true);
    assert_eq!(send_transaction["block_height"].as_u64().unwrap(), 3);

    assert_eq!(
        send_transaction["outgoing_metadata"][0]["address"],
        EXT_ZADDR.to_string()
    );
    assert_eq!(
        send_transaction["outgoing_metadata"][0]["memo"],
        outgoing_memo
    );
    assert_eq!(
        send_transaction["outgoing_metadata"][0]["value"]
            .as_u64()
            .unwrap(),
        sent_value
    );

    // 6. Mine the sent transaction
    fake_compactblock_list.add_pending_sends(&data).await;
    mine_pending_blocks(&mut fake_compactblock_list, &data, &lightclient).await;

    assert_eq!(send_transaction.contains("unconfirmed"), false);
    assert_eq!(send_transaction["block_height"].as_u64().unwrap(), 3);

    // 7. Check the notes to see that we have one spent sapling note and one unspent orchard note (change)
    // Which is immediately spendable.
    let notes = lightclient.do_list_notes(true).await;
    println!("{}", json::stringify_pretty(notes.clone(), 4));
    assert_eq!(notes["unspent_orchard_notes"].len(), 1);
    assert_eq!(
        notes["unspent_orchard_notes"][0]["created_in_block"]
            .as_u64()
            .unwrap(),
        3
    );
    assert_eq!(
        notes["unspent_orchard_notes"][0]["created_in_txid"],
        sent_transaction_id
    );
    assert_eq!(
        notes["unspent_orchard_notes"][0]["value"].as_u64().unwrap(),
        value - sent_value - u64::from(DEFAULT_FEE)
    );
    assert_eq!(
        notes["unspent_orchard_notes"][0]["is_change"]
            .as_bool()
            .unwrap(),
        true
    );
    assert_eq!(
        notes["unspent_orchard_notes"][0]["spendable"]
            .as_bool()
            .unwrap(),
        true
    ); // Spendable

    assert_eq!(notes["spent_sapling_notes"].len(), 1);
    assert_eq!(
        notes["spent_sapling_notes"][0]["created_in_block"]
            .as_u64()
            .unwrap(),
        2
    );
    assert_eq!(
        notes["spent_sapling_notes"][0]["value"].as_u64().unwrap(),
        value
    );
    assert_eq!(
        notes["spent_sapling_notes"][0]["is_change"]
            .as_bool()
            .unwrap(),
        false
    );
    assert_eq!(
        notes["spent_sapling_notes"][0]["spendable"]
            .as_bool()
            .unwrap(),
        false
    ); // Already spent
    assert_eq!(
        notes["spent_sapling_notes"][0]["spent"],
        sent_transaction_id
    );
    assert_eq!(
        notes["spent_sapling_notes"][0]["spent_at_height"]
            .as_u64()
            .unwrap(),
        3
    );
}

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

    if let JsonValue::Array(mut sorted_transactions) = transactions.clone() {
        sorted_transactions.sort_by_cached_key(|t| t["amount"].as_u64().unwrap());

        for i in 0..4 {
            //assert_eq!(sorted_transactions[i]["txid"], transaction.txid().to_string());
            assert_eq!(sorted_transactions[i]["block_height"].as_u64().unwrap(), 11);
            assert_eq!(
                sorted_transactions[i]["address"],
                lightclient.do_addresses().await[0]["address"]
            );
            assert_eq!(
                sorted_transactions[i]["amount"].as_u64().unwrap(),
                value + i as u64
            );
        }
    } else {
        panic!("transactions is not array");
    }

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
    assert_eq!(transactions[4]["txid"], sent_transaction_id);
    assert_eq!(transactions[4]["block_height"], 17 as u32);
    assert_eq!(
        transactions[4]["amount"].as_i64().unwrap(),
        -(sent_value as i64) - i64::from(DEFAULT_FEE)
    );
    assert_eq!(
        transactions[4]["outgoing_metadata"][0]["address"],
        EXT_ZADDR.to_string()
    );
    assert_eq!(
        transactions[4]["outgoing_metadata"][0]["value"]
            .as_u64()
            .unwrap(),
        sent_value
    );
    assert_eq!(
        transactions[4]["outgoing_metadata"][0]["memo"].is_null(),
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
#[tokio::test]
async fn sapling_to_sapling_scan_together() {
    // Create an incoming transaction, and then send that transaction, and scan everything together, to make sure it works.
    // (For this test, the Sapling Domain is assumed in all cases.)
    // Sender Setup:
    // 1. create a spend key: SpendK_S
    // 2. derive a Shielded Payment Address from SpendK_S: SPA_KS
    // 3. construct a Block Reward Transaction where SPA_KS receives a block reward: BRT
    // 4. publish BRT
    // 5. optionally mine a block including BRT <-- There are two separate tests to run
    // 6. optionally mine sufficient subsequent blocks to "validate" BRT
    // Recipient Setup:
    // 1. create a spend key: "SpendK_R"
    // 2. from SpendK_R derive a Shielded Payment Address: SPA_R
    // Test Procedure:
    // 1. construct a transaction "spending" from a SpendK_S output to SPA_R
    // 2. publish the transaction to the mempool
    // 3. mine a block
    // Constraints:
    // 1. SpendK_S controls start - spend funds
    // 2. SpendK_R controls 0 + spend funds
    let (testserver_state, config, ready_receiver, stop_transmitter, test_server_handle) =
        create_test_server().await;

    ready_receiver.await.unwrap();

    let lightclient = LightClient::test_new(&config, WalletBase::FreshEntropy, 0)
        .await
        .unwrap();
    let mut fake_compactblock_list = FakeCompactBlockList::new(0);

    // 2. Send an incoming sapling transaction to fill the wallet
    let (mockuser_spendkey, mockuser_fvk): (SaplingSpendingKey, SaplingFvk) = {
        let wc_readlock = lightclient.wallet.wallet_capability();
        let wc = &*wc_readlock.read().await;
        (wc.try_into().unwrap(), wc.try_into().unwrap())
    };
    let value = 100_000;
    let (transaction, _height, note) = fake_compactblock_list // NOTE: Extracting fvk this way for future proof.
        .create_sapling_coinbase_transaction(
            &SaplingFvk::from(ExtendedFullViewingKey::from(&mockuser_spendkey)),
            value,
        );
    let txid = transaction.txid();

    // 3. Calculate witness so we can get the nullifier without it getting mined
    let trees = crate::blaze::test_utils::trees_from_cblocks(&fake_compactblock_list.blocks);
    let witness_from_last_sapling_tree = IncrementalWitness::from_tree(trees.0.last().unwrap());
    let nf = note.nf(
        &mockuser_fvk.fvk().vk.nk,
        witness_from_last_sapling_tree.position() as u64,
    );

    //  Create recipient to receive funds from Mock User
    let pa = if let Some(RecipientAddress::Shielded(pa)) =
        RecipientAddress::decode(&config.chain, EXT_ZADDR)
    {
        pa
    } else {
        panic!("Couldn't parse address")
    };
    let spent_value = 250;

    // Construct transaction to wallet-external recipient-address.
    let spent_txid = fake_compactblock_list
        .create_spend_transaction_from_ovk(&nf, spent_value, &mockuser_fvk.fvk().ovk, &pa)
        .txid();

    // 4. Mine the blocks and sync the lightwallet, that is, execute transactions:
    //     * from coinbase to mockuser_spendauthority
    //     * from mockuser_spendauthority to external address
    mine_pending_blocks(&mut fake_compactblock_list, &testserver_state, &lightclient).await;

    // 5. Check the transaction list to make sure we got all transactions
    let list = lightclient.do_list_transactions(false).await;

    assert_eq!(list[0]["block_height"].as_u64().unwrap(), 1);
    assert_eq!(list[0]["txid"], txid.to_string());

    assert_eq!(list[1]["block_height"].as_u64().unwrap(), 2);
    assert_eq!(list[1]["txid"], spent_txid.to_string());
    assert_eq!(list[1]["amount"].as_i64().unwrap(), -(value as i64));
    assert_eq!(
        list[1]["outgoing_metadata"][0]["address"],
        EXT_ZADDR.to_string()
    );
    assert_eq!(
        list[1]["outgoing_metadata"][0]["value"].as_u64().unwrap(),
        spent_value
    );

    clean_shutdown(stop_transmitter, test_server_handle).await;
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

apply_scenario! {mixed_transaction 10}
async fn mixed_transaction(scenario: NBlockFCBLScenario) {
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
    lightclient.test_do_send(tos).await.unwrap();

    fake_compactblock_list.add_pending_sends(&data).await;
    mine_pending_blocks(&mut fake_compactblock_list, &data, &lightclient).await;

    let notes = lightclient.do_list_notes(true).await;
    let list = lightclient.do_list_transactions(false).await;

    // 5. Check everything
    assert_eq!(notes["unspent_orchard_notes"].len(), 1);
    assert_eq!(
        notes["unspent_orchard_notes"][0]["created_in_block"]
            .as_u64()
            .unwrap(),
        18
    );
    assert_eq!(
        notes["unspent_orchard_notes"][0]["is_change"]
            .as_bool()
            .unwrap(),
        true
    );
    assert_eq!(
        notes["unspent_orchard_notes"][0]["value"].as_u64().unwrap(),
        tvalue + zvalue - sent_tvalue - sent_zvalue - u64::from(DEFAULT_FEE)
    );

    assert_eq!(notes["spent_sapling_notes"].len(), 1);
    assert_eq!(
        notes["spent_sapling_notes"][0]["spent"],
        notes["unspent_orchard_notes"][0]["created_in_txid"]
    );

    assert_eq!(notes["pending_sapling_notes"].len(), 0);
    assert_eq!(notes["utxos"].len(), 0);
    assert_eq!(notes["pending_utxos"].len(), 0);

    assert_eq!(notes["spent_utxos"].len(), 1);
    assert_eq!(
        notes["spent_utxos"][0]["spent"],
        notes["unspent_orchard_notes"][0]["created_in_txid"]
    );

    assert_eq!(list.len(), 3);
    assert_eq!(list[2]["block_height"].as_u64().unwrap(), 18);
    assert_eq!(
        list[2]["amount"].as_i64().unwrap(),
        0 - (sent_tvalue + sent_zvalue + u64::from(DEFAULT_FEE)) as i64
    );
    assert_eq!(
        list[2]["txid"],
        notes["unspent_orchard_notes"][0]["created_in_txid"]
    );
    assert_eq!(
        list[2]["outgoing_metadata"]
            .members()
            .find(|j| j["address"].to_string() == EXT_ZADDR
                && j["value"].as_u64().unwrap() == sent_zvalue)
            .unwrap()["memo"]
            .to_string(),
        sent_zmemo
    );
    assert_eq!(
        list[2]["outgoing_metadata"]
            .members()
            .find(|j| j["address"].to_string() == EXT_TADDR)
            .unwrap()["value"]
            .as_u64()
            .unwrap(),
        sent_tvalue
    );
}

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
async fn witness_clearing(scenario: NBlockFCBLScenario) {
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
    let (transaction, _height, _) =
        fake_compactblock_list.create_sapling_coinbase_transaction(&fvk1, value);
    let txid = transaction.txid();
    mine_pending_blocks(&mut fake_compactblock_list, &data, &lightclient).await;
    mine_numblocks_each_with_two_sap_txs(&mut fake_compactblock_list, &data, &lightclient, 5).await;

    // 3. Send z-to-z transaction to external z address with a memo
    let sent_value = 2000;
    let outgoing_memo = "Outgoing Memo".to_string();

    let _sent_transaction_id = lightclient
        .test_do_send(vec![(EXT_ZADDR, sent_value, Some(outgoing_memo.clone()))])
        .await
        .unwrap();

    // transaction is not yet mined, so witnesses should still be there
    let witnesses = lightclient
        .wallet
        .transactions()
        .read()
        .await
        .current
        .get(&txid)
        .unwrap()
        .sapling_notes
        .get(0)
        .unwrap()
        .witnesses
        .clone();
    assert_eq!(witnesses.len(), 6);

    // 4. Mine the sent transaction
    fake_compactblock_list.add_pending_sends(&data).await;
    mine_pending_blocks(&mut fake_compactblock_list, &data, &lightclient).await;

    // transaction is now mined, but witnesses should still be there because not 100 blocks yet (i.e., could get reorged)
    let witnesses = lightclient
        .wallet
        .transactions()
        .read()
        .await
        .current
        .get(&txid)
        .unwrap()
        .sapling_notes
        .get(0)
        .unwrap()
        .witnesses
        .clone();
    assert_eq!(witnesses.len(), 6);

    // 5. Mine 50 blocks, witness should still be there
    mine_numblocks_each_with_two_sap_txs(&mut fake_compactblock_list, &data, &lightclient, 50)
        .await;
    let witnesses = lightclient
        .wallet
        .transactions()
        .read()
        .await
        .current
        .get(&txid)
        .unwrap()
        .sapling_notes
        .get(0)
        .unwrap()
        .witnesses
        .clone();
    assert_eq!(witnesses.len(), 6);

    // 5. Mine 100 blocks, witness should now disappear
    mine_numblocks_each_with_two_sap_txs(&mut fake_compactblock_list, &data, &lightclient, 100)
        .await;
    let witnesses = lightclient
        .wallet
        .transactions()
        .read()
        .await
        .current
        .get(&txid)
        .unwrap()
        .sapling_notes
        .get(0)
        .unwrap()
        .witnesses
        .clone();
    assert_eq!(witnesses.len(), 0);
}

apply_scenario! {mempool_clearing 10}
async fn mempool_clearing(scenario: NBlockFCBLScenario) {
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
    let (transaction, _height, _) =
        fake_compactblock_list.create_sapling_coinbase_transaction(&fvk1, value);
    let orig_transaction_id = transaction.txid().to_string();
    mine_pending_blocks(&mut fake_compactblock_list, &data, &lightclient).await;
    mine_numblocks_each_with_two_sap_txs(&mut fake_compactblock_list, &data, &lightclient, 5).await;
    assert_eq!(
        lightclient.do_maybe_recent_txid().await["last_txid"],
        orig_transaction_id
    );

    // 3. Send z-to-z transaction to external z address with a memo
    let sent_value = 2000;
    let outgoing_memo = "Outgoing Memo".to_string();

    let sent_transaction_id = lightclient
        .test_do_send(vec![(EXT_ZADDR, sent_value, Some(outgoing_memo.clone()))])
        .await
        .unwrap();

    // 4. The transaction is not yet sent, it is just sitting in the test GRPC server, so remove it from there to make sure it doesn't get mined.
    assert_eq!(
        lightclient.do_maybe_recent_txid().await["last_txid"],
        sent_transaction_id
    );
    let mut sent_transactions = data
        .write()
        .await
        .sent_transactions
        .drain(..)
        .collect::<Vec<_>>();
    assert_eq!(sent_transactions.len(), 1);
    let sent_transaction = sent_transactions.remove(0);

    // 5. At this point, the raw transaction is already been parsed, but we'll parse it again just to make sure it doesn't create any duplicates.
    let notes_before = lightclient.do_list_notes(true).await;
    let transactions_before = lightclient.do_list_transactions(false).await;

    let transaction = Transaction::read(&sent_transaction.data[..], BranchId::Sapling).unwrap();
    lightclient
        .wallet
        .transaction_context
        .scan_full_tx(transaction, BlockHeight::from_u32(17), true, 0, None)
        .await;

    {
        let transactions_reader = lightclient
            .wallet
            .transaction_context
            .transaction_metadata_set
            .clone();
        let wallet_transactions = transactions_reader.read().await;
        let sapling_notes: Vec<_> = wallet_transactions
            .current
            .values()
            .map(|wallet_tx| &wallet_tx.sapling_notes)
            .flatten()
            .collect();
        assert_ne!(sapling_notes.len(), 0);
        for note in sapling_notes {
            let mut note_bytes = Vec::new();
            note.write(&mut note_bytes).unwrap();
            let note2 = ReceivedSaplingNoteAndMetadata::read(&*note_bytes, ()).unwrap();
            assert_eq!(note.fvk().to_bytes(), note2.fvk().to_bytes());
            assert_eq!(note.nullifier(), note2.nullifier());
            assert_eq!(note.diversifier, note2.diversifier);
            assert_eq!(note.note, note2.note);
            assert_eq!(note.spent, note2.spent);
            assert_eq!(note.unconfirmed_spent, note2.unconfirmed_spent);
            assert_eq!(note.memo, note2.memo);
            assert_eq!(note.is_change, note2.is_change);
            assert_eq!(note.have_spending_key, note2.have_spending_key);
        }
    }
    let notes_after = lightclient.do_list_notes(true).await;
    let transactions_after = lightclient.do_list_transactions(false).await;

    assert_eq!(notes_before.pretty(2), notes_after.pretty(2));
    assert_eq!(transactions_before.pretty(2), transactions_after.pretty(2));

    // 6. Mine 10 blocks, the unconfirmed transaction should still be there.
    mine_numblocks_each_with_two_sap_txs(&mut fake_compactblock_list, &data, &lightclient, 10)
        .await;
    assert_eq!(lightclient.wallet.last_synced_height().await, 26);

    let notes = lightclient.do_list_notes(true).await;
    let transactions = lightclient.do_list_transactions(false).await;

    // There is 1 unspent note, which is the unconfirmed transaction
    println!("{}", json::stringify_pretty(notes.clone(), 4));
    println!("{}", json::stringify_pretty(transactions.clone(), 4));
    // One unspent note, change, unconfirmed
    assert_eq!(notes["unspent_orchard_notes"].len(), 1);
    assert_eq!(notes["unspent_sapling_notes"].len(), 0);
    let note = notes["unspent_orchard_notes"][0].clone();
    assert_eq!(note["created_in_txid"], sent_transaction_id);
    assert_eq!(
        note["value"].as_u64().unwrap(),
        value - sent_value - u64::from(DEFAULT_FEE)
    );
    assert_eq!(note["unconfirmed"].as_bool().unwrap(), true);
    assert_eq!(transactions.len(), 2);

    // 7. Mine 100 blocks, so the mempool expires
    mine_numblocks_each_with_two_sap_txs(&mut fake_compactblock_list, &data, &lightclient, 100)
        .await;
    assert_eq!(lightclient.wallet.last_synced_height().await, 126);

    let notes = lightclient.do_list_notes(true).await;
    let transactions = lightclient.do_list_transactions(false).await;

    // There is now again 1 unspent note, but it is the original (confirmed) note.
    assert_eq!(notes["unspent_sapling_notes"].len(), 1);
    assert_eq!(
        notes["unspent_sapling_notes"][0]["created_in_txid"],
        orig_transaction_id
    );
    assert_eq!(
        notes["unspent_sapling_notes"][0]["unconfirmed"]
            .as_bool()
            .unwrap(),
        false
    );
    assert_eq!(notes["pending_sapling_notes"].len(), 0);
    assert_eq!(transactions.len(), 1);
}
apply_scenario! {mempool_and_balance 10}
async fn mempool_and_balance(scenario: NBlockFCBLScenario) {
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

    let bal = lightclient.do_balance().await;
    println!("{}", json::stringify_pretty(bal.clone(), 4));
    assert_eq!(bal["sapling_balance"].as_u64().unwrap(), value);
    assert_eq!(bal["unverified_sapling_balance"].as_u64().unwrap(), 0);
    assert_eq!(bal["verified_sapling_balance"].as_u64().unwrap(), value);

    // 3. Mine 10 blocks
    mine_numblocks_each_with_two_sap_txs(&mut fake_compactblock_list, &data, &lightclient, 10)
        .await;
    let bal = lightclient.do_balance().await;
    assert_eq!(bal["sapling_balance"].as_u64().unwrap(), value);
    assert_eq!(bal["verified_sapling_balance"].as_u64().unwrap(), value);
    assert_eq!(bal["unverified_sapling_balance"].as_u64().unwrap(), 0);

    // 4. Spend the funds
    let sent_value = 2000;
    let outgoing_memo = "Outgoing Memo".to_string();

    let _sent_transaction_id = lightclient
        .test_do_send(vec![(EXT_ZADDR, sent_value, Some(outgoing_memo.clone()))])
        .await
        .unwrap();

    let bal = lightclient.do_balance().await;

    // Even though the transaction is not mined (in the mempool) the balances should be updated to reflect the spent funds
    let new_bal = value - (sent_value + u64::from(DEFAULT_FEE));
    assert_eq!(bal["orchard_balance"].as_u64().unwrap(), new_bal);
    assert_eq!(bal["verified_orchard_balance"].as_u64().unwrap(), 0);
    assert_eq!(bal["unverified_orchard_balance"].as_u64().unwrap(), new_bal);

    // 5. Mine the pending block, making the funds verified and spendable.
    fake_compactblock_list.add_pending_sends(&data).await;
    mine_pending_blocks(&mut fake_compactblock_list, &data, &lightclient).await;

    let bal = lightclient.do_balance().await;

    assert_eq!(bal["orchard_balance"].as_u64().unwrap(), new_bal);
    assert_eq!(bal["verified_orchard_balance"].as_u64().unwrap(), new_bal);
    assert_eq!(bal["unverified_orchard_balance"].as_u64().unwrap(), 0);
}

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

#[tokio::test]
async fn load_wallet_from_v26_dat_file() {
    // We test that the LightWallet can be read from v26 .dat file
    // Changes in version 27:
    //   - The wallet does not have to have a mnemonic.
    //     Absence of mnemonic is represented by an empty byte vector in v27.
    //     v26 serialized wallet is always loaded with `Some(mnemonic)`.
    //   - The wallet capabilities can be restricted from spending to view-only or none.
    //     We introduce `Capability` type represent different capability types in v27.
    //     v26 serialized wallet is always loaded with `Capability::Spend(sk)`.

    // A testnet wallet initiated with
    // --seed "chimney better bulb horror rebuild whisper improve intact letter giraffe brave rib appear bulk aim burst snap salt hill sad merge tennis phrase raise"
    // --birthday 0
    // --nosync
    // with 3 addresses containig all receivers.
    let data = include_bytes!("zingo-wallet-v26.dat");

    let config = zingoconfig::ZingoConfig::create_unconnected(ChainType::Testnet, None);
    let wallet = LightWallet::read_internal(&data[..], &config)
        .await
        .map_err(|e| format!("Cannot deserialize LightWallet version 26 file: {}", e))
        .unwrap();

    let expected_mnemonic = Mnemonic::from_phrase(TEST_SEED.to_string()).unwrap();
    assert_eq!(wallet.mnemonic(), Some(&expected_mnemonic));

    let expected_wc = WalletCapability::new_from_phrase(&config, &expected_mnemonic, 0).unwrap();
    let wc = wallet.wallet_capability().read().await.clone();

    // We don't want the WalletCapability to impl. `Eq` (because it stores secret keys)
    // so we have to compare each component instead

    // Compare Orchard
    let Capability::Spend(orchard_sk) = &wc.orchard else {
        panic!("Expected Orchard Spending Key");
    };
    assert_eq!(
        orchard_sk.to_bytes(),
        OrchardSpendingKey::try_from(&expected_wc)
            .unwrap()
            .to_bytes()
    );

    // Compare Sapling
    let Capability::Spend(sapling_sk) = &wc.sapling else {
        panic!("Expected Sapling Spending Key");
    };
    assert_eq!(
        sapling_sk,
        &SaplingSpendingKey::try_from(&expected_wc).unwrap()
    );

    // Compare transparent
    let Capability::Spend(transparent_sk) = &wc.transparent else {
        panic!("Expected transparent extended private key");
    };
    assert_eq!(
        transparent_sk,
        &ExtendedPrivKey::try_from(&expected_wc).unwrap()
    );

    assert_eq!(wc.addresses().len(), 3);
    for addr in wc.addresses() {
        assert!(addr.orchard().is_some());
        assert!(addr.sapling().is_some());
        assert!(addr.transparent().is_some());
    }
}

#[tokio::test]
async fn test_scanning_in_watch_only_mode() {
    // # Scenario:
    // 1. fill wallet with a coinbase transaction
    // 2. send a transaction contaning all types of outputs
    // 3. reset wallet
    // 4. for every combination of FVKs
    //     4.1. init a wallet with UFVK
    //     4.2. check that the wallet is empty
    //     4.3. rescan
    //     4.4. check that notes and utxos were detected by the wallet
    //
    // # Current watch-only mode limitations:
    // - wallet will not detect funds on all transparent addresses
    //   see: https://github.com/zingolabs/zingolib/issues/245
    // - wallet will not detect funds on internal addresses
    //   see: https://github.com/zingolabs/zingolib/issues/246

    // wait for test server to start
    let (data, config, ready_receiver, _stop_transmitter, _test_server_handle) =
        create_test_server().await;
    ready_receiver.await.unwrap();

    let lightclient = LightClient::test_new(
        &config,
        WalletBase::MnemonicPhrase(TEST_SEED.to_string()),
        0,
    )
    .await
    .unwrap();

    let mut fake_compactblock_list = FakeCompactBlockList::new(0);
    let wc = lightclient.wallet.wallet_capability().read().await.clone();

    // create a coinbase transaction
    let extfvk: SaplingFvk = (&wc).try_into().unwrap();
    let value = 1_111_000;
    let (transaction, _height, _note) =
        fake_compactblock_list.create_sapling_coinbase_transaction(&extfvk, value);
    let txid = transaction.txid();

    // make the coinbase transaction spendable
    mine_pending_blocks(&mut fake_compactblock_list, &data, &lightclient).await;
    mine_numblocks_each_with_two_sap_txs(&mut fake_compactblock_list, &data, &lightclient, 5).await;

    // test that we have the transaction
    let list = lightclient.do_list_transactions(false).await;
    assert_eq!(list[0]["txid"], txid.to_string());
    assert_eq!(list[0]["amount"].as_u64().unwrap(), value);
    let addr_0 = wc.addresses()[0].clone();
    assert_eq!(list[0]["address"], addr_0.encode(&config.chain));
    assert_eq!(
        lightclient.do_balance().await["sapling_balance"]
            .as_u64()
            .unwrap(),
        value
    );

    // send a transaction we want to watch
    let o_addr = addr_0.orchard().clone().unwrap();
    let s_addr = addr_0.sapling().clone().unwrap();
    let t_addr = addr_0.transparent().clone().unwrap();
    let o_addr_str =
        UAddress::try_from_items(vec![Receiver::Orchard(o_addr.to_raw_address_bytes())])
            .unwrap()
            .encode(&config.chain.to_zcash_address_network());
    let s_addr_str = UAddress::try_from_items(vec![Receiver::Sapling(s_addr.to_bytes())])
        .unwrap()
        .encode(&config.chain.to_zcash_address_network());
    let t_addr_str = address_from_pubkeyhash(&config, Some(t_addr.clone())).unwrap();
    let sent_o_value = 1_000_000;
    let sent_s_value = 100_000;
    let sent_t_value = 10_000;
    let sent_o_memo = "Some Orchard memo".to_string();
    let sent_s_memo = "Some Sapling memo".to_string();
    let tos = vec![
        (&o_addr_str[..], sent_o_value, Some(sent_o_memo.clone())),
        (&s_addr_str[..], sent_s_value, Some(sent_s_memo.clone())),
        (&t_addr_str[..], sent_t_value, None),
    ];
    lightclient.test_do_send(tos).await.unwrap();

    // confirm that transaction
    fake_compactblock_list.add_pending_sends(&data).await;
    mine_pending_blocks(&mut fake_compactblock_list, &data, &lightclient).await;
    mine_numblocks_each_with_two_sap_txs(&mut fake_compactblock_list, &data, &lightclient, 5).await;

    // check that the wallet has received the transansaction
    {
        let balance = lightclient.do_balance().await;
        assert_eq!(balance["sapling_balance"], sent_s_value);
        assert_eq!(balance["verified_sapling_balance"], sent_s_value);
        assert_eq!(balance["unverified_sapling_balance"], 0);
        assert_eq!(balance["orchard_balance"], sent_o_value);
        assert_eq!(balance["verified_orchard_balance"], sent_o_value);
        assert_eq!(balance["unverified_orchard_balance"], 0);
        assert_eq!(balance["transparent_balance"], sent_t_value);
    }

    // check that do_rescan works
    lightclient.do_rescan().await.unwrap();
    {
        let balance = lightclient.do_balance().await;
        assert_eq!(balance["sapling_balance"], sent_s_value);
        assert_eq!(balance["verified_sapling_balance"], sent_s_value);
        assert_eq!(balance["unverified_sapling_balance"], 0);
        assert_eq!(balance["orchard_balance"], sent_o_value);
        assert_eq!(balance["verified_orchard_balance"], sent_o_value);
        assert_eq!(balance["unverified_orchard_balance"], 0);
        assert_eq!(balance["transparent_balance"], sent_t_value);
    }

    let o_fvk = Fvk::Orchard(OrchardFvk::try_from(&wc).unwrap().to_bytes());
    let s_fvk = Fvk::Sapling(SaplingFvk::try_from(&wc).unwrap().to_bytes());
    let mut t_fvk_bytes = [0u8; 65];
    let t_ext_pk: ExtendedPubKey = (&wc).try_into().unwrap();
    t_fvk_bytes[0..32].copy_from_slice(&t_ext_pk.chain_code[..]);
    t_fvk_bytes[32..65].copy_from_slice(&t_ext_pk.public_key.serialize()[..]);
    let t_fvk = Fvk::P2pkh(t_fvk_bytes);
    let fvks_sets = vec![
        vec![&o_fvk],
        vec![&s_fvk],
        vec![&o_fvk, &s_fvk],
        vec![&o_fvk, &t_fvk],
        vec![&s_fvk, &t_fvk],
        vec![&o_fvk, &s_fvk, &t_fvk],
    ];
    for fvks_set in fvks_sets.iter() {
        log::debug!("testing UFVK containig:");
        log::debug!("    orchard fvk: {}", fvks_set.contains(&&o_fvk));
        log::debug!("    sapling fvk: {}", fvks_set.contains(&&s_fvk));
        log::debug!("    transparent fvk: {}", fvks_set.contains(&&t_fvk));

        let ufvk = Ufvk::try_from_items(fvks_set.clone().into_iter().map(|x| x.clone()).collect())
            .unwrap()
            .encode(&config.chain.to_zcash_address_network());

        let watch_client = LightClient::test_new(&config, WalletBase::Ufvk(ufvk), 0)
            .await
            .unwrap();

        let watch_wc = watch_client.wallet.wallet_capability().read().await.clone();

        // assert empty wallet before rescan
        {
            let balance = watch_client.do_balance().await;
            assert_eq!(balance["sapling_balance"], 0);
            assert_eq!(balance["verified_sapling_balance"], 0);
            assert_eq!(balance["unverified_sapling_balance"], 0);
            assert_eq!(balance["orchard_balance"], 0);
            assert_eq!(balance["verified_orchard_balance"], 0);
            assert_eq!(balance["unverified_orchard_balance"], 0);
            assert_eq!(balance["transparent_balance"], 0);
        }

        watch_client.do_rescan().await.unwrap();
        let balance = watch_client.do_balance().await;
        let notes = watch_client.do_list_notes(true).await;

        // Orchard
        if fvks_set.contains(&&o_fvk) {
            assert!(watch_wc.orchard.can_view());
            assert_eq!(balance["orchard_balance"], sent_o_value);
            assert_eq!(balance["verified_orchard_balance"], sent_o_value);
            // assert 1 Orchard note, or 2 notes if a dummy output is included
            let orchard_notes_count = notes["unspent_orchard_notes"].members().count();
            assert!((1..=2).contains(&orchard_notes_count));
        } else {
            assert!(!watch_wc.orchard.can_view());
            assert_eq!(balance["orchard_balance"], 0);
            assert_eq!(balance["verified_orchard_balance"], 0);
            assert_eq!(notes["unspent_orchard_notes"].members().count(), 0);
        }

        // Sapling
        if fvks_set.contains(&&s_fvk) {
            assert!(watch_wc.sapling.can_view());
            assert_eq!(balance["sapling_balance"], sent_s_value);
            assert_eq!(balance["verified_sapling_balance"], sent_s_value);
            assert_eq!(notes["unspent_sapling_notes"].members().count(), 1);
        } else {
            assert!(!watch_wc.sapling.can_view());
            assert_eq!(balance["sapling_balance"], 0);
            assert_eq!(balance["verified_sapling_balance"], 0);
            assert_eq!(notes["unspent_sapling_notes"].members().count(), 0);
        }

        // transparent
        if fvks_set.contains(&&t_fvk) {
            assert!(watch_wc.transparent.can_view());
            assert_eq!(balance["transparent_balance"], sent_t_value);
            assert_eq!(notes["utxos"].members().count(), 1);
        } else {
            assert!(!watch_wc.transparent.can_view());
            assert_eq!(notes["utxos"].members().count(), 0);
        }
    }
}

#[tokio::test]
async fn refuse_spending_in_watch_only_mode() {
    // create a test enviroment
    let (data, config, ready_receiver, _stop_transmitter, _test_server_handle) =
        create_test_server().await;
    {
        ready_receiver.await.unwrap();
        let mut fake_compactblock_list = FakeCompactBlockList::new(0);
        let lightclient = LightClient::test_new(&config, WalletBase::FreshEntropy, 0)
            .await
            .unwrap();
        mine_numblocks_each_with_two_sap_txs(&mut fake_compactblock_list, &data, &lightclient, 5)
            .await;
    }

    // we run the test for several UFVKs
    let some_ufvks = vec![
        "uview10gy6gvvdhxg8r36frvpc35mgnmgjgnzxtd06wlwsrmllhuc9j6tw6mgdtyz82d6vfzlvwrf3ejz9njngyr2l88wpq833fll6mpmfuvz4g3c46t2r5vxa8vatlhreeqazka6l7z70yh7jwycuk8vz8e74xatt4zwn5utrr56ylpclngy9t9gn6qgejve7z".to_string(),
        "uview1gyq8hc0apfr54lp3x7rl4y2me74rlhqla4f5eh9prtunx32x0ck45dxltvjk9atewzr3uhq9fl2j3gm4uk4aayn2tqpue83s53fxkgr0zumg4arad7uhmhhc488y3d06aeyjy6f7farnxcfprrw005q7wey7x8gz52jlhaxka98hfp65gq5er6dwtyraxzzauc8k9n2uw7u864zjraq4dqshjqngq3qxwlgqhhpcwvf76x36".to_string(),
        "uview1hvy0kh9xxqn6z54scnt6hhvlkp6psdv3vcrhzspnrnusd6zwuyqnvjnz9dstd99vm6xv4984p7lg2hsru5z22zc8ze02a83r4qzkytrur7s5gky0gv2e9kgkkpqr6ylaswysmuenqg03s8qf9cukkju8v765dvpun3jp6vyv6u8f2qgxnsdyq8v6424w0ewu9djaq2npcpf0mmuur8xhfxtnmxj36ezyl276sszy967pumlnshsl8qfllnyk57emyl40rnt4w0tug9zxepyw5ehal5vkw9sa6nemlg35vtrw6qtsu536sg54rsv6y8lw5ksrgnc5n5w03dz3xuem52ltq0x24ylzfp5u8hmu2u8vx4rs2fsn9085qnr8vpgpxwujujqzwmu3z".to_string(),
        "uview1hq3tvgethyxrqrxcah70j0w8zxsm7lsjukpk45ykj3uhq0dzfavygas7tfhxnqsqujlgv35nguewd9apl3errdz8q9erz2z78700zmlswltd88qxlnx5eqr4qn0dhc2k320u988anrp9vh60c9qnwrhxrlq8fcartuxg6qslzdlylnz30xlnpzgc2erlgl9326sqgs3mfjfrh40x5nu82yp5qnl46ulj522x387j5cw5l7kxtyjjkzlwfkcptnpp5dam7hy4308pg9vgs558n9xmwkgcypepcs7k8wyq".to_string(),
        "uview1vyga9aepl8hs2k4dpenvxdhdw69ue8y4yr4c9w0y4p5c9heu505mt3w5gdcrk0n0epyqaztuxuuqfhd7nxxrk2dekwhhl0wlhnltc4pj280wk2ml8vdfgvzy24zlaqc8dehdwp3dyxe8700mg2mh0tp5t5mpqngwxup8xqgq687nypga8jzgsrrh8q880lljam88c4q0c60vlkdpfm5xq5c8fz57a83feurknu7kh95xh659anqzu5gkacls6zrgquj9ct00q3vjupy80r48a2q66ws2t28l7hx5a2czuj2vknd7xrqc866qmuyfujfvey9x7v90986c36y7f90gycyd7z7".to_string(),
        "uview13c2v3h2qxz954frrwxjzy5s3q3v3kmzh48qjs7nvcwvl9h6g47ytt8392f85n9qd6flyhj8gzslght9p6nfqpdlv6vpfc6gevygj4u22nsvnyt9jhy98eq0n0udcxmxst886j6vycukk3f0p57rpgn2v994yzcph933xhk42822wh822uej4yztj2cvcvc6qfxmva707rtjqml48k80j05gkj3920k0y8qxxze8wfjw42hgg3tzdytdn8nvyuzv5sqet77hha3q8jh6sr4vcl4n90hggayjum4lmkte5x27ne3hsaz7fee5rf0l47uwdgzy84etngcr7zy9mpx36hdyfkrkcype6rd9l46duht8qj27qgsqdk0p2puxzx00rtx246ua09f8j3eak8zvl809xuyjahzquz6zm4pslyr0m0490ay6y0lq78uh6d2z9zpke7l2fsljujtx4gsd4muczr4h7jzelu986t43vcem2sksezsgkstxe".to_string(),
    ];

    for ufvk in some_ufvks.into_iter() {
        let wallet_base = WalletBase::Ufvk(ufvk);
        let watch_client = LightClient::test_new(&config, wallet_base, 0)
            .await
            .unwrap();
        watch_client.do_rescan().await.unwrap();
        assert_eq!(
            watch_client.do_send(vec![(EXT_TADDR, 1000, None)]).await,
            Err("Wallet is in watch-only mode a thus it cannot spend".to_string())
        );
    }
}

#[tokio::test]
async fn test_do_rescan() {
    // we test that do_rescan re-detect all Sapling and Orchard notes

    // wait for test server to start
    let (data, config, ready_receiver, _stop_transmitter, _test_server_handle) =
        create_test_server().await;
    ready_receiver.await.unwrap();

    let lightclient = LightClient::test_new(
        &config,
        WalletBase::MnemonicPhrase(TEST_SEED.to_string()),
        0,
    )
    .await
    .unwrap();

    let mut fake_compactblock_list = FakeCompactBlockList::new(0);
    let wc = lightclient.wallet.wallet_capability().read().await.clone();

    // create transaction funding
    let extfvk: SaplingFvk = (&wc).try_into().unwrap();
    let value = 1_000_000 + 1_000;
    let (transaction, _height, _note) =
        fake_compactblock_list.create_sapling_coinbase_transaction(&extfvk, value);
    let txid = transaction.txid();

    // make funding transaction spendable
    mine_pending_blocks(&mut fake_compactblock_list, &data, &lightclient).await;
    mine_numblocks_each_with_two_sap_txs(&mut fake_compactblock_list, &data, &lightclient, 5).await;

    // test that we have the transaction
    let list = lightclient.do_list_transactions(false).await;
    assert_eq!(list[0]["txid"], txid.to_string());
    assert_eq!(list[0]["amount"].as_u64().unwrap(), value);
    let addr_0 = wc.addresses()[0].clone().encode(&config.chain);
    assert_eq!(list[0]["address"], addr_0);

    // check the balance
    {
        let balance = lightclient.do_balance().await;
        assert_eq!(balance["sapling_balance"].as_u64().unwrap(), value);
        assert_eq!(balance["orchard_balance"], 0);
    }

    // send funds to orchard addresss
    let send_o_value = 600_000;
    let send_s_value = 400_000;
    // create new Sapling-only address
    let s_addr = lightclient
        .wallet
        .wallet_capability()
        .write()
        .await
        .new_address(ReceiverSelection {
            orchard: false,
            sapling: true,
            transparent: false,
        })
        .unwrap()
        .encode(&config.chain);
    let tos = vec![
        (&addr_0[..], send_o_value, None),
        (&s_addr[..], send_s_value, None),
    ];
    lightclient.test_do_send(tos).await.unwrap();

    // mine the transaction
    fake_compactblock_list.add_pending_sends(&data).await;
    mine_pending_blocks(&mut fake_compactblock_list, &data, &lightclient).await;
    mine_numblocks_each_with_two_sap_txs(&mut fake_compactblock_list, &data, &lightclient, 5).await;

    // we assert these amounts
    {
        let balance = lightclient.do_balance().await;
        assert_eq!(balance["sapling_balance"], send_s_value);
        assert_eq!(balance["verified_sapling_balance"], send_s_value);
        assert_eq!(balance["orchard_balance"], send_o_value);
        assert_eq!(balance["verified_orchard_balance"], send_o_value);
    }

    lightclient.do_rescan().await.unwrap();

    // we assert same amounts after the rescan
    {
        let balance = lightclient.do_balance().await;
        assert_eq!(balance["sapling_balance"], send_s_value);
        assert_eq!(balance["verified_sapling_balance"], send_s_value);
        assert_eq!(balance["orchard_balance"], send_o_value);
        assert_eq!(balance["verified_orchard_balance"], send_o_value);
    }
}

pub const EXT_TADDR: &str = "t1NoS6ZgaUTpmjkge2cVpXGcySasdYDrXqh";
pub const EXT_ZADDR: &str =
    "zs1va5902apnzlhdu0pw9r9q7ca8s4vnsrp2alr6xndt69jnepn2v2qrj9vg3wfcnjyks5pg65g9dc";
pub const EXT_ZADDR2: &str =
    "zs1fxgluwznkzm52ux7jkf4st5znwzqay8zyz4cydnyegt2rh9uhr9458z0nk62fdsssx0cqhy6lyv";
pub const TEST_SEED: &str = "chimney better bulb horror rebuild whisper improve intact letter giraffe brave rib appear bulk aim burst snap salt hill sad merge tennis phrase raise";
