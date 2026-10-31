//! The vote server's rounds endpoint, typed, with round authentication.
//!
//! The server's round metadata (snapshot height, roots, roster) travels as
//! plain JSON; only the `ea_pk` binding is trusted, and it comes from the
//! resolved config's authenticated rounds, never from the server's answer.
//! The wire DTOs here own that parse; `authenticate_round` performs the
//! binding through [`ResolvedVotingConfig::trusted_voting_round_params`].

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use std::time::Duration;
use zcash_voting::config::ResolvedVotingConfig;
use zcash_voting::types::{validate_proposal_id, validate_vote_options};
use zcash_voting::wire::VotingRoundParams;
use zcash_voting::{DirectRoute, RouteHttp, RouteRequest, RouteResponse};

use crate::error::VotingFailure;

/// Response-body ceiling for a rounds fetch. The staging server lists every
/// round since genesis, so the ceiling bounds abuse, not the happy path.
const MAX_ROUNDS_RESPONSE_BYTES: usize = 64 * 1024 * 1024;

/// Request deadline for a rounds fetch.
const ROUNDS_FETCH_TIMEOUT: Duration = Duration::from_secs(30);

/// One proposal as the vote server lists it in a round.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServerProposal {
    pub id: u32,
    pub title: String,
    pub num_options: u32,
    /// What each choice means, in the order the chain indexes decisions
    /// (position = decision index). Wallets show these instead of
    /// "option 1 / option 2".
    pub option_labels: Vec<String>,
}

/// One round as the vote server lists it.
#[derive(Clone, Debug)]
pub struct ServerRound {
    /// Canonical hex round id (the server encodes base64 on the wire).
    pub round_id: String,
    /// The endorsers backing this round; filled by
    /// [`crate::session::list_network_rounds`], empty otherwise.
    pub endorsers: Vec<String>,
    pub snapshot_height: u64,
    pub nc_root: Vec<u8>,
    pub nullifier_imt_root: Vec<u8>,
    pub vote_end_time_seconds: u64,
    /// The server's raw status code. Surfaced, never interpreted: activity is
    /// decided by `vote_end_time_seconds` against the host clock.
    pub status: u32,
    pub proposals: Vec<ServerProposal>,
}

/// One authenticated, open round ready to vote on.
#[derive(Clone, Debug)]
pub struct ActiveVotingRound {
    pub round_id: String,
    /// Round params with `ea_pk` bound from the authenticated config.
    pub params: VotingRoundParams,
    pub proposals: Vec<ServerProposal>,
    pub vote_end_time_seconds: u64,
    pub status: u32,
}

#[derive(serde::Deserialize)]
struct WireRounds {
    rounds: Vec<WireRound>,
}

#[derive(serde::Deserialize)]
struct WireRound {
    vote_round_id: String,
    snapshot_height: u64,
    nc_root: String,
    nullifier_imt_root: String,
    vote_end_time: u64,
    status: u32,
    // The wire carries an `ea_pk` field too; serde drops it as unknown and
    // that is deliberate: the authenticated config's value replaces it.
    proposals: Vec<WireProposal>,
}

#[derive(serde::Deserialize)]
struct WireProposal {
    id: u32,
    #[serde(default)]
    title: String,
    options: Vec<WireOption>,
}

#[derive(serde::Deserialize)]
struct WireOption {
    #[serde(default)]
    label: String,
}

