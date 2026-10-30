//! The voting session: arms against the live config and drives a round's
//! whole lifecycle.
//!
//! [`arm_session`] resolves the pinned config, authenticates an open round,
//! and prepares everything offline-capable: the sidecar round row, the
//! round's voting hotkey, and witness validation. [`cast_ballot`] journals
//! the ballot, then runs a bounded plan-driven loop over
//! [`resume_plan`]: every pass re-reads the durable plan and executes its
//! first step (bundle precompute, delegation, chain advancement, cast, share
//! delivery), so a crash at any point resumes from durable state instead of
//! redoing finished work. Each state change publishes a typed
//! [`VotingEvent`]. The SDK's `RoundExecutor` drives the same plan shape, but
//! its delegation steps need a `DelegationDriver` implementation that only
//! exists behind the fork's wallet-DB seam; the manual delegation path here
//! is the seam-free counterpart (`PreparedDelegationBundle` is constructed
//! directly from the session's inputs, all of whose fields are public).

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use zcash_voting::chain_submission::{
    AdvanceDelegation, AdvanceVote, ChainRecoveryMode, ChainSubmissionClient,
    ChainSubmissionClientConfig, ChainSubmissionControl, ChainSubmissionResult,
    DEFAULT_CHAIN_MAXIMUM_POST_ATTEMPTS, DEFAULT_CHAIN_RETRY_BACKOFFS,
    DEFAULT_CHAIN_TRACKING_WINDOW,
};
use zcash_voting::config::ResolvedVotingConfig;
use zcash_voting::delegate::{
    DelegationAccountKeys, DelegationKeys, DelegationLwdInputs, DelegationSigningRequest,
    PreparedDelegationBundle, PreparedSigner, ResolveDelegationLwdParams,
    gather_delegation_lwd_inputs,
};
use zcash_voting::helper::client::HelperClient;
use zcash_voting::helper::health::HelperHealth;
use zcash_voting::hotkey::generate_random_voting_hotkey;
use zcash_voting::note_bundling::BundlePolicy;
use zcash_voting::pir::PirFleet;
use zcash_voting::precompute::{
    SnapshotBundlePrecomputeReport, precompute_snapshot_bundles, sync_vote_tree, van_witness,
};
use zcash_voting::prelude::NoopProgressReporter;
use zcash_voting::round::{
    BundleLayout, VotingDb, bundle_notes_for_index_for_round, delegation_round_name,
};
use zcash_voting::session::{NextStep, resume_plan};
use zcash_voting::share_tracking::{ShareDeliveryPlanningParams, ShareDeliverySubmissionParams};
use zcash_voting::types::{Network, NoteInfo, VotingError, WitnessData};
use zcash_voting::vote::{CommittedVote, VanWitness};
use zcash_voting::wire::{DraftVote, VoteRecord};
use zcash_voting::{DelegationSigner, DirectRoute, HyperTransport, VotingHotkey};

use crate::config::{STAGING_STATIC_VOTING_CONFIG_SOURCE, resolve_voting_config};
use crate::error::VotingFailure;
use crate::events::{VotingEvent, VotingProgressHandle};
use crate::journal::{self, BallotDecision};
use crate::vote_server::{
    ActiveVotingRound, ServerRound, authenticate_round, endorsed_round_ids, endorsers, fetch_rounds,
};
use crate::witness::validate_witnesses;

/// Upper bound on plan-driven passes in one cast. Each pass advances at least
/// one durable step or surfaces a failure, so the bound only guards against a
/// pathological plan that regresses forever.
const MAX_LIFECYCLE_PASSES: usize = 64;

/// Events buffered per subscriber before the oldest one drops. A consumer
/// reading its subscription keeps pace; the capacity only bounds a stalled
/// reader's backlog.
pub(crate) const EVENT_CHANNEL_CAPACITY: usize = 64;

/// Everything the wallet contributes to a round beyond the sidecar: the
/// snapshot note set, its witnesses at the snapshot height, and the account's
/// ZIP-32 context. zingolib builds this under the wallet lock; the session
/// consumes it as plain data.
#[derive(Clone, Debug)]
pub struct VotingSnapshot {
    /// The round's snapshot-eligible notes, in canonical order.
    pub note_infos: Vec<NoteInfo>,
    /// One witness per note, rooted at the round's snapshot `nc_root`.
    pub witnesses: Vec<WitnessData>,
    /// The account's ZIP-32 context for delegation keys.
    pub account: DelegationAccountKeys,
}

/// Host-owned endpoints and the pinned config source for one session.
///
/// Vote-server and PIR endpoints come from the resolved config at arm time;
/// the helper fleet and vote-tree nodes are host choices because the config
/// does not name them. Defaults pair every helper and tree URL with the
/// resolved vote servers, the deployment shape staging runs.
#[derive(Clone, Debug)]
pub struct VotingSessionConfig {
    pub network: Network,
    /// The pinned static config source. Production passes its own pin; the
    /// default is the staging constant.
    pub static_config_source: String,
    /// The wallet's indexer (lightwalletd-compatible) URI, for the anchor
    /// treestate fetch.
    pub lightwalletd_url: String,
    /// Helper fleet for share delivery; defaults to the vote servers.
    pub helper_server_urls: Option<Vec<String>>,
    /// Vote-tree nodes for VAN witnessing; defaults to the vote servers.
    pub vote_tree_node_urls: Option<Vec<String>>,
}

