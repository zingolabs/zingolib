use zcash_protocol::value::Zatoshis;

use crate::mocks;

#[test]
fn total_payment_amount() {
    let proposal = mocks::proposal::ProposalBuilder::default().build();
    assert_eq!(
        super::total_payment_amount(&proposal).unwrap(),
        Zatoshis::from_u64(100_000).unwrap()
    );
}
#[test]
fn total_fee() {
    let proposal = mocks::proposal::ProposalBuilder::default().build();
    assert_eq!(
        super::total_fee(&proposal).unwrap(),
        Zatoshis::from_u64(20_000).unwrap()
    );
}
