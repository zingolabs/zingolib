use super::*;

#[test]
fn a_seed_fixes_the_permutation() {
    let mut first = vec![1, 2, 3, 4, 5, 6, 7, 8];
    let mut second = first.clone();
    seeded_shuffle(&mut first, 42);
    seeded_shuffle(&mut second, 42);
    assert_eq!(first, second, "the same seed yields the same order");
}

#[test]
fn shuffling_permutes_without_loss() {
    let mut items = vec![1, 2, 3, 4, 5, 6, 7, 8];
    seeded_shuffle(&mut items, 7);
    let mut sorted = items.clone();
    sorted.sort();
    assert_eq!(sorted, vec![1, 2, 3, 4, 5, 6, 7, 8]);
}

#[test]
fn empty_and_singleton_slices_are_untouched() {
    let mut empty: Vec<u8> = vec![];
    seeded_shuffle(&mut empty, 1);
    assert!(empty.is_empty());

    let mut one = vec![9];
    seeded_shuffle(&mut one, 1);
    assert_eq!(one, vec![9]);
}
