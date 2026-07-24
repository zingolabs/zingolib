# PR #2470 review M6: CI must execute the nym surface, not just compile it

CLAIMED 2026-07-23 by the review-remediation session (M1 ff64da547,
M2 6b9d002de, M3 0936a66d2, M4 4b70ac582, M5 58d6d513c). Finding M6:
the nym-feature job clippy-compiles every gated test but executes only
filtered zingolib module paths, leaving all zingo-cli nym-gated tests,
any gated zingolib test outside the filter list, and gated doc-tests
unexecuted. Filter lists rot — this walk extended the list twice.
Ratified remedy: run the suites whole, and fold in the netutils
rustdoc gate the workspace split dropped.

## File claims

- `.github/workflows/ci-pr.yaml` — the nym-feature job's test steps and
  the netutils-standalone doc step.
- `zingo-netutils/src/bin/nym-proxy.rs` — the redundant-link doc fix the
  new gate caught on its first local run.

## Status

APPLIED (2026-07-23), uncommitted; commits are the user's to make. The
nym-feature job's filtered test steps are replaced by whole suites —
`cargo test -p zingolib --features nym,nym-diary --lib`, the same for
`-p zingo-cli`, and a zingolib `--doc` run — with a comment recording
why filters are banned (they rotted twice inside one review walk). The
netutils-standalone job gains a `cargo doc --all-features --no-deps`
step under `RUSTDOCFLAGS: -D warnings`, restoring the rustdoc gate the
workspace split dropped; its first local run immediately caught a
redundant explicit doc link in the nym-proxy binary, now fixed.

Verified locally, command for command as CI will run them: 293 gated
zingolib lib tests, 104 zingo-cli tests, the gated doc-tests, and the
netutils doc build all green. (The first full-suite run filled the
/tmp quota mid-flight and wedged the session shell; the user cleared
the quota and every gate then passed.)