fn hex_round_id(wire_id: &str) -> Result<String, VotingFailure> {
    let bytes = BASE64_STANDARD
        .decode(wire_id)
        .map_err(|_| VotingFailure::VoteServer(format!("round id {wire_id} is not base64")))?;
    if bytes.len() != 32 {
        return Err(VotingFailure::VoteServer(format!(
            "round id {wire_id} decodes to {} bytes, expected 32",
            bytes.len()
        )));
    }
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

fn root_bytes(wire_root: &str, field: &str, round_id: &str) -> Result<Vec<u8>, VotingFailure> {
    let bytes = BASE64_STANDARD.decode(wire_root).map_err(|_| {
        VotingFailure::VoteServer(format!("round {round_id} {field} is not base64"))
    })?;
    if bytes.len() != 32 {
        return Err(VotingFailure::VoteServer(format!(
            "round {round_id} {field} decodes to {} bytes, expected 32",
            bytes.len()
        )));
    }
    Ok(bytes)
}

fn parse_rounds(bytes: &[u8]) -> Result<Vec<ServerRound>, VotingFailure> {
    let wire: WireRounds = serde_json::from_slice(bytes)
        .map_err(|error| VotingFailure::VoteServer(format!("rounds decode failed: {error}")))?;
    wire.rounds
        .iter()
        .map(|round| {
            let round_id = hex_round_id(&round.vote_round_id)?;
            let proposals = round
                .proposals
                .iter()
                .map(|proposal| -> Result<ServerProposal, VotingFailure> {
                    validate_proposal_id(proposal.id)?;
                    let num_options = u32::try_from(proposal.options.len()).map_err(|_| {
                        VotingFailure::VoteServer(format!(
                            "round {} proposal {} option count overflows",
                            round.vote_round_id, proposal.id
                        ))
                    })?;
                    validate_vote_options(num_options)?;
                    Ok(ServerProposal {
                        id: proposal.id,
                        title: proposal.title.clone(),
                        num_options,
                        option_labels: proposal
                            .options
                            .iter()
                            .map(|option| option.label.clone())
                            .collect(),
                    })
                })
                .collect::<Result<Vec<_>, VotingFailure>>()?;
            Ok(ServerRound {
                endorsers: Vec::new(),
                nc_root: root_bytes(&round.nc_root, "nc_root", &round_id)?,
                nullifier_imt_root: root_bytes(
                    &round.nullifier_imt_root,
                    "nullifier_imt_root",
                    &round_id,
                )?,
                round_id,
                snapshot_height: round.snapshot_height,
                vote_end_time_seconds: round.vote_end_time,
                status: round.status,
                proposals,
            })
        })
        .collect()
}

/// Fetches every round the server lists.
///
/// # Errors
///
/// Returns [`VotingFailure::VoteServer`] for transport failures, non-success
/// answers, oversized bodies, and undecodable payloads.
pub async fn fetch_rounds(
    route: &DirectRoute,
    base_url: &str,
) -> Result<Vec<ServerRound>, VotingFailure> {
    let base = base_url.trim_end_matches('/');
    let url = format!("{base}/shielded-vote/v1/rounds");
    let request = RouteRequest {
        method: http::Method::GET,
        url: &url,
        headers: &[],
        body: Vec::new(),
        timeout: ROUNDS_FETCH_TIMEOUT,
        connect_timeout: None,
        max_response_bytes: MAX_ROUNDS_RESPONSE_BYTES,
    };
    let on_dispatch = || {};
    let response: RouteResponse = RouteHttp::execute(route, request, &on_dispatch)
        .await
        .map_err(|error| VotingFailure::VoteServer(error.message))?;
    if !(200..300).contains(&response.status) {
        return Err(VotingFailure::VoteServer(format!(
            "rounds fetch returned HTTP {}",
            response.status
        )));
    }
    parse_rounds(&response.body)
}

/// Authenticates one open round against the resolved config.
///
/// A round qualifies when the config's authenticated rounds list its id; the
/// returned params bind `ea_pk` from that authentication. Among qualifying
/// rounds the one closing soonest wins, so a consumer always acts on the
/// round about to end. The server's own `ea_pk` field is parsed but never
/// compared: authentication replaces it wholesale.
///
/// # Errors
///
/// Returns [`VotingFailure::NoActiveRound`] when no authenticated round is
/// still open at `now_seconds`.
pub fn authenticate_round(
    resolved: &ResolvedVotingConfig,
    rounds: &[ServerRound],
    now_seconds: u64,
) -> Result<ActiveVotingRound, VotingFailure> {
    let mut active: Option<ActiveVotingRound> = None;
    for round in rounds {
        let open = round.vote_end_time_seconds > now_seconds;
        let authenticated = resolved
            .authenticated_rounds
            .iter()
            .any(|candidate| candidate.round_id == round.round_id);
        if !(open && authenticated) {
            continue;
        }
        let supersedes = match active.as_ref() {
            None => true,
            Some(current) => round.vote_end_time_seconds < current.vote_end_time_seconds,
        };
        if !supersedes {
            continue;
        }
        let params = resolved
            .trusted_voting_round_params(
                round.round_id.clone(),
                round.snapshot_height,
                round.nc_root.clone(),
                round.nullifier_imt_root.clone(),
            )
            .map_err(|error| VotingFailure::Config(error.to_string()))?;
        active = Some(ActiveVotingRound {
            round_id: round.round_id.clone(),
            params,
            proposals: round.proposals.clone(),
            vote_end_time_seconds: round.vote_end_time_seconds,
            status: round.status,
        });
    }
    active.ok_or(VotingFailure::NoActiveRound)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn round_json(wire_round_id: &str, vote_end_time: u64) -> String {
        let nc_root = BASE64_STANDARD.encode([7u8; 32]);
        let imt_root = BASE64_STANDARD.encode([9u8; 32]);
        let ea_pk = BASE64_STANDARD.encode([11u8; 32]);
        format!(
            r#"{{"rounds":[{{"vote_round_id":"{wire_round_id}","snapshot_height":4245460,
                "snapshot_blockhash":"AA==","proposals_hash":"AA==",
                "vote_end_time":{vote_end_time},
                "nullifier_imt_root":"{imt_root}","nc_root":"{nc_root}",
                "creator":"sv1test","status":1,"ea_pk":"{ea_pk}",
                "proposals":[{{"id":1,"title":"q","description":"d",
                "options":[{{"label":"Yes"}},{{"index":1,"label":"No"}}]}}]}}]}}"#
        )
    }

    #[test]
    fn rounds_parse_and_round_ids_are_hex() {
        let wire_id = BASE64_STANDARD.encode([0xABu8; 32]);
        let bytes = round_json(&wire_id, 1_000);
        let rounds = parse_rounds(bytes.as_bytes()).expect("fixture parses");
        assert_eq!(rounds.len(), 1);
        let round = &rounds[0];
        assert_eq!(round.round_id, "ab".repeat(32));
        assert_eq!(round.snapshot_height, 4_245_460);
        assert_eq!(round.nc_root, vec![7u8; 32]);
        assert_eq!(round.proposals.len(), 1);
        assert_eq!(round.proposals[0].num_options, 2);
        assert_eq!(
            round.proposals[0].option_labels,
            vec!["Yes".to_string(), "No".to_string()]
        );
        assert_eq!(round.proposals[0].id, 1);
    }

    #[test]
    fn a_round_id_that_is_not_32_bytes_is_rejected() {
        let json = r#"{"rounds":[{"vote_round_id":"AAAA","snapshot_height":1,
            "nc_root":"AAAA","nullifier_imt_root":"AAAA","vote_end_time":1,
            "status":1,"ea_pk":"AA==","proposals":[]}]}"#;
        let error = parse_rounds(json.as_bytes()).expect_err("short id is refused");
        assert!(matches!(error, VotingFailure::VoteServer(_)));
    }

    #[test]
    fn a_roster_id_outside_the_valid_range_is_rejected() {
        let json = format!(
            r#"{{"rounds":[{{"vote_round_id":"{}","snapshot_height":1,
            "nc_root":"{}","nullifier_imt_root":"{}","vote_end_time":1,
            "status":1,"ea_pk":"{}",
            "proposals":[{{"id":51,"title":"q","options":[]}}]}}]}}"#,
            BASE64_STANDARD.encode([1u8; 32]),
            BASE64_STANDARD.encode([2u8; 32]),
            BASE64_STANDARD.encode([3u8; 32]),
            BASE64_STANDARD.encode([4u8; 32]),
        );
        let error = parse_rounds(json.as_bytes()).expect_err("roster id is out of range");
        assert!(matches!(error, VotingFailure::Sdk(_)));
    }
}