impl VotingSessionConfig {
    /// A staging config pointing at the wallet's indexer.
    pub fn staging(network: Network, lightwalletd_url: String) -> Self {
        Self {
            network,
            static_config_source: STAGING_STATIC_VOTING_CONFIG_SOURCE.to_string(),
            lightwalletd_url,
            helper_server_urls: None,
            vote_tree_node_urls: None,
        }
    }

    /// The production config, the default for mainnet wallets once the
    /// production source deploys.
    pub fn production(network: Network, lightwalletd_url: String) -> Self {
        Self {
            network,
            static_config_source: crate::config::PRODUCTION_STATIC_VOTING_CONFIG_SOURCE.to_string(),
            lightwalletd_url,
            helper_server_urls: None,
            vote_tree_node_urls: None,
        }
    }
}

/// A session armed against the live config, ready for [`cast_ballot`].
pub struct ArmedVotingSession {
    db: Arc<VotingDb>,
    config: VotingSessionConfig,
    resolved: ResolvedVotingConfig,
    round: ActiveVotingRound,
    snapshot: VotingSnapshot,
    events: VotingProgressHandle,
}

/// The wallet-facing summary of one completed cast.
#[derive(Clone, Debug)]
pub struct VotingRoundOutcome {
    pub round_id: String,
    /// True when the final plan reports every proposal decided with no
    /// blocking recovery left.
    pub all_decided: bool,
    /// True when the final plan still owes blocking work the next cast
    /// resumes.
    pub blocking_recovery: bool,
}

/// The durable state a consumer reads back for a round: the journaled votes,
/// the share rows, and the sidecar round phase.
#[derive(Clone, Debug)]
pub struct VotingResults {
    pub round_id: String,
    /// One row per journaled vote: proposal, bundle, choice.
    pub votes: Vec<VoteRecord>,
    /// One row per helper share: its delivery and confirmation state.
    pub shares: Vec<zcash_voting::types::ShareDelegationRecord>,
    /// The sidecar's lifecycle phase for the round, when initialized.
    pub round_phase: Option<zcash_voting::storage::RoundPhase>,
}

/// The wallet-facing summary of the active round: authenticated metadata
/// plus the wallet's bundle-quantized voting weight at the snapshot height.
#[derive(Clone, Debug)]
pub struct ActiveRoundSummary {
    pub round: ActiveVotingRound,
    /// Bundle-quantized weight the wallet can prove at the snapshot.
    pub eligible_weight_zatoshi: u64,
    /// Bundles the canonical plan builds from the snapshot notes.
    pub bundle_count: u32,
}

fn now_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|since| since.as_secs())
        .unwrap_or_default()
}

impl ArmedVotingSession {
    /// The round this session drives.
    pub fn round_id(&self) -> &str {
        &self.round.round_id
    }

    /// A handle to the session's event channel.
    pub fn progress_handle(&self) -> VotingProgressHandle {
        self.events.clone()
    }

    /// The authenticated round's metadata.
    pub fn round(&self) -> &ActiveVotingRound {
        &self.round
    }
}

fn round_bytes(round_id: &str) -> Result<[u8; 32], VotingFailure> {
    let bytes = (0..round_id.len() / 2)
        .map(|index| {
            u8::from_str_radix(&round_id[index * 2..index * 2 + 2], 16).map_err(|_| {
                VotingFailure::VoteServer(format!("round id {round_id} is not lowercase hex"))
            })
        })
        .collect::<Result<Vec<u8>, VotingFailure>>()?;
    bytes.try_into().map_err(|bytes: Vec<u8>| {
        VotingFailure::VoteServer(format!("round id decodes to {} bytes", bytes.len()))
    })
}

fn chain_config(
    network: Network,
    vote_server_urls: &[String],
) -> Result<ChainSubmissionClientConfig, VotingError> {
    Ok(ChainSubmissionClientConfig {
        network,
        vote_chain_id: network.default_vote_chain_id().to_string(),
        endpoints: vote_server_urls.to_vec(),
        tracking_window: DEFAULT_CHAIN_TRACKING_WINDOW,
        maximum_post_attempts: DEFAULT_CHAIN_MAXIMUM_POST_ATTEMPTS,
        retry_backoffs: DEFAULT_CHAIN_RETRY_BACKOFFS.to_vec(),
    })
}

fn round_roster(round: &ActiveVotingRound) -> Vec<(u32, u32)> {
    round
        .proposals
        .iter()
        .map(|proposal| (proposal.id, proposal.num_options))
        .collect()
}

fn roster_ids(round: &ActiveVotingRound) -> Vec<u32> {
    round.proposals.iter().map(|proposal| proposal.id).collect()
}

