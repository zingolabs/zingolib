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

const SWAP_MEMO: &[u8] = b"OUT:3D67564CCA60B96A6A00D4D2CD81E1EA4AD052F3D7118CCBD2C9294A9D0C3B8A";
const OP_DUP: u8 = 0x76;

fn script(opcodes: &[u8], payload: &[u8]) -> Vec<u8> {
    [&[OP_RETURN], opcodes, payload].concat()
}

#[test]
fn null_data_payload_reads_a_direct_push() {
    let length = u8::try_from(PAYLOAD.len()).expect("PAYLOAD is under MAX_DIRECT_PUSH");
    assert_eq!(
        null_data_payload(&script(&[length], PAYLOAD)),
        Some(PAYLOAD.to_vec())
    );
}

#[test]
fn null_data_payload_reads_pushdata1_and_pushdata2() {
    let length = u16::try_from(SWAP_MEMO.len()).expect("SWAP_MEMO is under MAX_OP_RETURN_BYTES");
    let [low, high] = length.to_le_bytes();
    assert_eq!(
        null_data_payload(&script(&[OP_PUSHDATA1, low], SWAP_MEMO)),
        Some(SWAP_MEMO.to_vec())
    );
    assert_eq!(
        null_data_payload(&script(&[OP_PUSHDATA2, low, high], SWAP_MEMO)),
        Some(SWAP_MEMO.to_vec())
    );
}

#[test]
fn null_data_payload_reads_an_empty_op_return() {
    assert_eq!(null_data_payload(&[OP_RETURN]), Some(Vec::new()));
}

#[test]
fn null_data_payload_refuses_other_scripts_and_short_pushes() {
    let length = u8::try_from(PAYLOAD.len()).expect("PAYLOAD is under MAX_DIRECT_PUSH");
    assert_eq!(null_data_payload(&[OP_DUP]), None);
    assert_eq!(null_data_payload(&script(&[length], &PAYLOAD[1..])), None);
    assert_eq!(null_data_payload(&[]), None);
}

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

    crate::testutils::assertions::assert_null_data_send(bundle, PAYLOAD);
    assert_eq!(bundle.vin[0].prevout().n(), 0);
    assert_eq!(bundle.vin[0].prevout().hash(), &[7u8; 32]);

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