/// One `(proposal, decision)` aggregate from the chain's finalized tally.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TallyRow {
    pub proposal_id: u32,
    /// Absent in the wire body when the decision is 0.
    pub decision: u32,
    /// Aggregate voting weight in zatoshi.
    pub total_value_zatoshi: u64,
}

/// Fetches a finalized round's tally from a vote server.
///
/// # Errors
///
/// [`VotingFailure::VoteServer`] on transport, HTTP, or parse failures; the
/// chain only serves tallies after the round reaches `FINALIZED`.
pub async fn tally_results(
    route: &DirectRoute,
    base_url: &str,
    round_id: &str,
) -> Result<Vec<TallyRow>, VotingFailure> {
    let base = base_url.trim_end_matches('/');
    let url = format!("{base}/shielded-vote/v1/tally-results/{round_id}");
    let request = RouteRequest {
        method: http::Method::GET,
        url: &url,
        headers: &[],
        body: Vec::new(),
        timeout: ROUNDS_FETCH_TIMEOUT,
        connect_timeout: None,
        max_response_bytes: MAX_ROUNDS_RESPONSE_BYTES,
    };
    let on_dispatch = || {};
    let response: RouteResponse = RouteHttp::execute(route, request, &on_dispatch)
        .await
        .map_err(|error| VotingFailure::VoteServer(error.message))?;
    if !(200..300).contains(&response.status) {
        return Err(VotingFailure::VoteServer(format!(
            "tally fetch returned HTTP {}",
            response.status
        )));
    }
    parse_tally(&response.body)
}

