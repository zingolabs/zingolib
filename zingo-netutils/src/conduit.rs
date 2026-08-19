//! The conduit a wallet dials the mixnet through.
#![forbid(unsafe_code)]

use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// Where a conduit stands in its life, which only moves forward.
///
/// The order is the life: a conduit serves, is superseded by a replacement
/// while its existing work finishes, and retires once that work is done. It
/// never returns to an earlier state, which is what makes comparing two
/// states mean something.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum ConduitState {
    /// Carrying new work.
    Serving,
    /// A replacement took over. Work already dialed finishes here, and no
    /// new work arrives.
    Superseded,
    /// Superseded and idle, so the transport behind it can stop.
    Retired,
}

/// What a conduit dials through, shared by every guard it issues.
#[derive(Debug)]
struct ConduitCore {
    socks5: SocketAddr,
    superseded: AtomicBool,
}

/// Somewhere to send mixnet traffic, proven for the epoch (ADR 0046).
///
/// A conduit hands out no address of its own. Dialing takes a
/// [`ConduitDial`], counted while it lives, so the transport behind a
/// superseded conduit retires the moment its work drains rather than after
/// a guessed interval (ADR 0048).
#[derive(Clone, Debug)]
pub struct MixnetConduit {
    core: Arc<ConduitCore>,
}

/// One use of a conduit, counted while it lives.
///
/// Hold it for as long as the work it carries, because dropping it is what
/// says that work has finished.
#[derive(Debug)]
pub struct ConduitDial {
    core: Arc<ConduitCore>,
}

impl MixnetConduit {
    /// Wraps a ready transport's published address.
    pub fn over(socks5: SocketAddr) -> Self {
        MixnetConduit {
            core: Arc::new(ConduitCore {
                socks5,
                superseded: AtomicBool::new(false),
            }),
        }
    }

    /// Takes a guard for one use, counted until it drops.
    pub fn dial(&self) -> ConduitDial {
        ConduitDial {
            core: Arc::clone(&self.core),
        }
    }

    /// How many uses are outstanding.
    pub fn in_flight(&self) -> usize {
        // The conduit holds one reference of its own, which is never a use.
        Arc::strong_count(&self.core) - 1
    }

    /// Marks a replacement as having taken over, after which no new work
    /// should dial this conduit.
    pub fn supersede(&self) {
        self.core.superseded.store(true, Ordering::Release);
    }

    /// Where this conduit stands, derived rather than stored, so retirement
    /// cannot be claimed while work is still outstanding.
    pub fn state(&self) -> ConduitState {
        if !self.core.superseded.load(Ordering::Acquire) {
            ConduitState::Serving
        } else if self.in_flight() > 0 {
            ConduitState::Superseded
        } else {
            ConduitState::Retired
        }
    }
}

/// Two conduits are the same conduit when they reach the same endpoint, so
/// a clone compares equal to what it was cloned from.
impl PartialEq for MixnetConduit {
    fn eq(&self, other: &Self) -> bool {
        self.core.socks5 == other.core.socks5
    }
}

impl Eq for MixnetConduit {}

impl ConduitDial {
    /// The address this use dials, for as long as the guard is held.
    pub fn socks5(&self) -> SocketAddr {
        self.core.socks5
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn conduit() -> MixnetConduit {
        MixnetConduit::over("127.0.0.1:9050".parse().expect("the test address parses"))
    }

    /// HYPOTHESIS: a conduit counts its outstanding uses, so dropping the
    /// last guard is observable. Falsified if a dropped guard still counts.
    #[test]
    fn a_dropped_guard_stops_counting() {
        let conduit = conduit();
        assert_eq!(conduit.in_flight(), 0);
        let first = conduit.dial();
        let second = conduit.dial();
        assert_eq!(conduit.in_flight(), 2);
        drop(first);
        assert_eq!(conduit.in_flight(), 1);
        drop(second);
        assert_eq!(conduit.in_flight(), 0);
    }

    /// HYPOTHESIS: retirement waits for the work, so a superseded conduit
    /// with a guard outstanding is not retired. Falsified if superseding
    /// alone retires it.
    #[test]
    fn a_superseded_conduit_retires_only_when_idle() {
        let conduit = conduit();
        assert_eq!(conduit.state(), ConduitState::Serving);
        let held = conduit.dial();
        conduit.supersede();
        assert_eq!(conduit.state(), ConduitState::Superseded);
        drop(held);
        assert_eq!(conduit.state(), ConduitState::Retired);
    }

    /// HYPOTHESIS: the states order as the life runs, so a later state
    /// compares greater. Falsified if the derive ever reorders them.
    #[test]
    fn the_states_order_as_the_life_runs() {
        assert!(ConduitState::Serving < ConduitState::Superseded);
        assert!(ConduitState::Superseded < ConduitState::Retired);
    }

    /// HYPOTHESIS: a clone shares one count, so a conduit handed to two
    /// holders reports their uses together. Falsified if cloning forks the
    /// count.
    #[test]
    fn a_clone_shares_one_count() {
        let conduit = conduit();
        let shared = conduit.clone();
        let held = shared.dial();
        assert_eq!(conduit.in_flight(), 1);
        drop(held);
        assert_eq!(conduit.in_flight(), 0);
    }
}
