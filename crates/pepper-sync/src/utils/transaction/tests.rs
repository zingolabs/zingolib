use super::*;

#[test]
fn output_count_reads_each_pools_own_field() {
    let transaction = compact_tx(1, 2, 3, 4);
    assert_eq!(
        shielded_output_count(&transaction, ShieldedPool::Sapling),
        2
    );
    assert_eq!(
        shielded_output_count(&transaction, ShieldedPool::Orchard),
        3
    );
    assert_eq!(
        shielded_output_count(&transaction, ShieldedPool::Ironwood),
        4
    );
}

#[test]
fn input_count_reads_spends_for_sapling_and_actions_for_action_pools() {
    let transaction = compact_tx(1, 2, 3, 4);
    assert_eq!(shielded_input_count(&transaction, ShieldedPool::Sapling), 1);
    assert_eq!(shielded_input_count(&transaction, ShieldedPool::Orchard), 3);
    assert_eq!(
        shielded_input_count(&transaction, ShieldedPool::Ironwood),
        4
    );
}

#[test]
fn empty_transaction_counts_zero_for_every_pool() {
    let transaction = compact_tx(0, 0, 0, 0);
    for pool in ALL_POOLS {
        assert_eq!(shielded_output_count(&transaction, pool), 0);
        assert_eq!(shielded_input_count(&transaction, pool), 0);
    }
}
