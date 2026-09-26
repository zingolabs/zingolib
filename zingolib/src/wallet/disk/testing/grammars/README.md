# Format Census example wallets

One synthetic example Wallet File per row of the Format Census table in
[issue #2590](https://github.com/zingolabs/zingolib/issues/2590), which
enumerates the 77 distinguishable grammars the wallet writer has produced
across the linear (first-parent) histories of `dev` and `stable` (the
2026-07-29 revision; an item-level sweep of the full serializer closure
grew the table from its original 58 rows).

Each file is named `NN_<defining-commit>.dat`: `NN` is the census row number
(zero-padded so a directory listing sorts in census order), and the hash is
the row's Defining Commit — the format's identity, per the census's central
finding that the version word does not identify a format. Row numbers are
presentation order and have been renumbered once already; the hash is the
stable key, and the generator assigns numbers from a single manifest in
`tools/workbench/src/wallet_grammars.rs`. Row 70 (`70_5d8fda797.dat`) is
the one stable-only grammar; every other row was minted on dev's line.

These files are synthetic, not archival: each was produced by replicating
the writer source at its Defining Commit (`git show <hash>:<writer-path>`)
in the generator at `tools/workbench/src/wallet_grammars/`, populated
minimally so that the row's grammar-unique mark is present in the bytes.
They contain zeroed dummy key material and hold no value on any network.
Provenance notes, including every assumption made where an encoding came
from a dependency crate, live as doc comments on the per-row functions in
the generator's era modules.

Regenerate with:

```
cd tools/workbench
cargo run --bin wallet-grammar-fixtures
```

The corpus exists to pin Format Recognition: each row's Discriminator must
tell its file apart from the preceding and following rows' files. Two
archaeology results the corpus embodies ahead of the issue text: rows 1
and 2 are byte-identical grammars (`7ebc8686e` already wrote the version
word; their fixtures differ only in contents, pending a table ruling), and
row 20's compressed body is a gzip frame (`libflate`), not zstd.