fn parse_tally(body: &[u8]) -> Result<Vec<TallyRow>, VotingFailure> {
    #[derive(serde::Deserialize)]
    struct WireTally {
        #[serde(default)]
        results: Vec<WireTallyRow>,
    }
    #[derive(serde::Deserialize)]
    struct WireTallyRow {
        proposal_id: u32,
        #[serde(default)]
        vote_decision: Option<u32>,
        total_value: u64,
    }
    let wire: WireTally = serde_json::from_slice(body)
        .map_err(|error| VotingFailure::VoteServer(format!("tally parse failed: {error}")))?;
    Ok(wire
        .results
        .into_iter()
        .map(|row| TallyRow {
            proposal_id: row.proposal_id,
            decision: row.vote_decision.unwrap_or(0),
            total_value_zatoshi: row.total_value,
        })
        .collect())
}

/// The round ids a wallet vendor has endorsed on the vote chain: the
/// vendor's curation list. `None` means the chain has no such endorser
/// configured (HTTP 400/404), which callers treat as "show everything".
pub async fn endorsed_round_ids(
    route: &DirectRoute,
    base_url: &str,
    endorser_id: &str,
) -> Result<Option<Vec<String>>, VotingFailure> {
    let base = base_url.trim_end_matches('/');
    let url = format!("{base}/shielded-vote/v1/endorsed-rounds/{endorser_id}");
    let request = RouteRequest {
        method: http::Method::GET,
        url: &url,
        headers: &[],
        body: Vec::new(),
        timeout: ROUNDS_FETCH_TIMEOUT,
        connect_timeout: None,
        max_response_bytes: MAX_ROUNDS_RESPONSE_BYTES,
    };
    let on_dispatch = || {};
    let response: RouteResponse = RouteHttp::execute(route, request, &on_dispatch)
        .await
        .map_err(|error| VotingFailure::VoteServer(error.message))?;
    if response.status == 400 || response.status == 404 {
        return Ok(None);
    }
    if !(200..300).contains(&response.status) {
        return Err(VotingFailure::VoteServer(format!(
            "endorsed-rounds fetch returned HTTP {}",
            response.status
        )));
    }
    #[derive(serde::Deserialize)]
    struct WireEndorsed {
        vote_round_ids: Vec<String>,
    }
    let wire: WireEndorsed = serde_json::from_slice(&response.body).map_err(|error| {
        VotingFailure::VoteServer(format!("endorsed-rounds parse failed: {error}"))
    })?;
    let ids = wire
        .vote_round_ids
        .iter()
        .filter_map(|encoded| BASE64_STANDARD.decode(encoded).ok())
        .map(|bytes| bytes.iter().map(|byte| format!("{byte:02x}")).collect())
        .collect();
    Ok(Some(ids))
}

/// The ids of every endorser registered on the vote chain.
pub async fn endorsers(route: &DirectRoute, base_url: &str) -> Result<Vec<String>, VotingFailure> {
    let base = base_url.trim_end_matches('/');
    let url = format!("{base}/shielded-vote/v1/endorsers");
    let request = RouteRequest {
        method: http::Method::GET,
        url: &url,
        headers: &[],
        body: Vec::new(),
        timeout: ROUNDS_FETCH_TIMEOUT,
        connect_timeout: None,
        max_response_bytes: MAX_ROUNDS_RESPONSE_BYTES,
    };
    let on_dispatch = || {};
    let response: RouteResponse = RouteHttp::execute(route, request, &on_dispatch)
        .await
        .map_err(|error| VotingFailure::VoteServer(error.message))?;
    if !(200..300).contains(&response.status) {
        return Err(VotingFailure::VoteServer(format!(
            "endorsers fetch returned HTTP {}",
            response.status
        )));
    }
    #[derive(serde::Deserialize)]
    struct WireEndorser {
        endorser_id: String,
    }
    #[derive(serde::Deserialize)]
    struct WireEndorsers {
        #[serde(default)]
        endorsers: Vec<WireEndorser>,
    }
    let wire: WireEndorsers = serde_json::from_slice(&response.body)
        .map_err(|error| VotingFailure::VoteServer(format!("endorsers parse failed: {error}")))?;
    Ok(wire.endorsers.into_iter().map(|e| e.endorser_id).collect())
}