fn load_or_create_hotkey(
    db: &VotingDb,
    round_id: &str,
    network: Network,
    events: &VotingProgressHandle,
) -> Result<VotingHotkey, VotingFailure> {
    if let Some(secret) = journal::load_hotkey_secret(db, round_id)? {
        return VotingHotkey::from_stored_secret(&secret, network).map_err(VotingFailure::Sdk);
    }
    let hotkey = generate_random_voting_hotkey(network).map_err(VotingFailure::Sdk)?;
    journal::store_hotkey_secret(db, round_id, hotkey.stored_secret())?;
    let address = hotkey
        .delegation_target()
        .raw_orchard_address()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    events.publish(VotingEvent::HotkeyReady {
        round_id: round_id.to_string(),
        address,
    });
    Ok(hotkey)
}

/// Resolves the pinned config and fetches the servers' rounds, returning the
/// resolved config, every fetched round, and the authenticated open round
/// closing soonest.
///
/// Shared by [`arm_session`] and a client's round-metadata query. Network
/// work is limited to the config and rounds fetches.
///
/// # Errors
///
/// Returns [`VotingFailure::NoActiveRound`] when no authenticated round is
/// open, and the underlying failure otherwise.
/// Resolves the config and lists the rounds the vote servers publish,
/// without picking an active one: the network-wide overview. Only rounds
/// the signed config authenticates are listed — test churn and unverified
/// entries on a server stay out. A network with no reachable server yields
/// the config error, not an empty list.
pub async fn list_network_rounds(
    config: &VotingSessionConfig,
) -> Result<(ResolvedVotingConfig, Vec<ServerRound>), VotingFailure> {
    let route = DirectRoute::default();
    let resolved = resolve_voting_config(&route, &config.static_config_source).await?;

    let mut rounds = Vec::new();
    let mut last_error = None;
    for url in resolved
        .vote_servers
        .iter()
        .map(|server| server.url.clone())
    {
        match fetch_rounds(&route, &url).await {
            Ok(server_rounds) => {
                rounds = server_rounds;
                break;
            }
            Err(error) => last_error = Some(error),
        }
    }
    if rounds.is_empty() {
        return Err(last_error.unwrap_or(VotingFailure::VoteServer(
            "no vote servers configured".to_string(),
        )));
    }

    let authenticated: std::collections::HashSet<String> = resolved
        .authenticated_rounds
        .iter()
        .map(|round| round.round_id.clone())
        .collect();

    // Count chain endorsements per round: how many registered endorsers
    // back each ballot. The registry may not exist on every chain.
    let mut endorsements: std::collections::HashMap<String, Vec<String>> =
        std::collections::HashMap::new();
    let endorser_ids = endorsers(&route, &resolved.vote_servers[0].url)
        .await
        .unwrap_or_default();
    for endorser in endorser_ids {
        if let Ok(Some(endorsed)) =
            endorsed_round_ids(&route, &resolved.vote_servers[0].url, &endorser).await
        {
            for round_id in endorsed {
                endorsements
                    .entry(round_id)
                    .or_default()
                    .push(endorser.clone());
            }
        }
    }
    let rounds = rounds
        .into_iter()
        .filter(|round| authenticated.contains(&round.round_id))
        .map(|mut round| {
            round.endorsers = endorsements
                .get(&round.round_id)
                .cloned()
                .unwrap_or_default();
            round
        })
        .collect();
    Ok((resolved, rounds))
}

pub async fn resolve_active_round(
    config: &VotingSessionConfig,
) -> Result<(ResolvedVotingConfig, Vec<ServerRound>, ActiveVotingRound), VotingFailure> {
    let route = DirectRoute::default();
    let resolved = resolve_voting_config(&route, &config.static_config_source).await?;

    let mut rounds = Vec::new();
    for url in resolved
        .vote_servers
        .iter()
        .map(|server| server.url.clone())
    {
        match fetch_rounds(&route, &url).await {
            Ok(server_rounds) => {
                rounds = server_rounds;
                break;
            }
            Err(_) => continue,
        }
    }

    let round = authenticate_round(&resolved, &rounds, now_seconds())?;
    Ok((resolved, rounds, round))
}

