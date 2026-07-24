# The wallet file is sealed at rest under a caller-supplied key

To protect on-disk wallet data from any reader of the device's storage, the
Wallet File can be stored encrypted — Sealed. zingolib defines the sealed
format and performs all encryption and decryption; the consumer supplies the
secret. The library's entire key interface is a raw 32-byte symmetric key, the
Unlock Key, presented when a sealed wallet is opened. How that key is
produced, stored, and released is deliberately outside this repository:
zingo-mobile guards it with the platform keystore behind a biometric prompt,
and zingo-cli derives it from a passphrase. The library never sees a
passphrase, a biometric, or a platform keystore.

## Envelope encryption and the four operations

Inside the format, a random data-encryption key encrypts the wallet bytes with
ChaCha20-Poly1305, and the Unlock Key merely wraps that inner key. Rekeying —
a new device keystore, a changed passphrase — therefore rewraps one small
blob rather than rewriting the wallet. The API is four operations: open a
sealed wallet by presenting the Unlock Key at load time, seal a plaintext
wallet, rekey a sealed one, and unseal as the deliberate way back to
plaintext. After open, only the inner key stays in memory (held via `secrecy`,
zeroized on drop), so the periodic save task re-seals without the Unlock Key.

There is no mid-session locked state. zecwallet-light-cli's lock/unlock pair
poisoned every API method with a key-absent error path, and a consumer reaches
the same security state by dropping the client. A client is open or it is gone.

## One seal, not a sync/spend split

We considered a two-tier format — viewing keys and sync state under an
always-available key, spend authority under a stricter one — so background
sync could survive a strict biometric policy. We chose a single seal over the
whole file. The trade-off the split serves is real, but it belongs to the
consumer's key-release policy, not to the file format: a mobile keystore key
minted as "available after first device unlock" preserves background sync,
while "require per-use authentication" trades sync-while-closed for strength.
Encoding that policy choice into the format would have doubled the
serialization, the API, and every save path for a knob the platform already
provides.

## Opt-in, sticky, and honest about plaintext

Sealing is opt-in per wallet, and plaintext remains a legitimate at-rest state:
every deployed `zingo-wallet.dat` is plaintext today, and headless consumers
must keep working without inventing key management. A sealed file carries a
magic header with a format version and a KDF identifier; anything else parses
as legacy plaintext. Once a wallet is sealed, every save re-seals — the
library never silently writes plaintext again — and only an explicit unseal
goes back. We rejected making the seal irreversible: the seed restores the
wallet anywhere, so irreversibility would brick files without protecting
anything.

## Cryptography from the existing lockfile

The dependency rule (no new dependencies, no patches) holds: the scheme is
built entirely from crates already in `Cargo.lock` — `chacha20poly1305`,
`blake2b_simd`, `getrandom`, `secrecy`, `zeroize`, and, for the CLI's
passphrase path, `pbkdf2` (HMAC-SHA-512, iteration count in the hundreds of
thousands, per-wallet random salt stored in the header). We acknowledge PBKDF2
is weaker against GPU offline attack than the memory-hard argon2, which would
require a dependency ratification like the nym stack received. The mobile
path never touches a KDF — a keystore key is full-entropy — so PBKDF2 guards
only users who choose passphrases, and the header's KDF identifier lets a
future format adopt argon2 without a break.

## The rest of the disk: starve, don't encrypt

"All on-disk data" resolves to three surfaces. The Wallet File is sealed as
above. Nym mixnet state is kept off disk entirely: the client is constructed
ephemeral (`MixnetClientBuilder::new_ephemeral()`), and that ephemerality is a
pinned invariant — switching to persistent nym storage would create plaintext
mixnet identity keys and must revisit this ADR. The debug log is an
append-stream, and we refuse to encrypt a stream we can instead starve: no key
material, seeds, or addresses may be logged, and file logging stays off by
default for connected use.
