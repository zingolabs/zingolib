//! The typed voting event stream and its subscribable handle.
//!
//! `begin_voting` hands the consumer a [`VotingProgressHandle`]; the session
//! publishes [`VotingEvent`]s into its broadcast channel while the lifecycle
//! runs. A consumer subscribes before starting the work and reads events as
//! they publish, the same ownership shape as the transmit progress handle. No
//! polling loop and no stringly status line: every event is a typed value,
//! per the repo mandate that zingolib publishes events.

use tokio::sync::broadcast;

use crate::error::VotingFailureSummary;

/// One state change in a round's voting lifecycle.
#[derive(Clone, Debug)]
pub enum VotingEvent {
    /// The pinned static config resolved and its dynamic mirrors authenticated.
    ConfigResolved { source: String },
    /// An open round authenticated against the resolved config's `ea_pk`.
    RoundResolved {
        round_id: String,
        snapshot_height: u64,
        vote_end_time_seconds: u64,
    },
    /// The sidecar round row exists; the lifecycle can journal decisions.
    RoundInitialized { round_id: String },
    /// The wallet's snapshot notes were bundled and the bundle plan persisted.
    SnapshotNotesSelected {
        note_count: u32,
        eligible_weight_zatoshi: u64,
    },
    /// The round's voting hotkey exists and its secret is stored in the sidecar.
    HotkeyReady { round_id: String, address: String },
    /// One PIR warmup pass finished over the resolved layout.
    PirWarmup { cached: u32, fetched: u32 },
    /// One bundle's witnesses are stored and its delegation precompute is warm.
    BundlePrepared { bundle_index: u32 },
    /// One bundle's delegation is proven and its SpendAuth signature applied.
    DelegationSigned {
        bundle_index: u32,
        delegated_weight_zatoshi: u64,
    },
    /// One bundle's delegation reached the vote chain's dispatch path.
    DelegationSubmitted { bundle_index: u32 },
    /// One bundle's delegation confirmed on the vote chain.
    DelegationConfirmed { bundle_index: u32 },
    /// One cast-vote is proven, signed, and durably recoverable.
    VoteCommitted { bundle_index: u32, proposal_id: u32 },
    /// One cast-vote confirmed on the vote chain.
    VoteConfirmed { bundle_index: u32, proposal_id: u32 },
    /// One helper share entered the durable delivery plan.
    ShareScheduled {
        bundle_index: u32,
        proposal_id: u32,
        share_index: u32,
    },
    /// One helper share was accepted by a helper.
    ShareDelivered {
        bundle_index: u32,
        proposal_id: u32,
        share_index: u32,
    },
    /// The round's lifecycle reached a state with nothing left to drive.
    Finished { round_id: String },
    /// The lifecycle stopped on a failure; no further work runs this pass.
    Failed {
        round_id: Option<String>,
        error: VotingFailureSummary,
    },
}

/// A cloneable handle to the session's typed event channel.
///
/// Grab it from [`crate::begin_voting`](crate::VotingProgressHandle) before
/// calling [`crate::cast_vote`], subscribe, and read events concurrently.
#[derive(Clone)]
pub struct VotingProgressHandle {
    events: broadcast::Sender<VotingEvent>,
}

impl VotingProgressHandle {
    pub(crate) fn channel() -> (Self, broadcast::Receiver<VotingEvent>) {
        let (events, receiver) = broadcast::channel(crate::session::EVENT_CHANNEL_CAPACITY);
        (Self { events }, receiver)
    }

    /// Subscribes to the lifecycle's events. Events published before the
    /// subscription are not replayed; subscribe before starting work.
    pub fn subscribe(&self) -> broadcast::Receiver<VotingEvent> {
        self.events.subscribe()
    }

    pub(crate) fn publish(&self, event: VotingEvent) {
        // A send with no subscriber is a no-op, not a failure: the channel
        // exists so a consumer that subscribed observes the lifecycle.
        let _ = self.events.send(event);
    }
}

impl std::fmt::Debug for VotingProgressHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VotingProgressHandle")
            .field("subscribers", &self.events.receiver_count())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn failure_summary() -> VotingFailureSummary {
        VotingFailureSummary {
            kind: crate::error::VotingFailureKind::NoActiveRound,
            message: "no authenticated voting round is open".to_string(),
        }
    }

    #[tokio::test]
    async fn subscribers_receive_published_events_in_order() {
        let (handle, mut receiver) = VotingProgressHandle::channel();
        handle.publish(VotingEvent::RoundInitialized {
            round_id: "abc".to_string(),
        });
        handle.publish(VotingEvent::Finished {
            round_id: "abc".to_string(),
        });

        let first = receiver.recv().await.expect("first event arrives");
        let second = receiver.recv().await.expect("second event arrives");
        assert!(
            matches!(first, VotingEvent::RoundInitialized { ref round_id } if round_id == "abc")
        );
        assert!(matches!(second, VotingEvent::Finished { ref round_id } if round_id == "abc"));
    }

    #[tokio::test]
    async fn late_subscribers_do_not_replay_earlier_events() {
        let (handle, _dropped) = VotingProgressHandle::channel();
        handle.publish(VotingEvent::RoundInitialized {
            round_id: "abc".to_string(),
        });

        let mut receiver = handle.subscribe();
        handle.publish(VotingEvent::Finished {
            round_id: "abc".to_string(),
        });

        let next = receiver.recv().await.expect("the later event arrives");
        assert!(
            matches!(next, VotingEvent::Finished { .. }),
            "the event published before the subscription is not replayed"
        );
    }

    #[tokio::test]
    async fn publishing_without_subscribers_succeeds() {
        let (handle, receiver) = VotingProgressHandle::channel();
        drop(receiver);
        handle.publish(VotingEvent::Failed {
            round_id: None,
            error: failure_summary(),
        });
        handle.publish(VotingEvent::Finished {
            round_id: "abc".to_string(),
        });
    }

    #[tokio::test]
    async fn handle_is_cloneable_and_events_flow_to_original_subscribers() {
        let (handle, mut receiver) = VotingProgressHandle::channel();
        let cloned = handle.clone();
        cloned.publish(VotingEvent::RoundInitialized {
            round_id: "abc".to_string(),
        });
        let next = receiver.recv().await.expect("the clone publishes");
        assert!(
            matches!(next, VotingEvent::RoundInitialized { ref round_id } if round_id == "abc")
        );
    }
}