/// Resolves the pinned config, authenticates an open round, and prepares
/// everything offline-capable: the sidecar round row, the hotkey, and
/// witness validation against the round's authenticated `nc_root`.
///
/// `requested_round_id` pins the session to one round: `None` takes the
/// authenticated round closing soonest, and a round that is authenticated
/// but closed refuses with [`VotingFailure::NoActiveRound`]. Network work is
/// limited to the config and rounds fetches; PIR precompute, proving, chain
/// submission, and helper delivery wait for [`cast_ballot`].
///
/// # Errors
///
/// Returns [`VotingFailure::UnauthenticatedRound`] when the requested round
/// is not in the resolved config's authenticated rounds,
/// [`VotingFailure::NoActiveRound`] when it is closed or none is open,
/// [`VotingFailure::WitnessRootMismatch`] when the wallet's witnesses do not
/// root at the round's `nc_root`, and the underlying failure otherwise.
pub async fn arm_session(
    config: VotingSessionConfig,
    requested_round_id: Option<&str>,
    snapshot: VotingSnapshot,
    db: Arc<VotingDb>,
) -> Result<ArmedVotingSession, VotingFailure> {
    let (events, _receiver) = VotingProgressHandle::channel();

    let (resolved, rounds, soonest) = resolve_active_round(&config).await?;
    events.publish(VotingEvent::ConfigResolved {
        source: config.static_config_source.clone(),
    });

    let round = match requested_round_id {
        None => soonest,
        Some(round_id) if soonest.round_id == round_id => soonest,
        Some(round_id) => {
            let authenticated = resolved
                .authenticated_rounds
                .iter()
                .any(|candidate| candidate.round_id == round_id);
            let requested = rounds
                .iter()
                .find(|candidate| candidate.round_id == round_id)
                .filter(|_| authenticated)
                .ok_or_else(|| VotingFailure::UnauthenticatedRound {
                    round_id: round_id.to_string(),
                })?;
            authenticate_one(&resolved, requested)?
        }
    };
    events.publish(VotingEvent::RoundResolved {
        round_id: round.round_id.clone(),
        snapshot_height: round.params.snapshot_height,
        vote_end_time_seconds: round.vote_end_time_seconds,
    });

    validate_witnesses(&snapshot.witnesses, &snapshot.note_infos, &round.params)?;

    if !db.has_round(&round.round_id)? {
        db.init_round(config.network, &round.params, None)?;
    }
    events.publish(VotingEvent::RoundInitialized {
        round_id: round.round_id.clone(),
    });

    load_or_create_hotkey(&db, &round.round_id, config.network, &events)?;

    Ok(ArmedVotingSession {
        db,
        config,
        resolved,
        round,
        snapshot,
        events,
    })
}

/// Authenticates one specific server round against the resolved config.
fn authenticate_one(
    resolved: &ResolvedVotingConfig,
    requested: &ServerRound,
) -> Result<ActiveVotingRound, VotingFailure> {
    let params = resolved
        .trusted_voting_round_params(
            requested.round_id.clone(),
            requested.snapshot_height,
            requested.nc_root.clone(),
            requested.nullifier_imt_root.clone(),
        )
        .map_err(|error| VotingFailure::Config(error.to_string()))?;
    Ok(ActiveVotingRound {
        round_id: requested.round_id.clone(),
        params,
        proposals: requested.proposals.clone(),
        vote_end_time_seconds: requested.vote_end_time_seconds,
        status: requested.status,
    })
}

/// Signs `request` with the session's signer.
fn sign_request(
    signer: &DelegationSigner,
    request: DelegationSigningRequest,
) -> Result<[u8; 64], VotingFailure> {
    match signer {
        DelegationSigner::Software(spend_auth) => {
            Ok(spend_auth.sign(request).map_err(VotingFailure::Sdk)?)
        }
        DelegationSigner::Keystone(_) => Err(VotingFailure::SigningUnsupported {
            message: "the Keystone round-trip is out of scope for this phase".to_string(),
        }),
    }
}

/// The session's snapshot witness rows, by tree position.
fn witness_for_position<'a>(
    witnesses: &'a [WitnessData],
    position: u64,
) -> Result<&'a WitnessData, VotingFailure> {
    witnesses
        .iter()
        .find(|witness| witness.position == position)
        .ok_or_else(|| {
            VotingFailure::Sdk(VotingError::InvalidInput {
                message: format!("no snapshot witness for note position {position}"),
            })
        })
}

async fn resolve_lwd_inputs(
    session: &ArmedVotingSession,
) -> Result<DelegationLwdInputs, VotingFailure> {
    let round_name = delegation_round_name(&session.round.params, "");
    gather_delegation_lwd_inputs(ResolveDelegationLwdParams {
        lightwalletd_url: &session.config.lightwalletd_url,
        network: session.config.network,
        round_params: session.round.params.clone(),
        round_name: &round_name,
    })
    .await
    .map_err(VotingFailure::Sdk)
}

fn precompute_snapshot_plan(
    session: &ArmedVotingSession,
    pir: &PirFleet,
) -> Result<SnapshotBundlePrecomputeReport, VotingFailure> {
    pir.with_failover(|pir_session| {
        precompute_snapshot_bundles(
            &session.db,
            &session.round.round_id,
            &session.snapshot.note_infos,
            BundlePolicy::default(),
            pir_session,
            session.config.network,
        )
    })
    .map_err(VotingFailure::Sdk)
}

fn store_bundle_witnesses(
    session: &ArmedVotingSession,
    layout: &BundleLayout,
    bundle_index: u32,
) -> Result<(), VotingFailure> {
    let bundle_notes = bundle_notes_for_index_for_round(
        &session.snapshot.note_infos,
        layout,
        bundle_index,
        &session.db,
        &session.round.round_id,
    )?;
    let witnesses = bundle_notes
        .iter()
        .map(|note| witness_for_position(&session.snapshot.witnesses, note.position).cloned())
        .collect::<Result<Vec<_>, VotingFailure>>()?;
    session
        .db
        .store_witnesses(&session.round.round_id, bundle_index, &witnesses)?;
    Ok(())
}

