use super::*;

/// Each reader is given only a serialized version above the one its type writes. A reader that refuses the
/// version returns invalid data. A reader that read on would report the end of the input.
#[test]
fn readers_refuse_serialized_versions_above_their_own() {
    let newer_sync_config = [SyncConfig::serialized_version() + 1];
    let newer_performance_level = [PerformanceLevel::serialized_version() + 1];

    for (type_name, read) in [
        (
            "SyncConfig",
            SyncConfig::read(newer_sync_config.as_slice()).map(drop),
        ),
        (
            "PerformanceLevel",
            PerformanceLevel::read(newer_performance_level.as_slice()).map(drop),
        ),
    ] {
        assert_eq!(
            read.expect_err(type_name).kind(),
            std::io::ErrorKind::InvalidData,
            "{type_name}"
        );
    }
}
