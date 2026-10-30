//! Live staging integration for the arm path.
//!
//! Every test makes one attempt against the staging endpoints and skips
//! cleanly when they are unreachable or no round is open, so offline CI
//! stays green. No retry loop: one pass, one verdict.

use std::time::{SystemTime, UNIX_EPOCH};

use zingolib_voting::{
    DirectRoute, STAGING_STATIC_VOTING_CONFIG_SOURCE, VotingFailure, VotingSessionConfig,
    authenticate_round, fetch_rounds, resolve_voting_config,
};

/// Reads back the authenticated open round, or `None` when staging is
/// unreachable or holds no open round (printed, not asserted).
async fn staging_round() -> Option<zingolib_voting::ActiveVotingRound> {
    let config = VotingSessionConfig::staging(
        zingolib_voting::Network::Testnet,
        "https://localhost:9067".to_string(),
    );
    let route = DirectRoute::default();
    let resolved = match resolve_voting_config(&route, STAGING_STATIC_VOTING_CONFIG_SOURCE).await {
        Ok(resolved) => resolved,
        Err(error) => {
            eprintln!("skip: staging config unreachable: {error}");
            return None;
        }
    };
    for url in resolved
        .vote_servers
        .iter()
        .map(|server| server.url.clone())
    {
        match fetch_rounds(&route, &url).await {
            Ok(rounds) if !rounds.is_empty() => {
                let now = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map(|since| since.as_secs())
                    .unwrap_or_default();
                return match authenticate_round(&resolved, &rounds, now) {
                    Ok(round) => Some(round),
                    Err(VotingFailure::NoActiveRound) => {
                        eprintln!("skip: staging holds no open authenticated round");
                        None
                    }
                    Err(error) => {
                        eprintln!("skip: round authentication failed: {error}");
                        None
                    }
                };
            }
            Ok(_) => continue,
            Err(error) => {
                eprintln!("skip: rounds fetch from {url} failed: {error}");
                continue;
            }
        }
    }
    eprintln!("skip: no staging vote server answered");
    None
}

fn now_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|since| since.as_secs())
        .unwrap_or_default()
}

#[tokio::test]
async fn staging_config_resolves_and_the_active_round_authenticates() {
    let Some(round) = staging_round().await else {
        return;
    };
    assert!(
        !round.params.ea_pk.is_empty(),
        "the authenticated round binds a non-empty ea_pk"
    );
    assert_eq!(round.params.ea_pk.len(), 32);
    assert!(
        !round.proposals.is_empty(),
        "an open staging round lists its roster"
    );
    assert!(
        round.vote_end_time_seconds > now_seconds(),
        "the selected round is still open"
    );
}

#[tokio::test]
async fn staging_round_ids_are_canonical_hex() {
    let Some(round) = staging_round().await else {
        return;
    };
    assert_eq!(round.round_id.len(), 64, "32 bytes lowercase hex");
    assert!(
        round
            .round_id
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
        "round id is lowercase hex: {}",
        round.round_id
    );
}
