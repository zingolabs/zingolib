# A lock-neutral dependency edge may ground a constant

Status: draft — ruled in session 2026-08-23, pending review

## Context

The workspace forbids new dependencies. The rule exists to keep the
compiled surface fixed: every new package is new code to audit, build,
and trust, and pepper-sync in particular carries a standing instruction
that its dependency set only shrinks.

The serialization module needed named constants for its fixed byte
widths: field elements, block hashes, diversifiers, and memos. Some of
those widths are offered by crates the workspace already compiles, but
only through interfaces that require naming a crate the manifest does
not list. A jubjub scalar encodes through `<jubjub::Fr as
ff::PrimeField>::Repr`, and a pallas base element through
`pasta_curves::pallas::Base`. Both `ff` and `pasta_curves` sit in
Cargo.lock today as transitive dependencies of `sapling-crypto` and
`orchard`, resolved with their default features. Redefining the widths
locally repeats a number upstream already states; naming them upstream
requires manifest lines the rule forbids.

Recording a direct edge is not free of trace. Cargo.lock re-records the
member's `dependencies` array, so the lock text changes even though its
package set does not.

## Decision

A new manifest dependency edge is acceptable when both conditions hold.

1. It unifies a constant. A locally minted literal is replaced by a
   definition that an already-compiled crate offers.
2. It is lock-neutral. The Cargo.lock package set is unchanged: no
   package entry is added or removed, no version or checksum changes,
   and no feature is newly enabled. The only permitted lock delta is a
   member's `dependencies` array re-recording an existing package as a
   direct dependency.

An edge that fails either condition remains governed by the
no-new-dependencies rule and requires its own ruling.

## Consequences

The first application adds `ff = "0.13.1"` and `pasta_curves =
"0.5.2"` to the workspace manifest and to pepper-sync. It grounds
`FIELD_ELEMENT_SIZE` in `PrimeField::Repr`, with a compile-time
assertion that the jubjub and pallas widths agree.

Reviewers verify lock-neutrality by reading the Cargo.lock hunk in the
diff, which must touch only `dependencies` arrays. A feature-resolution
comparison with `cargo tree --format "{p} features={f}"` before and
after the edge proves that no feature was newly enabled.