fn prepare_bundle(
    session: &ArmedVotingSession,
    hotkey: &VotingHotkey,
    lwd_inputs: &DelegationLwdInputs,
    layout: &BundleLayout,
    bundle_index: u32,
) -> Result<PreparedDelegationBundle, VotingFailure> {
    let round_name = delegation_round_name(&session.round.params, "");
    let bundle_notes = bundle_notes_for_index_for_round(
        &session.snapshot.note_infos,
        layout,
        bundle_index,
        &session.db,
        &session.round.round_id,
    )?;
    let delegation_keys = DelegationKeys::with_voting_hotkey(
        session.snapshot.account.orchard_fvk_bytes.to_vec(),
        hotkey,
        session.snapshot.account.seed_fingerprint,
        session.snapshot.account.account_index,
        round_name.clone(),
    )
    .map_err(VotingFailure::Sdk)?;
    Ok(PreparedDelegationBundle {
        round_id: session.round.round_id.clone(),
        round_params: session.round.params.clone(),
        bundle_index,
        layout: layout.clone(),
        bundle_note_infos: bundle_notes,
        delegation_keys,
        branch_id_provider: lwd_inputs.branch_id_provider.clone(),
        anchor_tree_state_bytes: lwd_inputs.anchor_tree_state_bytes.clone(),
        network: session.config.network,
        round_name,
    })
}

fn delegate_bundle(
    session: &ArmedVotingSession,
    hotkey: &VotingHotkey,
    signer: &DelegationSigner,
    pir: &PirFleet,
    lwd_inputs: &DelegationLwdInputs,
    layout: &BundleLayout,
    bundle_index: u32,
) -> Result<(), VotingFailure> {
    let db = session.db.as_ref();
    let prepared = prepare_bundle(session, hotkey, lwd_inputs, layout, bundle_index)?;

    let setup = prepared
        .setup(db, &NoopProgressReporter)
        .map_err(VotingFailure::Sdk)?;
    let request = prepared.signing_request(db).map_err(VotingFailure::Sdk)?;
    let signature = sign_request(signer, request.clone())?;

    pir.with_failover(|pir_session| prepared.prove(db, pir_session, &NoopProgressReporter))
        .map_err(VotingFailure::Sdk)?;

    prepared
        .signed_bundle(
            db,
            setup.pczt_bytes,
            PreparedSigner::signature(signature, request.sighash),
        )
        .map_err(VotingFailure::Sdk)?;

    session.events.publish(VotingEvent::DelegationSigned {
        bundle_index,
        delegated_weight_zatoshi: prepared
            .delegated_weight_zatoshi()
            .map_err(VotingFailure::Sdk)?,
    });
    Ok(())
}

async fn advance_delegation(
    session: &ArmedVotingSession,
    hotkey: &VotingHotkey,
    signer: &DelegationSigner,
    chain: &ChainSubmissionClient<HyperTransport>,
    control: &ChainSubmissionControl,
    lwd_inputs: &DelegationLwdInputs,
    layout: &BundleLayout,
    bundle_index: u32,
) -> Result<(), VotingFailure> {
    // Re-sign the persisted setup so a crash between signing and dispatch
    // recovers without carrying the signature in memory. The randomizer and
    // sighash are durable, so the re-derived signature is the one the chain
    // lifecycle locks against.
    let prepared = prepare_bundle(session, hotkey, lwd_inputs, layout, bundle_index)?;
    let request = prepared
        .signing_request(&session.db)
        .map_err(VotingFailure::Sdk)?;
    let signature = sign_request(signer, request)?;

    let request_id = AdvanceDelegation::from_signature_bytes(
        round_bytes(&session.round.round_id)?,
        bundle_index,
        &signature,
    )
    .map_err(|failure| VotingFailure::VoteServer(failure.to_string()))?;
    let result = chain
        .advance_delegation_with_recovery(request_id, ChainRecoveryMode::ExactTree, control)
        .await
        .map_err(|failure| VotingFailure::VoteServer(failure.to_string()))?;
    publish_chain_event(session, ChainStage::Delegation { bundle_index }, &result);
    Ok(())
}

enum ChainStage {
    Delegation { bundle_index: u32 },
    Vote { bundle_index: u32, proposal_id: u32 },
}

fn publish_chain_event(
    session: &ArmedVotingSession,
    stage: ChainStage,
    result: &ChainSubmissionResult,
) {
    match (stage, result) {
        (ChainStage::Delegation { bundle_index }, ChainSubmissionResult::Confirmed(_)) => {
            session
                .events
                .publish(VotingEvent::DelegationConfirmed { bundle_index });
        }
        (
            ChainStage::Delegation { bundle_index },
            ChainSubmissionResult::Pending(_) | ChainSubmissionResult::Rejected(_),
        ) => {
            session
                .events
                .publish(VotingEvent::DelegationSubmitted { bundle_index });
        }
        (
            ChainStage::Vote {
                bundle_index,
                proposal_id,
            },
            ChainSubmissionResult::Confirmed(_),
        ) => {
            session.events.publish(VotingEvent::VoteConfirmed {
                bundle_index,
                proposal_id,
            });
        }
        (
            ChainStage::Vote {
                bundle_index,
                proposal_id,
            },
            ChainSubmissionResult::Pending(_) | ChainSubmissionResult::Rejected(_),
        ) => {
            session.events.publish(VotingEvent::VoteCommitted {
                bundle_index,
                proposal_id,
            });
        }
        _ => {}
    }
}

