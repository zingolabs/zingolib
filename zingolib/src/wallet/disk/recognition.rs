//! Format Recognition (see `zingolib/CONTEXT.md`, Persistence): the pure,
//! total judgment at the front of wallet ingestion that determines which
//! Shipped Format, if any, a candidate Wallet File's bytes conform to,
//! before any field of the file is interpreted.
//!
//! Issue zingolabs/zingolib#2590 is the census this module implements: one
//! enum arm per distinguishable writer grammar in the first-parent histories
//! of dev and stable, identified by its Defining Commit hash — never by the
//! serialized version number, which history reused, skipped, and decreased.
//! Each arm's discriminator structurally parses the entire buffer with
//! bounded reads and no allocation proportional to any claimed length; a
//! grammar conforms only when the parse consumes the buffer exactly.
//!
//! [`recognize`] runs every arm's discriminator and renders the complete
//! verdict: exactly one conformer is a recognition, several is an ambiguity
//! (the load refuses rather than guesses, per the version-42 precedent), and
//! none is a non-conformance carrying each arm's refusal evidence.

pub(crate) mod era_capability;
pub(crate) mod era_inception;
pub(crate) mod era_keys;
pub(crate) mod era_modern;

/// Why one grammar's discriminator refused the bytes: the byte offset the
/// parse died at and what the grammar demanded there.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Refusal {
    /// Byte offset at which the grammar's demand went unmet.
    pub offset: usize,
    /// The grammar's demand at that offset.
    pub expected: &'static str,
}

/// The Recognition Verdict: the complete outcome of Format Recognition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Recognition {
    /// The bytes conform to exactly one Shipped Format.
    Recognized(WalletFormat),
    /// The bytes conform to more than one Shipped Format; the load refuses
    /// rather than guesses, and Recovery Salvage remains available.
    Ambiguous(Vec<WalletFormat>),
    /// The bytes conform to no Shipped Format ever minted; the evidence is
    /// each arm's refusal.
    NotConforming(Vec<(WalletFormat, Refusal)>),
}

