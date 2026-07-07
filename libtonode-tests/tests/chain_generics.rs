#[cfg(feature = "chain_generic_tests")]
mod chain_generics {
    /// Deterministic pool matrix, replacing two former proptests.
    ///
    /// The proptests ran a single random-valued case each
    /// (`Config::with_cases(1)`), and their parameter ranges reduced to a
    /// sapling-only source with a transparent-or-sapling receiver, so
    /// coverage differed between runs, failures were not reproducible
    /// without the seed, and the orchard rows the "any source, any
    /// receiver" name promised never ran at all. The matrix below
    /// enumerates every source and receiver pair with fixed values, once
    /// with change and once without, plus boundary values on the
    /// sapling-to-transparent pair. The fixture logic is unchanged, and
    /// nextest parallelizes the entries where the proptests serialized
    /// their work inside two tests.
    #[cfg(feature = "proptests")]
    mod pool_matrix {
        use libtonode_tests::chain_generics::LibtonodeEnvironment;
        use zcash_protocol::{PoolType, ShieldedProtocol};
        use zingolib::testutils::chain_generics::fixtures;

        macro_rules! pool_matrix_case {
            ($name:ident, $source:expr, $receiver:expr, $send_value:expr, $change:expr) => {
                #[tokio::test]
                async fn $name() {
                    fixtures::any_source_sends_to_any_receiver::<LibtonodeEnvironment>(
                        $source,
                        $receiver,
                        $send_value,
                        $change,
                        true,
                    )
                    .await;
                }
            };
        }

        pool_matrix_case!(
            sapling_sends_to_transparent,
            ShieldedProtocol::Sapling,
            PoolType::TRANSPARENT,
            10_000,
            1_000
        );
        pool_matrix_case!(
            sapling_sends_to_sapling,
            ShieldedProtocol::Sapling,
            PoolType::SAPLING,
            10_000,
            1_000
        );
        pool_matrix_case!(
            sapling_sends_to_orchard,
            ShieldedProtocol::Sapling,
            PoolType::ORCHARD,
            10_000,
            1_000
        );
        pool_matrix_case!(
            orchard_sends_to_transparent,
            ShieldedProtocol::Orchard,
            PoolType::TRANSPARENT,
            10_000,
            1_000
        );
        pool_matrix_case!(
            orchard_sends_to_sapling,
            ShieldedProtocol::Orchard,
            PoolType::SAPLING,
            10_000,
            1_000
        );
        pool_matrix_case!(
            orchard_sends_to_orchard,
            ShieldedProtocol::Orchard,
            PoolType::ORCHARD,
            10_000,
            1_000
        );
        pool_matrix_case!(
            sapling_sends_to_transparent_no_change,
            ShieldedProtocol::Sapling,
            PoolType::TRANSPARENT,
            10_000,
            0
        );
        pool_matrix_case!(
            sapling_sends_to_sapling_no_change,
            ShieldedProtocol::Sapling,
            PoolType::SAPLING,
            10_000,
            0
        );
        pool_matrix_case!(
            sapling_sends_to_orchard_no_change,
            ShieldedProtocol::Sapling,
            PoolType::ORCHARD,
            10_000,
            0
        );
        pool_matrix_case!(
            orchard_sends_to_transparent_no_change,
            ShieldedProtocol::Orchard,
            PoolType::TRANSPARENT,
            10_000,
            0
        );
        pool_matrix_case!(
            orchard_sends_to_sapling_no_change,
            ShieldedProtocol::Orchard,
            PoolType::SAPLING,
            10_000,
            0
        );
        pool_matrix_case!(
            orchard_sends_to_orchard_no_change,
            ShieldedProtocol::Orchard,
            PoolType::ORCHARD,
            10_000,
            0
        );
        pool_matrix_case!(
            sapling_sends_to_transparent_minimum_value,
            ShieldedProtocol::Sapling,
            PoolType::TRANSPARENT,
            1,
            0
        );
        pool_matrix_case!(
            sapling_sends_to_transparent_boundary_values,
            ShieldedProtocol::Sapling,
            PoolType::TRANSPARENT,
            49_999,
            9_999
        );
    }
    use libtonode_tests::chain_generics::LibtonodeEnvironment;

    use zingolib::testutils::chain_generics::fixtures;

    #[tokio::test]
    async fn generate_a_range_of_value_transfers() {
        fixtures::create_various_value_transfers::<LibtonodeEnvironment>().await;
    }
    #[tokio::test]
    async fn send_shield_cycle() {
        fixtures::send_shield_cycle::<LibtonodeEnvironment>(1).await;
    }
    #[tokio::test]
    #[test_log::test]
    async fn ignore_dust_inputs() {
        fixtures::ignore_dust_inputs::<LibtonodeEnvironment>().await;
    }
    #[tokio::test]
    async fn note_selection_order() {
        fixtures::note_selection_order::<LibtonodeEnvironment>().await;
    }
}
