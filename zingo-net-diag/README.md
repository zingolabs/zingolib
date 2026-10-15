# zingo-net-diag

<!-- cargo-rdme start -->

The shared network-failure taxonomy (`docs/agents/net-diag-design.md`).

A network failure reported as prose can be read but never dispatched on.
When a Zingo operation failed somewhere between the local mixnet proxy
and a remote indexer, the wallet used to report one flattened sentence,
and every consumer — above all the mobile Connection Doctor — was
reduced to substring-matching that sentence to guess which layer broke.
Each layer that folded its cause into a string destroyed the evidence
the next layer needed. This crate ends the guessing: every covered
operation — the price fetch, the broadcast fan-out, the attach
validation, and the sync-path connectivity probe — reports its failures
as data. A `NetOpFailure` names which `NetOpStage` failed, against
what target, with the full cause chain carried as one text per layer — a
vector, never a concatenated string. One taxonomy serves them all, so a
consumer that learns to read one operation's failures can read every
operation's.

The crate is used in two roles. A *producer* — code that owns an error
type — decides the stage and captures the cause chain with
`NetOpFailure::from_error` (or walks a chain directly with
`chain_texts`). This crate deliberately holds no classifiers of its
own: classification belongs with the crates that own the error types
(`zingo-price` classifies `reqwest::Error`; zingolib classifies
`Socks5TransmitError`), and this crate holds only the taxonomy and the
generic chain inspector. A *consumer* matches on the typed fields and
chooses its own presentation; the `Display` rendering exists for humans
and logs only. The crate is std-only with zero dependencies, a hard
requirement: it is what lets one crate serve two lockfile-isolated cargo
workspaces (the parent workspace and the standalone `zingo-netutils`
workspace) without resolver coupling.

```rust
use zingo_net_diag::{NetOpFailure, NetOpStage};

// The producer side: at the seam that owns the error, name the stage
// and capture the whole source() chain, one text per layer.
let refused = std::io::Error::new(
    std::io::ErrorKind::ConnectionRefused,
    "connection refused",
);
let failure = NetOpFailure::from_error(
    NetOpStage::LocalProxyConnect,
    "127.0.0.1:1080",
    &refused,
);

// The consumer side: dispatch on fields, never on rendered prose.
match &failure.stage {
    NetOpStage::LocalProxyConnect => {
        // The local SOCKS endpoint is down: repair the proxy before
        // any remote target is worth probing.
    }
    NetOpStage::TimedOut { after_ms } => {
        println!("gave up after {after_ms}ms");
    }
    _ => {}
}
assert_eq!(failure.cause_chain, ["connection refused"]);
assert_eq!(
    failure.to_string(),
    "failed at local-proxy-connect to 127.0.0.1:1080: connection refused",
);
```

For a production producer in a Nym-touching context, see
`zingolib::mixnet::socks5_transmit_stage`: a pure typed match that
classifies every `Socks5TransmitError` variant into its stage with no
substring inspection. For the consumer-visible payoff, see
`LightClient::mixnet_death_detail`, which answers *why* the mixnet
transport died with one of these records instead of a sentence.

<!-- cargo-rdme end -->
