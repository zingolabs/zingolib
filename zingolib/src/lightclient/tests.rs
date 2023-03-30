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
