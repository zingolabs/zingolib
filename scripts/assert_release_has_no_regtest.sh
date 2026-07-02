#!/usr/bin/env bash
# Release tripwire: production artifacts must never contain regtest support.
#
# Cargo feature unification can silently re-enable zingolib's `regtest`
# feature through any dependency that requests it, so a default-off feature
# is not sufficient on its own. This script asserts that zingo-consensus
# (the activation-heights vocabulary crate, pulled only by the `regtest`
# feature) is absent from the default dependency graph of each release
# artifact. Run it in release CI before publishing or packaging.
#
# See docs/adr/0002-regtest-compiled-out-of-production.md.
set -euo pipefail

cd "$(dirname "$0")/.."

failed=0
for package in zingolib zingo-cli; do
  if cargo tree --package "$package" --edges normal | grep --quiet zingo-consensus; then
    echo "FAIL: ${package} release dependency graph contains zingo-consensus (regtest is enabled)"
    failed=1
  else
    echo "OK: ${package} release dependency graph has no regtest support"
  fi
done
exit "$failed"
