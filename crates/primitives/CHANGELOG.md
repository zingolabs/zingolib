# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added
- The crate holds the domain vocabulary a Zcash chain indexer's replies parse
  into, split out of `zl-common` so that a crate speaking only the wire
  protocol can name these types without the wallet stack (`orchard`,
  `sapling-crypto`, `zcash_keys`, ...) that `zl-common` carries for its own,
  larger job. `zl-common` re-exports every item at its old path
  (`zl_common::chain::X`).
- `chain` module: `BlockHash`, `BlockId`, `BlockRange`, `ChainMetadata`,
  `ChainType`, `CompactBlock`, `CompactOrchardAction`, `CompactSaplingOutput`,
  `CompactSaplingSpend`, `CompactTx`, `CompactTxIn`, `GetAddressUtxosArg`,
  `GetAddressUtxosReply`, `GetSubtreeRootsArg`, `LightdInfo`, `RawTransaction`,
  `RawTransactionStatus`, `StandardFee`, `SubtreeRoot`,
  `TransparentAddressBlockFilter`, `TreeState`, `TxFilter`, `TxOut`, each
  owning its own wire-boundary parsing and invariants, and their `Invalid*`
  error types.