fn witness_and_draft(
    session: &ArmedVotingSession,
    bundle_index: u32,
    proposal_id: u32,
    choice: u32,
) -> Result<(VanWitness, DraftVote), VotingFailure> {
    let tree_url = session
        .config
        .vote_tree_node_urls
        .clone()
        .unwrap_or_default()
        .into_iter()
        .next()
        .ok_or_else(|| VotingFailure::VoteServer("no vote-tree node is configured".to_string()))?;
    let anchor_height = sync_vote_tree(&session.db, &session.round.round_id, &tree_url)
        .map_err(VotingFailure::Sdk)?;
    let witness = van_witness(
        &session.db,
        &session.round.round_id,
        bundle_index,
        anchor_height,
    )
    .map_err(VotingFailure::Sdk)?;

    let num_options = session
        .round
        .proposals
        .iter()
        .find(|proposal| proposal.id == proposal_id)
        .map(|proposal| proposal.num_options)
        .ok_or_else(|| {
            VotingFailure::Sdk(VotingError::InvalidInput {
                message: format!("proposal {proposal_id} is not on the round roster"),
            })
        })?;
    let draft = DraftVote {
        proposal_id,
        choice,
        num_options,
        vc_tree_position: 0,
        single_share: false,
    };
    Ok((witness, draft))
}

fn cast_vote_step(
    session: &ArmedVotingSession,
    hotkey: &VotingHotkey,
    bundle_index: u32,
    proposal_id: u32,
    choice: u32,
) -> Result<(), VotingFailure> {
    let (witness, draft) = witness_and_draft(session, bundle_index, proposal_id, choice)?;
    CommittedVote::commit(
        &session.db,
        &session.round.round_id,
        bundle_index,
        &draft,
        &witness,
        zcash_voting::vote::VoteSigner::hotkey(hotkey),
        &NoopProgressReporter,
    )
    .map_err(VotingFailure::Sdk)?;
    session.events.publish(VotingEvent::VoteCommitted {
        bundle_index,
        proposal_id,
    });
    Ok(())
}

async fn advance_vote(
    session: &ArmedVotingSession,
    chain: &ChainSubmissionClient<HyperTransport>,
    control: &ChainSubmissionControl,
    bundle_index: u32,
    proposal_id: u32,
) -> Result<(), VotingFailure> {
    let request = AdvanceVote {
        vote_round_id: round_bytes(&session.round.round_id)?,
        bundle_index,
        proposal_id,
    };
    let result = chain
        .advance_vote_with_recovery(request, ChainRecoveryMode::ExactTree, control)
        .await
        .map_err(|failure| VotingFailure::VoteServer(failure.to_string()))?;
    publish_chain_event(
        session,
        ChainStage::Vote {
            bundle_index,
            proposal_id,
        },
        &result,
    );
    Ok(())
}

fn helper_client() -> HelperClient {
    HelperClient::new(Arc::new(HyperTransport::new()), HelperHealth::default())
}

/// Runs one background confirmation-and-recovery pass over the round's
/// pending helper shares.
async fn track_pending_share_confirmations(
    session: &ArmedVotingSession,
) -> Result<(), VotingFailure> {
    let params = zcash_voting::share_tracking::ShareTrackingParams {
        round_id: &session.round.round_id,
        configured_server_urls: &session.helper_urls(),
        now_seconds: now_seconds(),
        vote_end_time_seconds: Some(session.round.vote_end_time_seconds),
        policy: zcash_voting::share_policy::ShareTimingPolicy::default(),
    };
    let report = zcash_voting::share_tracking::track_pending_shares(
        &session.db,
        &params,
        &helper_client(),
        &|| false,
    )
    .await
    .map_err(VotingFailure::Sdk)?;
    for key in &report.confirmed {
        session.events.publish(VotingEvent::ShareDelivered {
            bundle_index: key.bundle_index,
            proposal_id: key.proposal_id,
            share_index: key.share_index,
        });
    }
    Ok(())
}

