# Regtest port plan

Ratified 2026-10-15 in a grilling session; the rulings below are the
user's. The plan carries `docs/testing/regtest-removal-ledger.md`, which
holds the guarantee inventory, from inventory to execution. The decision
record is the zingo-adrs entry "Chain tests run on the mock chain; live
networks are testnet cells" (number assigned by its pull request).

Vocabulary follows `zingolib_testutils/CONTEXT.md`: chain-bound test,
mock chain, twin, guarantee, testnet cell.

## Rulings

1. **The mock chain's validation grows.** `MockChain::validate` gains
   proof verification, spend-authorization and binding-signature
   verification, the shielded value balance, and null-data standardness,
   each behind a `Rules` flag and strict by default, using only the
   `orchard`, `sapling-crypto` and `zcash_proofs` crates already in the
   lock file. "A capacity the mock cannot provide" therefore means
   network policy that is not consensus, and a real indexer's behaviour.
2. **Testnet cells are feature-gated and non-default.** They sit behind
   the `testnet` feature of `libtonode-tests`, outside `extra-credit-tests`,
   compiled on every pull request by the cargo-hack sweep and run by no CI
   profile until the node below exists. No unlinked files.
3. **The node is a zingolabs-operated zainod on a tagged release, serving
   TLS, with NU6.3 active.** It does not exist yet. The fixture keeps one
   constant, `TESTNET_INDEXER`; the gated file's doc bullet names the node
   criterion, and one commit changes the constant and adds the nightly job
   when the node exists. NU6.3 is active on testnet today.
4. **A guarantee neither host can carry is exported or retired.**
   Exported to zaino: indexer latency behind the validator, indexer
   convergence after block generation, and the tip-spend acceptance
   matrix. Retired: the `zcash_local_net` launch contracts (the sentinels
   and `chain_mutates_only_via_owned_rpc`), since the crate is deleted, and
   zebra's subsidy shape, which is not zingolib's to validate. Deletion
   here waits on landing there.
5. **`zingolib_testutils` is deleted entirely.** Its two external
   consumers, zingo-mobile's `*_mobileclient` scenarios on indexer port
   20000 and zaino's `ClientBuilder`, each get an issue naming the revision
   after which the crate is gone, and the final deletion waits until both
   have repointed.
6. **Dependencies: narrow plus shear.** The removal drops exactly the
   crates that become unreachable, `zcash_local_net`, `zingo-consensus`,
   `toml_edit` and `toml_write`, then runs `cargo shear` and removes what
   it reports. A workspace dependency diet is a separate follow-up plan.
   No new dependency is added anywhere in this work.
7. **The unit is the guarantee; a twin never narrows.** Each guarantee
   takes exactly one disposition: mock chain, testnet cell, exported,
   retired. A twin carries every guarantee of the chain-bound test and may
   sharpen. The test's migration is complete when every guarantee has a
   disposition and every mock-chain guarantee has a twin, and the twin
   lands in the commit that deletes the test.
8. **Two cuts.** The container job's shard count follows the census as
   tests leave. Everything else, the container job, the nightly
   `live-originals` input, the `hierarchy-test` libtonode phase, the
   nextest overrides and `regen` profile, `test_binaries/bins`, the
   `sentinels` and `slow` features, and the ADR 0003 amendment, goes in one
   pull request after the last chain-bound test is deleted. The `testnet`
   feature survives the cut.
9. **Parallel start, verification first.** The four verification
   capability commits and the nothing-new twins start together; the
   acceptance-bearing twins follow the verification.
10. **Issue #2591 is rewritten as a testnet cell:** the GloryGoddess
    wallet in Mixnet Mode through a real `nym-proxy` to the zingolabs node,
    asserting the node saw no direct connection, which needs the node to
    expose connection sources.
11. **A ZIP 318 enforcement canary is a testnet cell,** landing with the
    ADR 0009 amendment issue #2539 proposes and closing that issue. The
    mock chain codes the same rule so twins and canary agree.
12. **One decision record** in zingo-adrs, covering the dispositions, the
    verification ruling, the gated feature and node criterion, the export
    rule, and the ADR 0003 and 0009 consequences.
13. **Execution is a stack of pull requests on dev,** in the order of
    section 3, each based on the one before.

## 1. GitHub audit (2026-10-15)

