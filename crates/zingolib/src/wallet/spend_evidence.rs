use std::ops::Range;

use pepper_sync::{sync, wallet};
use zcash_protocol::consensus::BlockHeight;

pub(crate) fn ceiling(sync_state: &wallet::SyncState) -> Option<BlockHeight> {
    sync_state.fully_scanned_height()
}

pub(crate) fn floor(
    scan_ranges: &[sync::ScanRange],
    wallet_birthday: Option<BlockHeight>,
    all_spends_known: bool,
) -> Option<BlockHeight> {
    let mut scan_ranges_top_to_bottom = scan_ranges.iter().rev();
    let awaits_spend_detection = |scan_range: &&sync::ScanRange| {
        if all_spends_known {
            scan_range.priority() >= sync::ScanPriority::FoundNote
                || scan_range.priority() == sync::ScanPriority::Scanning
        } else {
            !scan_range.priority().is_scanned()
        }
    };
    let highest_range_awaiting_detection = scan_ranges_top_to_bottom.find(awaits_spend_detection);
    highest_range_awaiting_detection
        .map(|awaiting_range| awaiting_range.block_range().end)
        .or(wallet_birthday)
}

pub(crate) fn note_spends_confirmed(
    note_height: BlockHeight,
    floor: BlockHeight,
    refetch_nullifier_ranges: &[Range<BlockHeight>],
    scan_ranges: &[sync::ScanRange],
) -> bool {
    note_height >= floor
        && refetch_nullifier_ranges.iter().all(|refetch_range| {
            scan_ranges.iter().any(|scan_range| {
                scan_range.priority() == sync::ScanPriority::Scanned
                    && scan_range.block_range().contains(&refetch_range.start)
                    && scan_range.block_range().contains(&(refetch_range.end - 1))
            })
        })
}

impl super::LightWallet {
    pub(crate) fn spend_evidence_floor(&self, all_spends_known: bool) -> Option<BlockHeight> {
        floor(
            self.sync_state.scan_ranges(),
            self.sync_state.wallet_birthday(),
            all_spends_known,
        )
    }

    pub(crate) fn note_spends_confirmed(
        &self,
        note_height: BlockHeight,
        floor: BlockHeight,
        refetch_nullifier_ranges: &[Range<BlockHeight>],
    ) -> bool {
        note_spends_confirmed(
            note_height,
            floor,
            refetch_nullifier_ranges,
            self.sync_state.scan_ranges(),
        )
    }
}

#[cfg(test)]
mod tests {
    use std::ops::Range;

    use pepper_sync::sync;
    use zcash_protocol::consensus::BlockHeight;

    const BIRTHDAY: BlockHeight = BlockHeight::from_u32(0);
    const GAP_START: BlockHeight = BlockHeight::from_u32(100);
    const GAP_END: BlockHeight = BlockHeight::from_u32(200);
    const TIP_END: BlockHeight = BlockHeight::from_u32(300);
    const BELOW_THE_GAP: BlockHeight = BlockHeight::from_u32(50);
    const ABOVE_THE_GAP: BlockHeight = BlockHeight::from_u32(250);

    fn scan_ranges_with_a_gap(gap_priority: sync::ScanPriority) -> [sync::ScanRange; 3] {
        [
            sync::ScanRange::from_parts(BIRTHDAY..GAP_START, sync::ScanPriority::Scanned),
            sync::ScanRange::from_parts(GAP_START..GAP_END, gap_priority),
            sync::ScanRange::from_parts(GAP_END..TIP_END, sync::ScanPriority::Scanned),
        ]
    }

    #[test]
    fn the_floor_is_the_end_of_the_highest_range_that_awaits_spend_detection() {
        let scan_ranges = scan_ranges_with_a_gap(sync::ScanPriority::Historic);

        assert_eq!(
            super::floor(&scan_ranges, Some(BIRTHDAY), false),
            Some(GAP_END)
        );
    }

    #[test]
    fn the_floor_of_a_fully_scanned_wallet_is_its_birthday() {
        let scan_ranges = scan_ranges_with_a_gap(sync::ScanPriority::Scanned);

        assert_eq!(
            super::floor(&scan_ranges, Some(BIRTHDAY), false),
            Some(BIRTHDAY)
        );
    }

    #[test]
    fn known_spends_lower_the_floor_past_a_range_that_holds_no_found_note() {
        let scan_ranges = scan_ranges_with_a_gap(sync::ScanPriority::Historic);

        assert_eq!(
            super::floor(&scan_ranges, Some(BIRTHDAY), true),
            Some(BIRTHDAY)
        );
    }

    #[test]
    fn a_note_below_the_floor_has_no_confirmed_spends() {
        let scan_ranges = scan_ranges_with_a_gap(sync::ScanPriority::Historic);
        let no_refetch_ranges: [Range<BlockHeight>; 0] = [];

        assert!(super::note_spends_confirmed(
            ABOVE_THE_GAP,
            GAP_END,
            &no_refetch_ranges,
            &scan_ranges
        ));
        assert!(!super::note_spends_confirmed(
            BELOW_THE_GAP,
            GAP_END,
            &no_refetch_ranges,
            &scan_ranges
        ));
    }

    #[test]
    fn a_refetch_range_outside_the_scanned_ranges_leaves_spends_unconfirmed() {
        let scan_ranges = scan_ranges_with_a_gap(sync::ScanPriority::Historic);

        assert!(super::note_spends_confirmed(
            ABOVE_THE_GAP,
            GAP_END,
            &[GAP_END..TIP_END],
            &scan_ranges
        ));
        assert!(!super::note_spends_confirmed(
            ABOVE_THE_GAP,
            GAP_END,
            &[GAP_START..GAP_END],
            &scan_ranges
        ));
    }
}