async fn deliver_shares(
    session: &ArmedVotingSession,
    bundle_index: u32,
    proposal_id: u32,
) -> Result<(), VotingFailure> {
    let committed = CommittedVote::recover(
        &session.db,
        &session.round.round_id,
        bundle_index,
        proposal_id,
    )
    .map_err(VotingFailure::Sdk)?;
    let client = helper_client();
    let fleet = client
        .preflight_fleet(&session.helper_urls())
        .await
        .map_err(|error| VotingFailure::VoteServer(error.to_string()))?;
    let roster = roster_ids(&session.round);
    committed
        .prepare_share_delivery(
            &session.db,
            ShareDeliveryPlanningParams {
                fleet: &fleet,
                now_seconds: now_seconds(),
                vote_end_time_seconds: session.round.vote_end_time_seconds,
                last_moment_buffer_seconds: None,
                proposal_ids: &roster,
            },
        )
        .map_err(VotingFailure::Sdk)?;
    let share_records = zcash_voting::share::list(&session.db, &session.round.round_id)
        .map_err(VotingFailure::Sdk)?;
    for record in &share_records {
        if record.bundle_index == bundle_index && record.proposal_id == proposal_id {
            session.events.publish(VotingEvent::ShareScheduled {
                bundle_index,
                proposal_id,
                share_index: record.share_index,
            });
        }
    }
    let confirmed = committed
        .confirmed(&session.db)
        .map_err(VotingFailure::Sdk)?
        .ok_or_else(|| {
            VotingFailure::Sdk(VotingError::InvalidInput {
                message: format!(
                    "vote for proposal {proposal_id} is not confirmed; shares wait for confirmation"
                ),
            })
        })?;
    let report = confirmed
        .submit_prepared_shares(
            &session.db,
            &client,
            ShareDeliverySubmissionParams {
                configured_server_urls: &session.helper_urls(),
                now_seconds: now_seconds(),
            },
            &|| false,
        )
        .await
        .map_err(VotingFailure::Sdk)?;
    for delivery in &report.deliveries {
        session.events.publish(VotingEvent::ShareDelivered {
            bundle_index,
            proposal_id,
            share_index: delivery.share_index,
        });
    }
    Ok(())
}

/// Journals the ballot and drives the round's lifecycle to quiescence.
///
/// The loop executes at most one durable step per pass and re-plans from
/// durable state every pass, so any interruption resumes with
/// [`cast_ballot`] again (or the next [`crate::cast_vote`] on the client).
///
/// # Errors
///
/// Returns the first failure the lifecycle hits; durable state keeps the
/// progress already made.
pub async fn cast_ballot(
    session: ArmedVotingSession,
    decisions: &[BallotDecision],
    signer: DelegationSigner,
) -> Result<VotingRoundOutcome, VotingFailure> {
    let round_id = session.round.round_id.clone();
    journal::record_decisions(
        &session.db,
        &round_id,
        session.config.network,
        &round_roster(&session.round),
        decisions,
    )?;

    let vote_server_urls: Vec<String> = session
        .resolved
        .vote_servers
        .iter()
        .map(|server| server.url.clone())
        .collect();
    let chain = ChainSubmissionClient::new(
        Arc::clone(&session.db),
        chain_config(session.config.network, &vote_server_urls)?,
    )
    .map_err(|failure| VotingFailure::VoteServer(failure.to_string()))?;
    let control = ChainSubmissionControl::new(now_seconds());
    let hotkey = load_or_create_hotkey(
        &session.db,
        &round_id,
        session.config.network,
        &session.events,
    )?;
    let pir = PirFleet::new(
        &session.pir_endpoint_urls(),
        session.resolved.pir_layout,
        Arc::new(HyperTransport::new()),
    )
    .map_err(VotingFailure::Sdk)?;

    let mut snapshot_plan: Option<SnapshotBundlePrecomputeReport> = None;
    let mut lwd_inputs: Option<DelegationLwdInputs> = None;
    let mut outcome = VotingRoundOutcome {
        round_id: round_id.clone(),
        all_decided: false,
        blocking_recovery: false,
    };

    for _pass in 0..MAX_LIFECYCLE_PASSES {
        let plan = resume_plan(&session.db, &round_id, &roster_ids(&session.round))?;
        if let Some(step) = plan.next_steps.first().cloned() {
            // Only non-blocking confirmations remain: one background
            // tracking pass reconciles them, and this cast quiesces.
            if matches!(step, NextStep::ConfirmShare { .. }) {
                track_pending_share_confirmations(&session).await?;
                break;
            }
            let snapshot_plan = &mut snapshot_plan;
            let lwd_inputs = &mut lwd_inputs;
            execute_step(
                &session,
                &hotkey,
                &signer,
                &pir,
                &chain,
                &control,
                snapshot_plan,
                lwd_inputs,
                step,
            )
            .await?;
            continue;
        }

        outcome.all_decided = plan.all_decided;
        outcome.blocking_recovery = plan.blocking_recovery;
        break;
    }

    session.events.publish(VotingEvent::Finished {
        round_id: round_id.clone(),
    });
    Ok(outcome)
}

