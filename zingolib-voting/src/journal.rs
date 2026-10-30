//! The durable decision journal and round hotkey storage over the sidecar DB.
//!
//! Ballot decisions are recorded before any vote artifact exists, keyed by
//! the round's authenticated roster and each proposal's option count. The
//! hotkey secret lives in the sidecar's own table, so a crash between
//! generation and delegation never loses the key that spends the round's
//! confirmed vote-authority note. Both persist through the SDK's
//! [`VotingDb`], next to the wallet DB.

use crate::error::VotingFailure;
use zcash_voting::round::VotingDb;
use zcash_voting::session::Decision;
use zcash_voting::types::{Network, VotingError};
use zcash_voting::vote_work::BallotIntent;

/// Maps a raw sidecar SQLite failure onto the SDK's storage error.
fn storage(error: rusqlite::Error) -> VotingFailure {
    VotingFailure::Sdk(VotingError::Storage {
        message: error.to_string(),
    })
}

/// One ballot decision to journal.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BallotDecision {
    pub proposal_id: u32,
    pub decision: Decision,
}

/// Records every decision durably, refusing any proposal outside the round's
/// authenticated roster and any choice outside its option count.
///
/// # Errors
///
/// Returns [`VotingFailure::Sdk`] when the roster disagrees with the round's
/// stored network, a decision exceeds its option count, or the write fails.
pub fn record_decisions(
    db: &VotingDb,
    round_id: &str,
    network: Network,
    roster: &[(u32, u32)],
    decisions: &[BallotDecision],
) -> Result<(), VotingFailure> {
    let intents: Vec<(u32, Decision, u32)> = decisions
        .iter()
        .map(|ballot| {
            let num_options = roster
                .iter()
                .find(|(proposal_id, _)| *proposal_id == ballot.proposal_id)
                .map(|&(_, options)| options)
                .ok_or_else(|| {
                    VotingFailure::Sdk(VotingError::InvalidInput {
                        message: format!(
                            "proposal {} is not on round {round_id}'s authenticated roster",
                            ballot.proposal_id
                        ),
                    })
                })?;
            Ok((ballot.proposal_id, ballot.decision, num_options))
        })
        .collect::<Result<Vec<_>, VotingFailure>>()?;
    db.set_ballot_intents(round_id, network, &intents)?;
    Ok(())
}

/// Reads the journaled decisions as ballot intents.
///
/// # Errors
///
/// Returns [`VotingFailure::Sdk`] when the read fails.
pub fn journaled_decisions(
    db: &VotingDb,
    round_id: &str,
) -> Result<Vec<BallotIntent>, VotingFailure> {
    let decisions = db.ballot_intents(round_id)?;
    Ok(decisions
        .into_iter()
        .map(|(proposal_id, decision)| BallotIntent {
            proposal_id,
            decision,
        })
        .collect())
}

/// Clears one journaled decision.
///
/// # Errors
///
/// Returns [`VotingFailure::Sdk`] when the proposal's vote is already owned by
/// the chain lifecycle; clearing those is refused by the SDK.
pub fn clear_decision(
    db: &VotingDb,
    round_id: &str,
    proposal_id: u32,
) -> Result<(), VotingFailure> {
    db.clear_ballot_intent(round_id, proposal_id)?;
    Ok(())
}

/// Creates the hotkey table if this sidecar does not have it yet.
fn ensure_hotkey_table(conn: &rusqlite::Connection) -> Result<(), VotingFailure> {
    conn.execute(
        "CREATE TABLE IF NOT EXISTS zingo_voting_hotkeys (
            wallet_id TEXT NOT NULL,
            round_id TEXT NOT NULL,
            secret BLOB NOT NULL,
            PRIMARY KEY (wallet_id, round_id)
        )",
        (),
    )
    .map_err(storage)?;
    Ok(())
}

/// Stores the round's voting hotkey secret bytes, replacing any earlier value.
///
/// # Errors
///
/// Returns [`VotingFailure::Sdk`] when the write fails.
pub fn store_hotkey_secret(
    db: &VotingDb,
    round_id: &str,
    secret: &[u8],
) -> Result<(), VotingFailure> {
    let wallet_id = db.wallet_id();
    let conn = db.conn();
    ensure_hotkey_table(&conn)?;
    conn.execute(
        "INSERT INTO zingo_voting_hotkeys (wallet_id, round_id, secret)
         VALUES (?1, ?2, ?3)
         ON CONFLICT(wallet_id, round_id) DO UPDATE SET secret = excluded.secret",
        (&wallet_id, round_id, secret),
    )
    .map_err(storage)?;
    Ok(())
}

