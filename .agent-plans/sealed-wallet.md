# Sealed wallet: at-rest encryption

Grilling session of 2026-07-21 resolved the design; the decisions are
recorded in `docs/adr/0012-sealed-wallet-at-rest-encryption.md` and the terms
in `zingolib/CONTEXT.md` (Persistence section). Implementation has not
started.

## Resolved decisions

The library encrypts and the caller unlocks: zingolib takes a raw 32-byte
Unlock Key, and biometrics live entirely in the consumer. Envelope encryption
(ChaCha20-Poly1305) wraps a random inner key, so rekeying is cheap. One seal
covers the whole file; the background-sync trade-off is the consumer's
keystore policy, not a format tier. Sealing is opt-in and sticky, with a
deliberate unseal; plaintext stays legitimate. The API is open, seal, rekey,
unseal — no mid-session lock. The CLI derives its key with in-tree PBKDF2
behind a KDF identifier in the header. Nym state stays ephemeral (pinned
invariant), and the log is starved of secrets rather than encrypted. No new
dependencies.

## File claims

This stream owns `docs/adr/0012-sealed-wallet-at-rest-encryption.md`, the
Persistence entries of `zingolib/CONTEXT.md`, and this file. Implementation
claims (wallet disk I/O, LightClient API, zingo-cli commands) will be added
here before any code edit.
