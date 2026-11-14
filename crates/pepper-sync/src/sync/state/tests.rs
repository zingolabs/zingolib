use super::*;

/// Where a load's scan results stand once they return, decided from the wallet's scan ranges, the tasks selected
/// after the load's task, the load's task, and the load's own blocks.
mod scan_results_standing {
    use super::*;

    fn blocks(start: u32, end: u32) -> Range<BlockHeight> {
        BlockHeight::from_u32(start)..BlockHeight::from_u32(end)
    }

    /// The wallet after a `Verify` range was carved out of a task's `Scanning` range and selected as a later task.
    fn wallet_with_a_later_task() -> Vec<ScanRange> {
        vec![
            ScanRange::from_parts(blocks(1, 21), ScanPriority::Scanned),
            ScanRange::from_parts(blocks(21, 31), ScanPriority::Scanning),
            ScanRange::from_parts(blocks(31, 41), ScanPriority::Scanning),
        ]
    }

    #[test]
    fn a_load_of_an_earlier_task_is_stale_where_a_later_task_was_selected_over_it() {
        let earlier_task = ScanRange::from_parts(blocks(21, 41), ScanPriority::ChainTip);

        assert_eq!(
            scan_results_standing(
                &wallet_with_a_later_task(),
                &[blocks(31, 41)],
                &earlier_task,
                &blocks(31, 41),
            ),
            ScanResultsStanding::Stale {
                cause: StaleCause::Superseded,
                reset: Vec::new(),
            },
        );
    }

    #[test]
    fn a_load_of_the_later_task_is_current() {
        let later_task = ScanRange::from_parts(blocks(31, 41), ScanPriority::Verify);

        assert_eq!(
            scan_results_standing(
                &wallet_with_a_later_task(),
                &[],
                &later_task,
                &blocks(31, 41)
            ),
            ScanResultsStanding::Current,
        );
    }

    #[test]
    fn a_stale_load_resets_only_the_blocks_no_later_task_holds() {
        let earlier_task = ScanRange::from_parts(blocks(21, 41), ScanPriority::ChainTip);

        assert_eq!(
            scan_results_standing(
                &wallet_with_a_later_task(),
                &[blocks(31, 41)],
                &earlier_task,
                &blocks(28, 41),
            ),
            ScanResultsStanding::Stale {
                cause: StaleCause::Superseded,
                reset: vec![blocks(28, 31)],
            },
        );
    }

    #[test]
    fn a_load_past_the_truncated_tip_is_stale_and_resets_the_blocks_the_wallet_still_holds() {
        let wallet = vec![
            ScanRange::from_parts(blocks(1, 21), ScanPriority::Scanned),
            ScanRange::from_parts(blocks(21, 40), ScanPriority::Scanning),
        ];
        let task = ScanRange::from_parts(blocks(21, 41), ScanPriority::ChainTip);

        assert_eq!(
            scan_results_standing(&wallet, &[], &task, &blocks(31, 41)),
            ScanResultsStanding::Stale {
                cause: StaleCause::Truncated,
                reset: vec![blocks(31, 40)],
            },
        );
        assert_eq!(
            scan_results_standing(&wallet, &[], &task, &blocks(21, 31)),
            ScanResultsStanding::Current,
        );
    }

    #[test]
    fn a_load_below_a_chain_height_that_dropped_past_it_is_stale_with_nothing_to_reset() {
        let wallet = vec![ScanRange::from_parts(blocks(1, 21), ScanPriority::Scanned)];
        let task = ScanRange::from_parts(blocks(21, 22), ScanPriority::Verify);

        assert_eq!(
            scan_results_standing(&wallet, &[], &task, &blocks(21, 22)),
            ScanResultsStanding::Stale {
                cause: StaleCause::Truncated,
                reset: Vec::new(),
            },
        );
    }

    #[test]
    fn a_stale_load_resets_blocks_of_its_own_in_flight_priority_alone() {
        let wallet = vec![
            ScanRange::from_parts(blocks(1, 11), ScanPriority::RefetchingNullifiers),
            ScanRange::from_parts(blocks(11, 22), ScanPriority::Scanning),
        ];
        let refetch_task =
            ScanRange::from_parts(blocks(1, 21), ScanPriority::ScannedWithoutMapping);

        assert_eq!(
            scan_results_standing(&wallet, &[], &refetch_task, &blocks(1, 21)),
            ScanResultsStanding::Stale {
                cause: StaleCause::Reprioritised,
                reset: vec![blocks(1, 11)],
            },
        );

        let mut sync_state = SyncState::new_for_test(wallet);
        sync_state.reset_stale_load(refetch_task.priority(), &[blocks(1, 11)]);
        assert_eq!(
            sync_state.scan_ranges(),
            vec![
                ScanRange::from_parts(blocks(1, 11), ScanPriority::ScannedWithoutMapping),
                ScanRange::from_parts(blocks(11, 22), ScanPriority::Scanning),
            ],
        );
    }

    #[test]
    fn a_load_of_a_reprioritised_range_is_stale_with_nothing_to_reset() {
        let wallet = vec![ScanRange::from_parts(blocks(21, 41), ScanPriority::Verify)];
        let task = ScanRange::from_parts(blocks(21, 41), ScanPriority::ChainTip);

        assert_eq!(
            scan_results_standing(&wallet, &[], &task, &blocks(21, 41)),
            ScanResultsStanding::Stale {
                cause: StaleCause::Reprioritised,
                reset: Vec::new(),
            },
        );
    }
}
