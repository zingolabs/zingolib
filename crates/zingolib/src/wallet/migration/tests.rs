/// Structural decoupling lint (ZIP 318: a part-transmission session must
/// never synchronize). The migration modules that run on the transmission
/// path must have no dependency edge to the sync engine or the network
/// client. The only permitted pepper-sync surface is wallet data types.
#[test]
fn transmission_path_has_no_sync_dependency() {
    let forbidden = [
        "pepper_sync::sync",
        "sync_and_await",
        "pause_sync",
        "resume_sync",
        "GrpcIndexer",
        "zl_netutils",
    ];
    for module in ["parts", "transmission", "schedule", "store", "reconcile"] {
        let path = format!(
            "{}/src/wallet/migration/{module}.rs",
            env!("CARGO_MANIFEST_DIR")
        );
        let Ok(source) = std::fs::read_to_string(&path) else {
            continue; // modules land tranche by tranche
        };
        for needle in forbidden {
            assert!(
                !source.contains(needle),
                "{path} references `{needle}`: the migration transmission path must remain \
                     decoupled from the sync engine"
            );
        }
    }
}