/// Loads the round's voting hotkey secret bytes, if one is stored.
///
/// # Errors
///
/// Returns [`VotingFailure::Sdk`] when the read fails.
pub fn load_hotkey_secret(db: &VotingDb, round_id: &str) -> Result<Option<Vec<u8>>, VotingFailure> {
    let wallet_id = db.wallet_id();
    let conn = db.conn();
    ensure_hotkey_table(&conn)?;
    let secret = conn
        .query_row(
            "SELECT secret FROM zingo_voting_hotkeys WHERE wallet_id = ?1 AND round_id = ?2",
            (&wallet_id, round_id),
            |row| row.get::<_, Vec<u8>>(0),
        )
        .map(Some)
        .or_else(|error| match error {
            rusqlite::Error::QueryReturnedNoRows => Ok(None),
            other => Err(storage(other)),
        })?;
    Ok(secret)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn memory_db() -> VotingDb {
        let db = VotingDb::open_in_memory().expect("in-memory sidecar opens");
        db.set_wallet_id("test");
        db
    }

    fn init_round(db: &VotingDb, round_id: &str, network: Network) {
        let params = zcash_voting::wire::VotingRoundParams {
            vote_round_id: round_id.to_string(),
            snapshot_height: 1,
            ea_pk: vec![0u8; 32],
            nc_root: vec![0u8; 32],
            nullifier_imt_root: vec![0u8; 32],
        };
        db.init_round(network, &params, None)
            .expect("round initializes");
    }

    fn decision(proposal_id: u32, choice: u32) -> BallotDecision {
        BallotDecision {
            proposal_id,
            decision: Decision::Choice(choice),
        }
    }

    #[test]
    fn decisions_round_trip_through_the_journal() {
        let db = memory_db();
        init_round(&db, "round", Network::Regtest);
        let roster = [(1u32, 2u32), (2, 3)];
        record_decisions(
            &db,
            "round",
            Network::Regtest,
            &roster,
            &[
                decision(1, 0),
                BallotDecision {
                    proposal_id: 2,
                    decision: Decision::Skipped,
                },
            ],
        )
        .expect("decisions journal");

        let intents = journaled_decisions(&db, "round").expect("read back");
        assert_eq!(intents.len(), 2);
        assert!(
            intents
                .iter()
                .any(|intent| intent.proposal_id == 1 && intent.decision == Decision::Choice(0))
        );
        assert!(
            intents
                .iter()
                .any(|intent| intent.proposal_id == 2 && intent.decision == Decision::Skipped)
        );

        clear_decision(&db, "round", 2).expect("clear skipped");
        let intents = journaled_decisions(&db, "round").expect("read back");
        assert_eq!(intents.len(), 1);
        assert_eq!(intents[0].proposal_id, 1);
    }

    #[test]
    fn a_decision_off_the_roster_is_refused() {
        let db = memory_db();
        init_round(&db, "round", Network::Regtest);
        let error = record_decisions(&db, "round", Network::Regtest, &[(1, 2)], &[decision(9, 0)])
            .expect_err("proposal 9 is not on the roster");
        assert!(matches!(error, VotingFailure::Sdk(_)));
    }

    #[test]
    fn a_choice_beyond_the_option_count_is_refused() {
        let db = memory_db();
        init_round(&db, "round", Network::Regtest);
        let error = record_decisions(&db, "round", Network::Regtest, &[(1, 2)], &[decision(1, 5)])
            .expect_err("choice 5 exceeds two options");
        assert!(matches!(error, VotingFailure::Sdk(_)));
    }

    #[test]
    fn a_decision_is_refused_for_a_round_stored_on_another_network() {
        let db = memory_db();
        init_round(&db, "round", Network::Mainnet);
        let error = record_decisions(&db, "round", Network::Regtest, &[(1, 2)], &[decision(1, 0)])
            .expect_err("the round is stored for mainnet");
        assert!(matches!(error, VotingFailure::Sdk(_)));
    }

    #[test]
    fn hotkey_secret_stores_once_and_reloads() {
        let db = memory_db();
        assert_eq!(load_hotkey_secret(&db, "round").expect("empty"), None);
        store_hotkey_secret(&db, "round", &[1, 2, 3]).expect("store");
        assert_eq!(
            load_hotkey_secret(&db, "round").expect("reload"),
            Some(vec![1, 2, 3])
        );
        store_hotkey_secret(&db, "round", &[9]).expect("replace");
        assert_eq!(
            load_hotkey_secret(&db, "round").expect("reload"),
            Some(vec![9])
        );
        assert_eq!(
            load_hotkey_secret(&db, "other-round").expect("other round"),
            None
        );
    }
}
