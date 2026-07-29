# 25. Going online requires Connectivity Consent

Date: 2026-07-28

Status: draft — ratified in session, pending review

## Context

ADR 0001 made the library offline by default: a `ClientConfig` configures
no Indexer until the consumer explicitly asks for one, because the
consumer should control when and with which server network activity
happens. The consumers then quietly inverted that decision at their own
layer. A default zingo-cli session resolves an Indexer and connects at
launch; zingo-mobile and zingo-pc assume a connected session from first
boot. The user who was supposed to control the first network contact was
never asked.

The 2026-07-28 session-driver grilling surfaced the inversion while
placing the mixnet driver's go-online moment, and the user ratified the
missing rule: the default state must be offline. The same session had
already split the wallet's consents into two tiers — the per-session
transport consent (Mixnet Mode's switched off, never persisted) and the
connectivity consent above it — and ratified that the ground state
carries no online intent (the recovery predicate excludes unattached for
exactly this reason).

## Decision

First boot is offline for every consumer — zingo-cli, zingo-mobile, and
zingo-pc. Going online happens only by an explicit user act, and the
user may store that choice so later sessions attach to the network
automatically.

zingolib owns the stored choice and its predicate, as it owns the rest
of the session policy (ADR 0024): the `connectivity` module records the
standing choice in a `connectivity-consent` file beside the wallet,
whose sole granting content is the minted token `standing-online`. The
store is fail-closed — an absent, unreadable, or unrecognized record
reads as unrecorded and withholds the connection, so corruption can keep
a session offline but never take one online. Consumers render the prompt
and pass the acts in; they never re-derive the rules or restate the
tokens.

Connectivity Consent is the outer of the two consent tiers and the only
persistable one. The inner tier — the per-session clearnet opt-out that
Mixnet Mode's switched off records — is never persisted, and a stored
connectivity consent implies nothing about it: a session that
auto-attaches still forces the mixnet on at its go-online moment, per
the session driver's start policy.

In zingo-cli the explicit acts are launch-time: `--online` consents for
the session, `--remember-online` consents and stores the standing
choice, `--forget-online` removes it, and an explicit `--server`
argument is itself a consent act, since it names the endpoint to
connect to. A session with no recorded and no expressed consent runs
offline with a notice naming the acts; the deliberate `--offline` flag
keeps its stricter contract (zero network traffic, no notice) unchanged.

## Considered options

Keeping the connected default and adding only an opt-out was rejected:
an opt-out after the fact cannot un-send the first launch's traffic, and
the library's own founding decision (ADR 0001) already names silent
network activity as the harm. Persisting the consent inside the wallet
file was rejected because the choice is not secret material, would cost
a format bump, and must be readable before a wallet necessarily exists;
a sidecar file beside the wallet follows the indexer diary's pattern.
Prompting interactively at first boot was rejected for zingo-cli because
the CLI runs scripted as often as interactively; flags are the CLI's
native consent surface, and the graphical consumers will render their
own prompts in their convergence phases.

## Consequences

zingo-cli's default invocation changes behavior: with no stored consent
and no consent act, the session runs offline where it previously
connected. This is the point, but it is breaking for scripts, which must
add `--online` (or store consent once with `--remember-online`).
zingo-mobile and zingo-pc still boot connected until their convergence
phases adopt the stored choice; until then the doctrine holds only where
this repo can enforce it. The first-boot notice must make the path
online obvious, or the flip reads as breakage rather than consent.