| item | bearing |
| --- | --- |
| PR #2990 "test(libtonode): unify the seven faucet_recipient_default tests into one test", merged 45f4ea0ef | The default census is 29; the six phases port as one twin. |
| PR #2982 "fix(pepper-sync): pause before retrying a request zaino refuses" | Wallet fix for the regtest flake of #2981; stays. |
| PR #2984 "perf(pepper-sync): only construct targeted checkpoints" | The checkpoint-retention twin must stay green across it. |
| PR #2972 "fix(pepper-sync): process the scan results of loads in height order" | Lands before the untracked-history twins. |
| #2534 Regtest launch block diverges the test environment from mainnet startup | Moot with the removal; closes with the cut, ADR 0003 amended. |
| #2050 validator and indexer selection feature gates are not defaulted correctly | Moot; closes with the cut. |
| #2488 Chain-build stall forensics | Chain-cache path only; closes with the cut. |
| #2981 sync after a mined block fails on zaino's snapshot refusal | Fixed by #2982; the regtest reproduction retires with its tests. |
| #2537 Reconcile MAX_REORG_ALLOWANCE | Wallet half stays open; the 150-block cache margin half is moot. Split at the cut. |
| #2715 mempool drain ends a sync session on a timer | Every twin asserting a `Mempool` status waits on the observation. |
| #2447 subtractive darkside_test feature | Fix or confirm fixed before transparent-balance twins land, since twins run in the packages phase. |
| #2539 ADR 0009's turnstile sentence | Ruling 11. |
| #2963, #2964 subtree roots deadlines | Land before the `add_subtree_roots` testnet cell. |
| #2591 libtonode over Nym | Ruling 10. |
| #2930 Delete the zingolib submodule from zingo-mobile | Ruling 5's zingo-mobile gate. |
| #2931 (closed) PR #2911 review findings | No open tracking issue existed; one is opened with this plan. |

## 2. Guarantee dispositions

Census: 37 tests after #2990. 29 default, 5 `#[ignore]`, 3 behind
`sentinels`. Ledger entry numbers in parentheses.

### Mock chain, nothing new (batch 3)

| chain-bound test | guarantees the twin carries |
| --- | --- |
| sync_all_expressible_epochs (46) | A wallet syncs across NU5/NU6 at height 2 and NU6.1/6.2/6.3 at height 5. |
| send_post_ironwood (59) | A default-era funding lands as Ironwood, balance pinned. |
| send_pre_ironwood (58) | With NU6.3 off, funding lands as Orchard, balance pinned. |
| bound_note_reservation_and_external_spend_invalidation (10) | Reserved note untouched by an ordinary send; external spend marks part 0 Invalidated; no replan at the sweep minimum; completion with the disclosed residual. |
| orchard_notes_in_untracked_history_are_recovered (21) and the Ironwood phase of the unified test (20) | Stripped history fails the first sync and recovers on the second; balance equals the pre-strip balance. |
| rescan_still_have_outgoing_notes::self_send (50) | Summaries equal across a rescan after two memo'd self-sends. |
| rescan_still_have_outgoing_notes::check_list_value_transfers_across_rescan (51) | Value transfers and summaries equal across a rescan for the {sapling, orchard} × {memo, memoless} matrix. |
| unified_address_discovery (39) | New unified addresses absent before, discovered after sends to successive indices, across a rescan. Extends `gap_address_compact_block_scanning`. |
| test_scanning_in_watch_only_mode (47) | Each UFVK combination sees the sent values per pool; a watch-only send fails with `NoSpendingKey`. |
| received_tx_status_pending_to_confirmed_with_mempool_monitor (41) | A received transaction is `Mempool(h)` before mining and `Confirmed(h)` after, waited on the observation. |
| mempool_spend_balance_and_note_status_accounting (55) | Three-way Ironwood balance split and per-note `SpendStatus` at every phase of two spend cycles. |
| migrate_all_orchard_to_ironwood (13), immediate_migration_chunks_a_fragmented_wallet (14) | Plan accounting, transaction counts, migrated and residual balances, the two-transaction chunking, the value-transfer sum. Extends `immediate_migration_is_a_migration_value_transfer`. |
| wallet_mempool_record_trails_validator_acceptance (9) | The sender's record reaches `Mempool`. Twin exists; the time bound retires. |
| generate_a_range_of_value_transfers (1), send_shield_cycle (2) | Twins exist and are assertion-identical by construction. |

### Mock chain, after verification (batch 4)

