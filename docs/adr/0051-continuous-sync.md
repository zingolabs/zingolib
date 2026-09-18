# Continuous sync keeps the wallet current as new blocks are mined

Status: accepted

## Context

The sync engine treated synchronization as a bounded operation. It learned a
chain tip, scanned until the wallet reached that height, and then returned. A
wallet that remained open therefore did not learn about blocks mined
afterwards without starting another sync.

Long-lived wallet sessions need the sync engine to remain at the tip and scan
new blocks as they appear.

The existing mempool stream provides a useful signal for this. Its closure can
coincide with a newly mined block, but it is not itself proof that the chain
advanced, so continuous sync also needs a periodic chain-height check.

Transparent scanning needs different handling once the initial sync is
complete. Re-running the transparent RPCs for every watched address after each
new block would repeat expensive discovery work. By then, however, the engine
already knows the in-use addresses and the gap addresses through the configured
gap limit. Newly mined blocks can therefore check their compact-block
transparent data against that known set.

Continuous sync also changes the meaning of reaching the chain tip. Completion
and shutdown can no longer be the same event, so the engine needs one shutdown
path shared by automatic and explicit termination.

## Decision

SyncConfig gains a shutdown_on_completion boolean.

When it is true, reaching the current chain tip keeps the existing behaviour:
the sync engine shuts down and sync returns.

When it is false, reaching the tip leaves the sync engine running. It checks
for new blocks, updates the wallet's last known chain height, and scans any
blocks added since the previous check.

shutdown_on_completion controls what happens once sync is complete; it does
not freeze the target height observed when the session started. While the sync
session is still active, newly mined blocks are detected and scanned even when
shutdown_on_completion is true.

New-block checks are triggered in two ways:

the mempool monitor reports internally that its stream closed; and
the sync engine checks the chain every ten seconds.

The mempool monitor now sends an internal MempoolMessage, containing either
a transaction or a stream-closed indication. A closed stream causes the sync
engine to check the chain height; only the observed height determines whether
there are new blocks to scan.

For newly mined blocks, transparent transaction detection uses the transparent
data in the compact blocks and matches it against the already known in-use and
gap addresses. This is limited to continuous sync, where blocks are consumed
as the chain advances. Historical sync continues to use the existing
transparent RPCs for address discovery.

Shutdown is unified after the scan loop. Whether the caller explicitly stops
the engine or shutdown_on_completion causes it to stop after reaching the
tip, the same cleanup runs before sync returns.

## Considered options

Poll the chain height without using the mempool stream. Rejected: periodic
polling is still required for correctness, but the existing stream provides a
useful signal for checking sooner.

Re-run the transparent RPCs for every newly mined block. Rejected: address
discovery has already established the set that needs to be watched, making
repeated per-address queries unnecessary.

## Consequences

A sync can now remain active for the lifetime of a wallet session and process
blocks mined after the initial catch-up without restarting the sync engine.

Continuous sync does not depend on mempool-stream closure for correctness:
stream closure prompts an immediate check, while the ten-second interval
provides the fallback.

Transparent scanning of newly mined blocks avoids repeating the expensive
transparent RPC discovery while leaving the historical sync strategy
unchanged.

Reaching the chain tip no longer necessarily means that sync returns.
Callers which want bounded synchronization use shutdown_on_completion;
long-lived sessions leave it disabled and stop the engine explicitly.

All sync termination now passes through one shutdown path.
