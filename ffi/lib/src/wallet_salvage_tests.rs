use super::*;

fn stable_prefix() -> Vec<u8> {
    let mut bytes = 42u64.to_le_bytes().to_vec();
    bytes.push(0);
    bytes.push(32);
    bytes.extend([7u8; 32]);
    bytes.extend(2_000_000u32.to_le_bytes());
    bytes.push(1);
    bytes
}

#[test]
fn a_truncated_wallet_salvages_seed_and_birthday() {
    let salvaged = read_wallet_recovery_info(stable_prefix()).unwrap();
    let val: serde_json::Value = serde_json::from_str(&salvaged).unwrap();
    assert_eq!(val["birthday"], 2_000_000);
    assert_eq!(val["no_of_accounts"], 1);
    assert_eq!(val["seed_phrase"].as_str().unwrap().split(' ').count(), 24);
}

#[test]
fn the_cut_point_past_the_prefix_does_not_matter() {
    let mut bytes = stable_prefix();
    bytes.extend([0xAB; 100]);
    let with_tail = read_wallet_recovery_info(bytes).unwrap();
    assert_eq!(
        with_tail,
        read_wallet_recovery_info(stable_prefix()).unwrap()
    );
}

#[test]
fn garbage_fails_on_the_read_channel() {
    for garbage in [vec![0x20; 47], vec![0x28; 40]] {
        assert!(matches!(
            read_wallet_recovery_info(garbage),
            Err(ZingolibError::Read(_))
        ));
    }
}

#[test]
fn a_cut_inside_the_prefix_fails_on_the_read_channel() {
    let mut bytes = stable_prefix();
    bytes.truncate(20);
    assert!(matches!(
        read_wallet_recovery_info(bytes),
        Err(ZingolibError::Read(_))
    ));
}