| chain-bound test | guarantees |
| --- | --- |
| faucet_recipient_default_in_one_chain (20, 42, 48, 49, 52, 54, 60) | The six phases as one twin, phase functions keeping their names: faucet setup-height state and back-and-forth balances with minted rewards; the two-Sapling-input spend with Orchard change; the transparent shield's coin status, balances, two new value transfers and rescan equality; the unified address saved by txid across a rescan; the OP_RETURN proposal fees, two txids, one input, two outputs, script and payload; the Ironwood untracked-history recovery. |
| two_phase_migration_end_to_end (12) | Part count, `Complete`, confirmed parts, migrated value, Ironwood balance, value-transfer sum, with NU6.3 deferred past funding. Extends `migrate_to_ironwood_returns_under_continuous_sync`. |

### Testnet cells (batch 5)

| cell | guarantee |
| --- | --- |
| served_outputs_match_chain_metadata_deltas (22) | The indexer's `chain_metadata` deltas equal the outputs it serves, and Ironwood metadata first appears at the activation coinbase. |
| add_subtree_roots (15), comparison half | The real indexer's subtree roots match the wallet's shard roots in count and bytes. The wallet-side half is a twin. The mainnet dial goes. |
| propose_and_send_with_op_return (60), confirmation half | The network's standardness policy accepts the null-data output. |
| ZIP 318 enforcement canary (ruling 11) | The network's verdict on a post-activation Orchard-output payment equals the ADR 0009 expectation. |
| Mixnet Mode over a real proxy (ruling 10) | The node saw no direct connection from the sending wallet. |
| reload_wallet_after_short_sync (62), the four GloryGoddess sends | Already written; un-ignored under the feature. |

### Exported to zaino (batch 6)

indexer_mempool_view_trails_validator_acceptance (8),
indexer_converges_with_validator_after_block_generation (19), and the
eight tip_spend_rejection cells (23 to 30). Each opens as a pull request
in zaino; the zingolib deletion lands after the zaino merge.

### Retired (batch 6)

The three sentinels (5 to 7), chain_mutates_only_via_owned_rpc (4),
sync_test (16), diagnose_subtree_root_stream (18), and the time bounds of
entries 8 and 9. The subsidy guarantees of the former miner-balance tests
are already mock-schedule pins and carry no zebra claim.

## 3. The stack

Each pull request bases on the one before and targets dev when its base
merges.

1. **docs/regtest-port-plan.** This plan and the glossary terms.
2. **test/testnet-conduct-chain.** The completed fixture, the `testnet`
   feature, the gated file, the single-threaded nextest group.
3. **mock verification.** Four capability commits: spend-authorization
   and binding signatures; shielded value balance; proof verification
   (Sapling through `zcash_proofs`, Orchard through `orchard`); null-data
   standardness. Each adds a `Rules` flag, strict by default, and a
   pinning test that a corrupted bundle is rejected.
4. **nothing-new twins.** One commit per chain-bound test in the table
   above, each deleting its test.
5. **acceptance-bearing twins.** The unified chain and the migration
   end-to-end, one commit each.
6. **testnet cells.** Into the gated file, with the ADR 0009 amendment
   and the #2591 rewrite.
7. **exports and retirements.** Retirements delete at once; exports
   delete as each zaino pull request merges. The shard count drops with
   the census.
8. **the cut.** Everything in ruling 8, gated on rulings 4 and 5.

## 4. Follow-ups outside this plan

The workspace dependency diet. The zaino pull requests. The zingo-mobile
repoint off `*_mobileclient`. The nightly job for the `testnet` feature,
when the node exists.

## 5. Status (2026-10-15)

The stack is open on dev, each pull request based on the one before:

1. #2993 the plan and the glossary terms.
2. #2994 the testnet fixture and the gated `testnet` feature.
3. #2995 mock verification of proofs, signatures and null-data standardness.
4. #2996 the fifteen nothing-new twins, one commit each.
5. #2997 the unified chain and the two-phase migration twins.
6. #2998 the testnet cells, including the ZIP 318 canary and the Mixnet Mode cell.
7. #2999 the retirements.
8. The cut, as a draft until the zaino exports land.

The decision record and the ADR 0009 amendment are zingo-adrs #43. The
consumer gate of ruling 5 is already satisfied: neither zingo-mobile nor
zaino names `zingolib_testutils`. Still open: the zaino pull requests for
the exported guarantees, the shard-count timing run of ruling 8, the
nightly job for the `testnet` feature once the node exists, and the
dependency diet of ruling 6 as its own plan.
