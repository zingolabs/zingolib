//! Synthetic example Wallet Files, one per Format Census row.
//!
//! Issue zingolabs/zingolib#2590 enumerates 58 distinguishable wallet
//! grammars, each identified by its Defining Commit hash. Every era module
//! below replicates its rows' grammars from the writer source at those
//! commits (`git show <defining-commit>:<writer-path>`), never from today's
//! code. Each fixture is a minimal wallet that exhibits its row's
//! grammar-unique mark: vectors the mark does not need are empty, fixed-width
//! key and seed material is zeroed, and length-prefixed opaque blobs hold
//! dummy bytes. The fixtures are the recognizer's test corpus; they pin each
//! Discriminator against its neighboring rows.
//!
//! Regenerate with `cargo run --bin wallet-grammar-fixtures` from
//! `tools/workbench/`.

pub mod era_capability;
pub mod era_chainwidth;
pub mod era_inception;
pub mod era_migration;
pub mod era_syncstate;
pub mod era_v32;
pub mod era_zkeys;
pub mod util;

/// One Format Census row's example Wallet File.
pub struct Fixture {
    /// Census row number in issue #2590's table (1 through 58).
    pub row: u8,
    /// The Defining Commit hash — the format's identity.
    pub defining_commit: &'static str,
    /// The branch whose linear history minted the grammar: "dev" or "stable".
    pub branch: &'static str,
    /// The complete example Wallet File.
    pub bytes: Vec<u8>,
}

impl Fixture {
    /// The fixture's file name: zero-padded row number, then the Defining
    /// Commit hash, so a directory listing sorts in census order while the
    /// hash carries the identity.
    pub fn file_name(&self) -> String {
        format!("{:02}_{}.dat", self.row, self.defining_commit)
    }
}

/// The canonical row numbering of issue #2590's table (77-row revision of
/// 2026-07-29). A format's identity is its Defining Commit hash; the row
/// number is presentation order and has already been renumbered once (58 → 77
/// when the item-level sweep recovered 19 sub-record grammars), so the era
/// modules carry hashes and [`all`] assigns numbers from this single table.
const ROW_NUMBERS: [(u8, &str); 77] = [
    (1, "7ebc8686e"),
    (2, "c2e26fbbc"),
    (3, "8ff6d15e3"),
    (4, "5bd8b754d"),
    (5, "db549f5b6"),
    (6, "f532b70ca"),
    (7, "b24f174b5"),
    (8, "0e8ab4d27"),
    (9, "f93267507"),
    (10, "b0f7d8fcf"),
    (11, "b3ca226ff"),
    (12, "df12ccf31"),
    (13, "88a80f574"),
    (14, "ba706ab7c"),
    (15, "e3f972508"),
    (16, "ebf3c7133"),
    (17, "e3a0fd2de"),
    (18, "fc15de568"),
    (19, "72548e077"),
    (20, "796663c97"),
    (21, "cbffd69c6"),
    (22, "fb1135328"),
    (23, "49ee4c406"),
    (24, "8e425fc6b"),
    (25, "28b795139"),
    (26, "b61175345"),
    (27, "bcf38a6fa"),
    (28, "7212e2bf1"),
    (29, "4a279179f"),
    (30, "87ad71c28"),
    (31, "ead95fe0a"),
    (32, "0cd53900b"),
    (33, "a1b9b0bbe"),
    (34, "ed3b21c09"),
    (35, "7f59c5320"),
    (36, "5e73adef4"),
    (37, "a6f8a0bd6"),
    (38, "6dd62d5e2"),
    (39, "2e8b86670"),
    (40, "6b6ed912e"),
    (41, "cc78c2358"),
    (42, "b01873337"),
    (43, "18014a7ee"),
    (44, "939ef32b1"),
    (45, "46eefb844"),
    (46, "b9a984dc8"),
    (47, "a3077c201"),
    (48, "33daec1d1"),
    (49, "9440d190d"),
    (50, "fd86965ea"),
    (51, "eb2210e79"),
    (52, "19f278670"),
    (53, "03c191810"),
    (54, "b82fbe17b"),
    (55, "db3f7f716"),
    (56, "44e6271cb"),
    (57, "8aaae992a"),
    (58, "82c61c0d3"),
    (59, "1ef03610b"),
    (60, "44baa11b4"),
    (61, "ccc1d681a"),
    (62, "e5e4a349f"),
    (63, "e6b02b0d8"),
    (64, "eae34880e"),
    (65, "ad6ded426"),
    (66, "b1c04e38c"),
    (67, "ff7ba3ec0"),
    (68, "f86717800"),
    (69, "eda1dca85"),
    (70, "5d8fda797"),
    (71, "6ae5c270d"),
    (72, "fffcc9e02"),
    (73, "32261bb5f"),
    (74, "4158e20c2"),
    (75, "a6c1354ad"),
    (76, "894fe8e0a"),
    (77, "f48b15c9e"),
];

/// Every authored fixture, renumbered from [`ROW_NUMBERS`] by Defining
/// Commit hash and sorted into census order. Panics if an era module emits a
/// hash the manifest does not know — that is drift between the corpus and
/// the issue table, not a recoverable condition.
pub fn all() -> Vec<Fixture> {
    let mut fixtures = Vec::new();
    fixtures.extend(era_inception::fixtures());
    fixtures.extend(era_zkeys::fixtures());
    fixtures.extend(era_capability::fixtures());
    fixtures.extend(era_v32::fixtures());
    fixtures.extend(era_syncstate::fixtures());
    fixtures.extend(era_chainwidth::fixtures());
    fixtures.extend(era_migration::fixtures());
    for fixture in &mut fixtures {
        let (row, _) = ROW_NUMBERS
            .iter()
            .find(|(_, hash)| *hash == fixture.defining_commit)
            .unwrap_or_else(|| {
                panic!(
                    "defining commit {} is not in the issue #2590 row manifest",
                    fixture.defining_commit
                )
            });
        fixture.row = *row;
    }
    fixtures.sort_by_key(|f| f.row);
    fixtures
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The corpus covers all 77 census rows exactly once, in order, and
    /// every fixture is byte-distinct from every other — the census's core
    /// claim that each row is a distinguishable grammar. (Known exception
    /// under audit: rows 1 and 2 were found byte-identical as grammars; their
    /// fixtures differ by contents until the issue table rules on merging.)
    #[test]
    fn corpus_is_complete_ordered_and_pairwise_distinct() {
        let fixtures = all();
        let rows: Vec<u8> = fixtures.iter().map(|f| f.row).collect();
        let missing: Vec<u8> = (1..=77).filter(|r| !rows.contains(r)).collect();
        assert!(missing.is_empty(), "missing census rows: {missing:?}");
        assert_eq!(rows, (1..=77).collect::<Vec<u8>>());
        for (i, a) in fixtures.iter().enumerate() {
            for b in &fixtures[i + 1..] {
                assert_ne!(
                    a.bytes, b.bytes,
                    "rows {} and {} produced identical bytes",
                    a.row, b.row
                );
            }
        }
    }
}