async fn execute_step(
    session: &ArmedVotingSession,
    hotkey: &VotingHotkey,
    signer: &DelegationSigner,
    pir: &PirFleet,
    chain: &ChainSubmissionClient<HyperTransport>,
    control: &ChainSubmissionControl,
    snapshot_plan: &mut Option<SnapshotBundlePrecomputeReport>,
    lwd_inputs: &mut Option<DelegationLwdInputs>,
    step: NextStep,
) -> Result<(), VotingFailure> {
    // The snapshot bundle plan and the anchor treestate are prepared lazily,
    // once, in the pass that first needs them.
    let needs_delegation_inputs = matches!(
        step,
        NextStep::Delegate { .. } | NextStep::AdvanceDelegation { .. }
    );
    if needs_delegation_inputs {
        let layout = match snapshot_plan {
            Some(report) => report.layout.clone(),
            None => {
                let report = precompute_snapshot_plan(session, pir)?;
                for bundle in &report.bundles {
                    session.events.publish(VotingEvent::PirWarmup {
                        cached: bundle.cached,
                        fetched: bundle.fetched,
                    });
                }
                session.events.publish(VotingEvent::SnapshotNotesSelected {
                    note_count: session.snapshot.note_infos.len() as u32,
                    eligible_weight_zatoshi: report.layout.eligible_weight,
                });
                for bundle_index in 0..report.layout.bundle_count {
                    store_bundle_witnesses(session, &report.layout, bundle_index)?;
                    session
                        .events
                        .publish(VotingEvent::BundlePrepared { bundle_index });
                }
                *snapshot_plan = Some(report);
                snapshot_plan
                    .as_ref()
                    .ok_or(VotingFailure::SeedUnavailable)?
                    .layout
                    .clone()
            }
        };
        if lwd_inputs.is_none() {
            *lwd_inputs = Some(resolve_lwd_inputs(session).await?);
        }
        let lwd_inputs = lwd_inputs.as_ref().ok_or(VotingFailure::SeedUnavailable)?;

        return match step {
            NextStep::Delegate { bundle_index } => delegate_bundle(
                session,
                hotkey,
                signer,
                pir,
                lwd_inputs,
                &layout,
                bundle_index,
            ),
            NextStep::AdvanceDelegation { bundle_index } => {
                advance_delegation(
                    session,
                    hotkey,
                    signer,
                    chain,
                    control,
                    lwd_inputs,
                    &layout,
                    bundle_index,
                )
                .await
            }
            _ => Ok(()),
        };
    }

    match step {
        NextStep::CastVote {
            bundle_index,
            proposal_id,
            choice,
        } => cast_vote_step(session, hotkey, bundle_index, proposal_id, choice),
        NextStep::AdvanceVote {
            bundle_index,
            proposal_id,
        } => advance_vote(session, chain, control, bundle_index, proposal_id).await,
        NextStep::SubmitShares {
            bundle_index,
            proposal_id,
            ..
        } => deliver_shares(session, bundle_index, proposal_id).await,
        NextStep::ConfirmShare { .. } => Ok(()),
        NextStep::AdvanceImportedDelegation { bundle_index } => {
            Err(VotingFailure::SigningUnsupported {
                message: format!(
                    "bundle {bundle_index} holds an imported delegation; \
                 capability import is out of scope for this phase"
                ),
            })
        }
        NextStep::AdvanceVoteBatch { .. } => Err(VotingFailure::SigningUnsupported {
            message: "atomic vote batches are not planned by this session".to_string(),
        }),
        _ => Ok(()),
    }
}

/// Reads a round's durable results from its sidecar.
///
/// # Errors
///
/// Returns [`VotingFailure::Sdk`] when a read fails.
pub fn voting_results(db: &VotingDb, round_id: &str) -> Result<VotingResults, VotingFailure> {
    Ok(VotingResults {
        round_id: round_id.to_string(),
        votes: db.get_votes(round_id)?,
        shares: zcash_voting::share::list(db, round_id)?,
        round_phase: db.get_round_state(round_id).ok().map(|state| state.phase),
    })
}

impl ArmedVotingSession {
    fn helper_urls(&self) -> Vec<String> {
        self.config.helper_server_urls.clone().unwrap_or_else(|| {
            self.resolved
                .vote_servers
                .iter()
                .map(|server| server.url.clone())
                .collect()
        })
    }

    fn pir_endpoint_urls(&self) -> Vec<String> {
        self.resolved
            .pir_endpoints
            .iter()
            .map(|endpoint| endpoint.url.clone())
            .collect()
    }
}

/// A round this wallet already went through, from the sidecar: the round's
/// identity plus every stored vote, for "previous ballots" surfaces.
#[derive(Clone, Debug)]
pub struct PastBallot {
    pub round_id: String,
    pub snapshot_height: u64,
    pub created_at: u64,
    pub phase: zcash_voting::storage::RoundPhase,
    pub votes: Vec<zcash_voting::wire::VoteRecord>,
}

/// Lists the wallet's past voting rounds with their recorded votes, newest
/// first. Rounds whose votes could not be read are skipped rather than
/// failing the whole history.
pub fn past_ballots(db: &VotingDb) -> Result<Vec<PastBallot>, VotingFailure> {
    let mut ballots = Vec::new();
    for round in db.list_rounds().map_err(VotingFailure::from)? {
        let votes = match db.get_votes(&round.round_id) {
            Ok(votes) => votes,
            Err(_error) => Vec::new(),
        };
        ballots.push(PastBallot {
            round_id: round.round_id,
            snapshot_height: round.snapshot_height,
            created_at: round.created_at,
            phase: round.phase,
            votes,
        });
    }
    Ok(ballots)
}
