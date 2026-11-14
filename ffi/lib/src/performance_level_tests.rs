use super::*;

#[test]
fn every_listed_level_round_trips_through_its_name() {
    for (level, name) in PERFORMANCE_LEVELS {
        assert_eq!(performance_level_name(level).unwrap(), name);
        assert_eq!(performance_level(name), Some(level));
    }
}

#[test]
fn a_name_outside_the_table_is_rejected() {
    assert_eq!(performance_level("Ultra"), None);
}
