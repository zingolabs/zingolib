# Indexers are classified by role, trust, and location, and one broadcast rule draws from them

Status: draft — ruled in sessions 2026-09-11 and 2026-09-14, pending review

## Context

A testnet send over the mixnet failed every time. The Destination draw
read a hand-curated list, `DESTINATION_INDEXERS`, that held only the
mainnet survivors of the 2026-07-21 discovery sweep, and neither the list
nor the draw took a chain. A testnet wallet built a testnet transaction and
raced it against six mainnet lightwalletd hosts, each of which rejected it
after the resilience policy's retries, so the send surfaced an
`AllFailed` escalation minutes later naming six mainnet hosts.

The defect was structural rather than a missing entry. Two lists described
"which indexers exist": the indexer registry in `zingo-netutils`, which
carries a chain per entry and feeds the Server-Selection Sweep, and the
Destination list in `zingolib`, a frozen snapshot with no chain that fed
the send. They had already drifted: two of the eleven Destination hosts
were absent from the registry. The draw also fused the ADR 0022 exclusion
with its source, so a chain whose registry holds one operator (testnet
holds `zec.rocks` alone) could only refuse.

A first answer, drawn from the registry per chain and rotated on every
transport, fixed testnet and broke four other cases. With the mixnet off,
a self-hoster's own node stopped receiving their sends, which raced across
public operators instead; every build without the `nym` feature did the
same on every send; a migration target on the user's own node was refused
outright; and a testnet self-hoster's sends reached the public server
first. Rotation on clearnet also bought nothing: the sync indexer learns
each of the wallet's sends once it is mined, because the sync engine
fetches the transaction by txid after its change note decrypts, and on
clearnet it already holds the wallet's IP. A second clearnet Destination
only added one more party holding the transaction beside the IP.

The four cases differed along properties the draw could not see: whether
the user trusts an indexer, what the user uses it for, and whether it runs
on the user's own network.

## Decision

Every indexer is classified by three properties, and one rule, the same on
every transport and every network, draws a broadcast's Destinations from
them.

**Role** says what the indexer is used for: `Sync`, `Broadcast`, or
`SyncAndBroadcast`. A session has at most one sync indexer and any number
of broadcast indexers. `migration_transmission_uri` keeps its meaning as a
migration-specific broadcast target.

**Trust** says whether the indexer's operator may link the wallet to its
transactions: `Trusted` or `Untrusted`. A trusted operator may learn
everything, the wallet's IP included, so transport stops mattering for it.

**Location** says whether the indexer runs on the wallet's machine or
local network: `Local` or `Remote`, read from the URI's host. A local
indexer shares the wallet's public IP, so its node announces a transaction
to Zcash peers from that IP, and the mixnet cannot reach it: an exit node
dialing a loopback or private address reaches its own network.

A consumer states role and trust per endpoint through `IndexerConfig`,
passed to `ClientConfigBuilder::add_indexer` or `LightClient::add_indexer`.
An entry for the sync indexer's endpoint classifies the sync indexer; an
entry with a broadcast role and another endpoint adds a broadcast
indexer. Anything left unstated takes a default: a local indexer is
trusted, a remote one takes the network's remote default, and every
indexer serves both roles. The network's remote default is one value,
`Trust::remote_default`: `Untrusted` on mainnet, `Trusted` on testnet and
regtest. A consumer overrides it with `set_remote_indexer_trust`, so the
library names a behaviour and the user decides where it applies.

**The broadcast rule.**

1. The candidates are the configured broadcast indexers, the sync indexer
   when its role broadcasts, and, over the mixnet only, the registry for
   the chain. Over the mixnet an untrusted sync indexer is no candidate,
   and neither is any untrusted entry run by its operator, compared by
   operator as ADR 0022 rules. Every candidate must be reachable over the
   transport: a mixnet candidate is remote, https, and on port 443.
2. When a trusted candidate remains, the race runs among the trusted
   candidates alone. If they fail, the send fails typed; it never falls
   through to public operators.
3. Otherwise it runs among the untrusted ones: the configured broadcast
   indexers first, in the order given, then the rest in random order.

Over clearnet the registry is never drawn, so a clearnet send reaches only
indexers the session names, which by default is the sync indexer, as it
was before this decision. Migration parts use the same draw, one random
candidate per part; a configured `migration_transmission_uri` is used
alone, and over the mixnet it is refused when it shares an operator with
an untrusted sync indexer.

`DestinationServerSet` in `zingolib::destination::servers` owns the
classification, the registry for the chain, and the rule. It is built
when a session opens from the registry, the chain, and the consumer's
configuration, it is never written to disk, and it reads the sync indexer
at draw time, so a session that rebinds it never draws against a stale
classification. The hedged race planner gains a preferred head: the first
`preferred` Destinations are contacted in the order given and only the
rest are shuffled.

Supporting moves: the race orchestrator leaves the nym-gated mixnet module
for `destination::rotation`; the Exit Pool leaves `destination::pool` for
`mixnet::pools`, since it was never about Destinations; the registry gains
the two hosts the hand list carried and the registry lacked; failure
attribution is renamed in engineering terms, `charge_phase` and
`FailurePhase` becoming `fault_domain` and `FaultDomain` with
`Unattributed` becoming `Unknown`; `TransmitRoute::Clearnet` names the
accepting Destination, and the CLI's transmit report renders
`destination` on both routes.

## Considered options

Rotate across the registry on clearnet too. Rejected: it moved every
self-hoster's sends off their own node, and it adds a party without hiding
anything from the sync indexer, which learns each send once mined.

Keep a hand list and add a testnet list. Rejected: testnet has one
operator, so the list would hold one entry and the exclusion would empty
it, and the drift between two hand lists is what caused the defect.

A per-network kind such as production, public test, or local test.
Rejected: it names a purpose the library cannot know, and every
difference between networks reduces to the trust a remote indexer gets by
default, which the user can override.

Serialize the set beside the wallet. Rejected: it is derived state, a
snapshot would freeze a list that must follow the registry, and writing
the wallet's broadcast targets to disk breaks the indexer history's
contract that nothing about where this wallet transmits survives the
process.

## Consequences

Testnet sends reach testnet indexers. On mainnet with the defaults, a
mixnet send rotates across every live registry operator on port 443 except
the sync indexer's, and a clearnet send goes to the sync indexer. A user's
own node on the local network receives their clearnet sends; over the
mixnet it is unreachable and the registry carries them. A self-hosted node
on a public address, such as a rented server, is untrusted until the user
marks it trusted.

The sync indexer's post-mining txid fetch remains. Removing it is a
sync-engine change that would let the exclusion protect the transaction
itself, not only the moment of broadcast.

The mock-chain tests drive the mainnet rule end to end over three mock
indexers on loopback, with a test-only registry constructor naming each
mock's operator.

Consumers of the CLI's transmit report read `destination` where the
clearnet route once said `indexer`. The CLI exposes none of the new
configuration yet.