/// A bounded reader over the candidate Wallet File's bytes.
///
/// Every length claim is validated against the bytes remaining *before* any
/// use, so a corrupt or misgrammared length degrades to a [`Refusal`] instead
/// of an allocation — the `read_string` 854 PB abort class this module
/// retires. The cursor never allocates; it only hands out subslices.
pub(crate) struct Cursor<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Cursor<'a> {
    pub(crate) fn new(bytes: &'a [u8]) -> Self {
        Cursor { bytes, pos: 0 }
    }

    pub(crate) fn offset(&self) -> usize {
        self.pos
    }

    pub(crate) fn remaining(&self) -> usize {
        self.bytes.len() - self.pos
    }

    /// Refuse at the current offset.
    pub(crate) fn refuse<T>(&self, expected: &'static str) -> Result<T, Refusal> {
        Err(Refusal {
            offset: self.pos,
            expected,
        })
    }

    /// A bounds-checked subslice of exactly `n` bytes.
    pub(crate) fn bytes(&mut self, n: usize, expected: &'static str) -> Result<&'a [u8], Refusal> {
        if self.remaining() < n {
            return self.refuse(expected);
        }
        let s = &self.bytes[self.pos..self.pos + n];
        self.pos += n;
        Ok(s)
    }

    pub(crate) fn u8(&mut self, expected: &'static str) -> Result<u8, Refusal> {
        Ok(self.bytes(1, expected)?[0])
    }

    pub(crate) fn u16_le(&mut self, expected: &'static str) -> Result<u16, Refusal> {
        let b = self.bytes(2, expected)?;
        Ok(u16::from_le_bytes([b[0], b[1]]))
    }

    pub(crate) fn u32_le(&mut self, expected: &'static str) -> Result<u32, Refusal> {
        let b = self.bytes(4, expected)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    pub(crate) fn u64_le(&mut self, expected: &'static str) -> Result<u64, Refusal> {
        let b = self.bytes(8, expected)?;
        Ok(u64::from_le_bytes([
            b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7],
        ]))
    }

    pub(crate) fn i32_le(&mut self, expected: &'static str) -> Result<i32, Refusal> {
        Ok(self.u32_le(expected)? as i32)
    }

    pub(crate) fn i64_le(&mut self, expected: &'static str) -> Result<i64, Refusal> {
        Ok(self.u64_le(expected)? as i64)
    }

    /// An exact little-endian u64 (the wallet-level version word).
    pub(crate) fn exact_u64(&mut self, want: u64, expected: &'static str) -> Result<(), Refusal> {
        let at = self.pos;
        if self.u64_le(expected)? != want {
            return Err(Refusal {
                offset: at,
                expected,
            });
        }
        Ok(())
    }

    /// An exact single byte (sub-record version bytes, discriminants).
    pub(crate) fn exact_u8(&mut self, want: u8, expected: &'static str) -> Result<(), Refusal> {
        let at = self.pos;
        if self.u8(expected)? != want {
            return Err(Refusal {
                offset: at,
                expected,
            });
        }
        Ok(())
    }

    /// A canonically-encoded Zcash CompactSize whose value fits the bytes
    /// remaining. Non-minimal encodings refuse: no shipped writer emits them.
    pub(crate) fn compact_size(&mut self, expected: &'static str) -> Result<usize, Refusal> {
        let at = self.pos;
        let tag = self.u8(expected)?;
        let n: u64 = match tag {
            0..=252 => u64::from(tag),
            253 => {
                let v = u64::from(self.u16_le(expected)?);
                if v < 253 {
                    return Err(Refusal {
                        offset: at,
                        expected,
                    });
                }
                v
            }
            254 => {
                let v = u64::from(self.u32_le(expected)?);
                if v <= u64::from(u16::MAX) {
                    return Err(Refusal {
                        offset: at,
                        expected,
                    });
                }
                v
            }
            255 => {
                let v = self.u64_le(expected)?;
                if v <= u64::from(u32::MAX) {
                    return Err(Refusal {
                        offset: at,
                        expected,
                    });
                }
                v
            }
        };
        usize::try_from(n)
            .ok()
            .filter(|n| *n <= self.remaining())
            .ok_or(Refusal {
                offset: at,
                expected,
            })
    }

    /// A u64 length claim, validated against the bytes remaining.
    pub(crate) fn u64_len(&mut self, expected: &'static str) -> Result<usize, Refusal> {
        let at = self.pos;
        let n = self.u64_le(expected)?;
        usize::try_from(n)
            .ok()
            .filter(|n| *n <= self.remaining())
            .ok_or(Refusal {
                offset: at,
                expected,
            })
    }

    /// The repo's `write_string` form: u64 length + that many bytes.
    pub(crate) fn u64_string(&mut self, expected: &'static str) -> Result<&'a [u8], Refusal> {
        let n = self.u64_len(expected)?;
        self.bytes(n, expected)
    }

    /// A `zcash_encoding::Vector`: CompactSize count, then `count` elements
    /// parsed by `f`.
    pub(crate) fn compact_vec(
        &mut self,
        expected: &'static str,
        mut f: impl FnMut(&mut Cursor<'a>) -> Result<(), Refusal>,
    ) -> Result<(), Refusal> {
        let n = self.compact_size(expected)?;
        for _ in 0..n {
            f(self)?;
        }
        Ok(())
    }

    /// A `Vector` of raw bytes: CompactSize count + that many bytes.
    pub(crate) fn compact_vec_u8(&mut self, expected: &'static str) -> Result<&'a [u8], Refusal> {
        let n = self.compact_size(expected)?;
        self.bytes(n, expected)
    }

    /// A `zcash_encoding::Optional`: u8 0 (absent) or 1 (present, then `f`).
    pub(crate) fn optional(
        &mut self,
        expected: &'static str,
        f: impl FnOnce(&mut Cursor<'a>) -> Result<(), Refusal>,
    ) -> Result<(), Refusal> {
        let at = self.pos;
        match self.u8(expected)? {
            0 => Ok(()),
            1 => f(self),
            _ => Err(Refusal {
                offset: at,
                expected,
            }),
        }
    }

    /// Conformance demands the grammar consume the buffer exactly.
    pub(crate) fn finish(&self) -> Result<(), Refusal> {
        if self.remaining() != 0 {
            return self.refuse("end of file (trailing bytes refuse the grammar)");
        }
        Ok(())
    }
}

macro_rules! wallet_formats {
    ($( $(#[doc = $doc:literal])+ $variant:ident = ($hash:literal, $parser:path), )+) => {
        /// Every distinguishable wallet grammar ever minted (issue #2590),
        /// one arm per Format Census row, identified by Defining Commit.
        ///
        /// Rows 1 and 2 of the census are byte-identical grammars
        /// (`7ebc8686e` already wrote the u64 version word 1; `c2e26fbbc`
        /// only refactored the literal), so they share the single arm
        /// [`WalletFormat::F7ebc8686e`]: indistinguishable writer states
        /// share an arm.
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        #[allow(non_camel_case_types)]
        pub(crate) enum WalletFormat {
            $( $(#[doc = $doc])+ $variant, )+
        }

        impl WalletFormat {
            /// All arms, census order.
            pub(crate) const ALL: &'static [WalletFormat] =
                &[ $( WalletFormat::$variant, )+ ];

            /// The Defining Commit hash — the format's identity.
            pub(crate) fn defining_commit(&self) -> &'static str {
                match self {
                    $( WalletFormat::$variant => $hash, )+
                }
            }

            /// The arm's discriminator: a full structural parse of the
            /// entire buffer under this grammar.
            pub(crate) fn discriminator(&self) -> fn(&[u8]) -> Result<(), Refusal> {
                match self {
                    $( WalletFormat::$variant => $parser, )+
                }
            }
        }
    };
}

wallet_formats! {
    /// Rows 1–2 (2019-09-06): the writer's birth; u64 version word 1. Also
    /// covers `c2e26fbbc` (byte-identical grammar).
    F7ebc8686e = ("7ebc8686e", era_inception::parse_7ebc8686e),
    /// Row 3 (2019-09-06): note `is_change`; txid and shielded-spent in tx.
    F8ff6d15e3 = ("8ff6d15e3", era_inception::parse_8ff6d15e3),
    /// Row 4 (2019-09-07): raw seed + ExtSK vector.
    F5bd8b754d = ("5bd8b754d", era_inception::parse_5bd8b754d),
    /// Row 5 (2019-09-13): tx `total_transparent_value_spent`.
    Fdb549f5b6 = ("db549f5b6", era_inception::parse_db549f5b6),
    /// Row 6 (2019-09-13): standalone Utxo vector.
    Ff532b70ca = ("f532b70ca", era_inception::parse_f532b70ca),
    /// Row 7 (2019-09-16): raw 32-byte tkey.
    Fb24f174b5 = ("b24f174b5", era_inception::parse_b24f174b5),
    /// Row 8 (2019-09-17): Utxos move into the tx record.
    F0e8ab4d27 = ("0e8ab4d27", era_inception::parse_0e8ab4d27),
    /// Row 9 (2019-09-17): Utxo drops unconfirmed-spent.
    Ff93267507 = ("f93267507", era_inception::parse_f93267507),
    /// Row 10 (2019-09-19): tx v2, outgoing metadata.
    Fb0f7d8fcf = ("b0f7d8fcf", era_inception::parse_b0f7d8fcf),
    /// Row 11 (2019-09-24): version word 2; chain-name string.
    Fb3ca226ff = ("b3ca226ff", era_inception::parse_b3ca226ff),
    /// Row 12 (2019-09-25): tx `full_tx_scanned`.
    Fdf12ccf31 = ("df12ccf31", era_inception::parse_df12ccf31),
    /// Row 13 (2019-09-27): birthday u64.
    F88a80f574 = ("88a80f574", era_inception::parse_88a80f574),
    /// Row 14 (2019-10-01): tkeys become a Vector.
    Fba706ab7c = ("ba706ab7c", era_inception::parse_ba706ab7c),
    /// Row 15 (2019-10-01): version word 3 (word-only; tx inner 2→3).
    Fe3f972508 = ("e3f972508", era_inception::parse_e3f972508),
    /// Row 16 (2019-10-18): tx v4, datetime.
    Febf3c7133 = ("ebf3c7133", era_inception::parse_ebf3c7133),
    /// Row 17 (2019-10-18): version word 4; locked byte + FVK vector.
    Fe3a0fd2de = ("e3a0fd2de", era_inception::parse_e3a0fd2de),
    /// Row 18 (2019-10-19): taddress vector.
    Ffc15de568 = ("fc15de568", era_inception::parse_fc15de568),
    /// Row 19 (2019-10-19): enc_seed + nonce vector.
    F72548e077 = ("72548e077", era_inception::parse_72548e077),
    /// Row 20 (2020-04-12): version word 5; gzip-compressed body.
    F796663c97 = ("796663c97", era_inception::parse_796663c97),
    /// Row 21 (2020-05-09): version word 6; plaintext restored.
    Fcbffd69c6 = ("cbffd69c6", era_inception::parse_cbffd69c6),
    /// Row 22 (2020-07-21): version word 7; WalletZKey vector.
    Ffb1135328 = ("fb1135328", era_keys::parse_fb1135328),
    /// Row 23 (2020-07-21): version word 8; note spent_at_height.
    F49ee4c406 = ("49ee4c406", era_keys::parse_49ee4c406),
    /// Row 24 (2020-08-24): version word 9; note is_spendable.
    F8e425fc6b = ("8e425fc6b", era_keys::parse_8e425fc6b),
    /// Row 25 (2020-10-15): version word 10; tagged rseed.
    F28b795139 = ("28b795139", era_keys::parse_28b795139),
    /// Row 26 (2020-12-01): version word 12; Utxo spent_at_height.
    Fb61175345 = ("b61175345", era_keys::parse_b61175345),
    /// Row 27 (2021-04-22): version word 13; tree_verified byte.
    Fbcf38a6fa = ("bcf38a6fa", era_keys::parse_bcf38a6fa),
    /// Row 28 (2021-05-05): note v5 / Utxo v3 pending-spent.
    F7212e2bf1 = ("7212e2bf1", era_keys::parse_7212e2bf1),
    /// Row 29 (2021-05-18): version word 14; price record; tx v5.
    F4a279179f = ("4a279179f", era_keys::parse_4a279179f),
    /// Row 30 (2021-06-25): version word 20; the Keys record (v20).
    F87ad71c28 = ("87ad71c28", era_keys::parse_87ad71c28),
    /// Row 31 (2021-07-14): version word 21; tx unconfirmed byte.
    Fead95fe0a = ("ead95fe0a", era_keys::parse_ead95fe0a),
    /// Row 32 (2021-07-27): version word 22; Optional TreeState.
    F0cd53900b = ("0cd53900b", era_keys::parse_0cd53900b),
    /// Row 33 (2021-07-27, reminted 2021-08-05): version word 23.
    Fa1b9b0bbe = ("a1b9b0bbe", era_keys::parse_a1b9b0bbe),
    /// Row 34 (2021-07-29): version word 24; compact-encoded blocks.
    Fed3b21c09 = ("ed3b21c09", era_keys::parse_ed3b21c09),
    /// Row 35 (2021-09-24): version word 24; WalletOptions record.
    F7f59c5320 = ("7f59c5320", era_keys::parse_7f59c5320),
    /// Row 36 (2021-10-13): Keys v21 (WalletTKey vector).
    F5e73adef4 = ("5e73adef4", era_keys::parse_5e73adef4),
    /// Row 37 (2022-07-23): tx v22 (pool triple + orchard nullifiers).
    Fa6f8a0bd6 = ("a6f8a0bd6", era_keys::parse_a6f8a0bd6),
    /// Row 38 (2022-08-23): Keys v22 (WalletOKey vector); tx v23.
    F6dd62d5e2 = ("6dd62d5e2", era_keys::parse_6dd62d5e2),
    /// Row 39 (2022-09-16): WalletOptions v2 (size filter).
    F2e8b86670 = ("2e8b86670", era_keys::parse_2e8b86670),
    /// Row 40 (2022-10-14): version word 25; orchard-anchor vector.
    F6b6ed912e = ("6b6ed912e", era_capability::parse_6b6ed912e),
    /// Row 41 (2022-10-26): WalletCapability v1 with trailing encrypted u8.
    Fcc78c2358 = ("cc78c2358", era_capability::parse_cc78c2358),
    /// Row 42 (2022-11-08): WalletCapability v1, encrypted byte trimmed.
    Fb01873337 = ("b01873337", era_capability::parse_b01873337),
    /// Row 43 (2022-11-16): version word 26; anchor vector removed.
    F18014a7ee = ("18014a7ee", era_capability::parse_18014a7ee),
    /// Row 44 (2023-02-28): version word 27; capability v2.
    F939ef32b1 = ("939ef32b1", era_capability::parse_939ef32b1),
    /// Row 45 (2023-04-19): note v2 (FVK removed).
    F46eefb844 = ("46eefb844", era_capability::parse_46eefb844),
    /// Row 46 (2023-06-13): note v3 (pending-spent dropped).
    Fb9a984dc8 = ("b9a984dc8", era_capability::parse_b9a984dc8),
    /// Row 47 (2023-08-19): note v4; WitnessTrees enter the file.
    Fa3077c201 = ("a3077c201", era_capability::parse_a3077c201),
    /// Row 48 (2023-09-28): version word 28; mnemonic account index.
    F33daec1d1 = ("33daec1d1", era_capability::parse_33daec1d1),
    /// Row 49 (2023-11-08): transparent output v4.
    F9440d190d = ("9440d190d", era_capability::parse_9440d190d),
    /// Row 50 (2024-10-07): version word 29; capability v3.
    Ffd86965ea = ("fd86965ea", era_capability::parse_fd86965ea),
    /// Row 51 (2024-10-15): version word 30; capability v4.
    Feb2210e79 = ("eb2210e79", era_capability::parse_eb2210e79),
    /// Row 52 (2024-10-24): note v5; ConfirmationStatus enters (v0).
    F19f278670 = ("19f278670", era_capability::parse_19f278670),
    /// Row 53 (2024-11-07): OutgoingTxData v0; tx record 24.
    F03c191810 = ("03c191810", era_capability::parse_03c191810),
    /// Row 54 (2025-02-02): version word 31; block vector dropped.
    Fb82fbe17b = ("b82fbe17b", era_capability::parse_b82fbe17b),
    /// Row 55 (2025-02-06): version word 31; key store dropped.
    Fdb3f7f716 = ("db3f7f716", era_capability::parse_db3f7f716),
    /// Row 56 (2025-03-17): version word 32; the v32 layout.
    F44e6271cb = ("44e6271cb", era_modern::parse_44e6271cb),
    /// Row 57 (2025-04-27): version word 32; vestigial tail dropped.
    F8aaae992a = ("8aaae992a", era_modern::parse_8aaae992a),
    /// Row 58 (2025-05-01): version word 33; SyncConfig.
    F82c61c0d3 = ("82c61c0d3", era_modern::parse_82c61c0d3),
    /// Row 59 (2025-05-15): version word 34; PriceList with api key.
    F1ef03610b = ("1ef03610b", era_modern::parse_1ef03610b),
    /// Row 60 (2025-05-23): version word 34; api key dropped, unbumped.
    F44baa11b4 = ("44baa11b4", era_modern::parse_44baa11b4),
    /// Row 61 (2025-05-27): version word 35; account-keyed key store.
    Fccc1d681a = ("ccc1d681a", era_modern::parse_ccc1d681a),
    /// Row 62 (2025-06-04): ReceiverSelection v2.
    Fe5e4a349f = ("e5e4a349f", era_modern::parse_e5e4a349f),
    /// Row 63 (2025-06-04): version word 36 (word-only).
    Fe6b02b0d8 = ("e6b02b0d8", era_modern::parse_e6b02b0d8),
    /// Row 64 (2025-06-13): version word 37; ScanTarget; SyncState v1.
    Feae34880e = ("eae34880e", era_modern::parse_eae34880e),
    /// Row 65 (2025-06-15): version word 38; min_confirmations.
    Fad6ded426 = ("ad6ded426", era_modern::parse_ad6ded426),
    /// Row 66 (2025-06-18): version word 39; SyncState v2.
    Fb1c04e38c = ("b1c04e38c", era_modern::parse_b1c04e38c),
    /// Row 67 (2026-02-12): version word 39; SyncState v3.
    Fff7ba3ec0 = ("ff7ba3ec0", era_modern::parse_ff7ba3ec0),
    /// Row 68 (2026-02-18): ConfirmationStatus v1.
    Ff86717800 = ("f86717800", era_modern::parse_f86717800),
    /// Row 69 (2026-03-25): version word 40, dev: chain-type u8.
    Feda1dca85 = ("eda1dca85", era_modern::parse_eda1dca85),
    /// Row 70 (2026-06-07, stable): version word 40: chain string, u32
    /// output indices.
    F5d8fda797 = ("5d8fda797", era_modern::parse_5d8fda797),
    /// Row 71 (2026-06-15): version word 41: chain u8 + u32 indices.
    F6ae5c270d = ("6ae5c270d", era_modern::parse_6ae5c270d),
    /// Row 72 (2026-07-13): version word 42, layout A (allow_v6 byte).
    Ffffcc9e02 = ("fffcc9e02", era_modern::parse_fffcc9e02),
    /// Row 73 (2026-07-14): version word 43.
    F32261bb5f = ("32261bb5f", era_modern::parse_32261bb5f),
    /// Row 74 (2026-07-14): version word 42, canonical layout.
    F4158e20c2 = ("4158e20c2", era_modern::parse_4158e20c2),
    /// Row 75 (2026-07-23): migration inner v2.
    Fa6c1354ad = ("a6c1354ad", era_modern::parse_a6c1354ad),
    /// Row 76 (2026-07-25): migration inner v3.
    F894fe8e0a = ("894fe8e0a", era_modern::parse_894fe8e0a),
    /// Row 77 (2026-07-25): migration inner v4. Today's writer.
    Ff48b15c9e = ("f48b15c9e", era_modern::parse_f48b15c9e),
}

/// Format Recognition: classify a complete Wallet File byte string to its
/// unique source grammar.
///
/// Pure and total: every byte string maps to exactly one
/// [`Recognition`]. Runs every arm's discriminator over the whole buffer;
/// no prefix or single discriminator byte is ever sufficient evidence
/// (version 42's discriminating evidence sits at end of file).
pub(crate) fn recognize(bytes: &[u8]) -> Recognition {
    let mut conformers = Vec::new();
    let mut refusals = Vec::new();
    for format in WalletFormat::ALL {
        match (format.discriminator())(bytes) {
            Ok(()) => conformers.push(*format),
            Err(refusal) => refusals.push((*format, refusal)),
        }
    }
    match conformers.len() {
        0 => Recognition::NotConforming(refusals),
        1 => Recognition::Recognized(conformers[0]),
        _ => Recognition::Ambiguous(conformers),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The fixture corpus (written by `wallet-grammar-fixtures` into
    /// `disk/testing/grammars/NN_<defining-commit>.dat`) pins every
    /// discriminator against every other arm: each fixture must recognize
    /// as exactly its own row's format. Rows 1 and 2 share the merged
    /// `F7ebc8686e` arm. Skips silently when the corpus has not been
    /// generated yet.
    #[test]
    fn corpus_recognizes_uniquely() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("src/wallet/disk/testing/grammars");
        let mut checked = 0usize;
        let Ok(entries) = std::fs::read_dir(&dir) else {
            return;
        };
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name = name.to_string_lossy().into_owned();
            let Some(stem) = name.strip_suffix(".dat") else {
                continue;
            };
            let Some((_, hash)) = stem.split_once('_') else {
                continue;
            };
            // Rows 1 and 2 are one arm: c2e26fbbc's fixture recognizes as
            // the merged 7ebc8686e arm.
            let expected = if hash == "c2e26fbbc" { "7ebc8686e" } else { hash };
            let bytes = std::fs::read(entry.path()).unwrap();
            match recognize(&bytes) {
                Recognition::Recognized(format) => {
                    assert_eq!(
                        format.defining_commit(),
                        expected,
                        "fixture {name} recognized as the wrong arm"
                    );
                }
                other => panic!("fixture {name} did not recognize uniquely: {other:?}"),
            }
            checked += 1;
        }
        // When the corpus exists it must be complete.
        if checked > 0 {
            assert!(checked >= 77, "corpus present but only {checked} fixtures");
        }
    }

    /// The motivating crash input: the first bytes of a dev-v40 wallet.
    /// Under the census it must classify to dev's v40 arm — and whatever
    /// the verdict, recognition must not allocate or abort on the length
    /// field that killed the reader.
    #[test]
    fn dev_v40_prefix_refuses_stable_v40_without_allocating() {
        // version word 40, chain byte 0 (Mainnet), CompactSize-32 seed,
        // truncated: enough to prove the stable-v40 arm refuses early and
        // cheaply rather than trusting a 854 PB length.
        let mut bytes = 40u64.to_le_bytes().to_vec();
        bytes.push(0);
        bytes.push(32);
        bytes.extend_from_slice(&[0x55; 32]);
        let refusal = (WalletFormat::F5d8fda797.discriminator())(&bytes)
            .expect_err("a dev-v40 prefix must refuse the stable-v40 grammar");
        assert_eq!(refusal.offset, 8, "stable v40 demands a string length at offset 8");
    }
}
