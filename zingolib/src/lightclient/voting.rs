//! LightClient's voting surface, the client-side half of the voting
//! lifecycle.
//!
//! The logic lives in `zingolib-voting`; these methods supply what only the
//! client knows: the wallet's snapshot notes and Merkle witnesses (read
//! under the wallet lock, from pepper-sync's orchard shard tree at the
//! snapshot height), the ZIP-32 account context, and software signing inside
//! the seed boundary — the client derives the spending key it already owns,
//! randomizes by the request's `alpha`, and signs the request's ZIP-244
//! sighash with RedPallas. The public methods are thin delegates over
//! [`zingolib_voting`], following the progress-handle idiom: grab the handle
//! from [`LightClient::begin_voting`], subscribe, then drive the round with
//! [`LightClient::cast_vote`].

use std::sync::Arc;

use incrementalmerkletree::Position;
use orchard::tree::MerkleHashOrchard;
use pepper_sync::wallet::{IronwoodNote, NoteInterface, OutputInterface};
use zcash_keys::keys::{UnifiedFullViewingKey, UnifiedSpendingKey};
use zcash_protocol::consensus::BlockHeight;
use zingolib_voting::{
    ActiveRoundSummary, BallotDecision, DelegationAccountKeys, DelegationSigner,
    DelegationSigningRequest, VotingDb, VotingProgressHandle, VotingResults, VotingRoundOutcome,
    VotingSessionConfig, VotingSnapshot, arm_session, cast_ballot, resolve_active_round,
    voting_results,
};
use zip32::fingerprint::SeedFingerprint;

use super::LightClient;
use super::error::LightClientError;
use crate::wallet::error::KeyError;

/// Folds a typed voting failure into the client error.
fn voting_failure(failure: zingolib_voting::VotingFailure) -> LightClientError {
    LightClientError::Voting(failure)
}

/// Folds an SDK voting error into the client error through the failure type.
fn sdk_failure(error: zingolib_voting::VotingError) -> LightClientError {
    LightClientError::Voting(zingolib_voting::VotingFailure::Sdk(error))
}

/// Folds a wallet key failure into the client error.
fn key_error(error: KeyError) -> LightClientError {
    LightClientError::WalletError(error.into())
}

impl LightClient {
    /// The config source this client resolves: the env override when set,
    /// otherwise the production anchor. Surfaced so wallets can show which
    /// voting network they are talking to.
    pub fn voting_source(&self) -> String {
        std::env::var(zingolib_voting::STATIC_CONFIG_SOURCE_ENV)
            .unwrap_or_else(|_| zingolib_voting::PRODUCTION_STATIC_VOTING_CONFIG_SOURCE.to_string())
    }

    /// The wallet-facing summary of the active voting round: authenticated
    /// metadata plus the wallet's bundle-quantized voting weight at the
    /// round's snapshot height.
    ///
    /// The weight covers exactly the notes this wallet can witness at the
    /// snapshot; a pruned snapshot checkpoint surfaces as a typed failure
    /// rather than an understated weight.
    pub async fn active_voting_round(
        &mut self,
        account_id: zip32::AccountId,
    ) -> Result<ActiveRoundSummary, LightClientError> {
        let config = self.voting_config()?;
        let (_resolved, _rounds, round) = resolve_active_round(&config).await?;
        let snapshot = self
            .voting_snapshot(account_id, round.params.snapshot_height)
            .await?;
        let (eligibility, plan) = zingolib_voting::minimum_voting_eligibility_and_plan_for_notes(
            &snapshot.note_infos,
            zingolib_voting::BundlePolicy::default(),
        )
        .map_err(sdk_failure)?;
        Ok(ActiveRoundSummary {
            round,
            eligible_weight_zatoshi: eligibility.eligible_weight,
            bundle_count: plan.bundles.len() as u32,
        })
    }

    /// Arms a voting session for `round_id` and returns its typed event
    /// handle. Subscribe to the handle before calling
    /// [`LightClient::cast_vote`]; events published earlier are not replayed.
    ///
    /// # Errors
    ///
    /// Returns [`LightClientError::Offline`] without a connected indexer and
    /// [`LightClientError::Voting`] for config resolution, round
    /// authentication, snapshot, or sidecar failures.
    pub async fn begin_voting(
        &mut self,
        account_id: zip32::AccountId,
        round_id: &str,
    ) -> Result<VotingProgressHandle, LightClientError> {
        let config = self.voting_config()?;
        let (_resolved, _rounds, round) = resolve_active_round(&config).await?;
        let snapshot = self
            .voting_snapshot(account_id, round.params.snapshot_height)
            .await?;
        let db = self.voting_sidecar()?;
        let session = arm_session(config, Some(round_id), snapshot, db).await?;
        let handle = session.progress_handle();
        self.voting = Some(session);
        Ok(handle)
    }

    /// Journals the ballot and drives the armed round's lifecycle to
    /// quiescence, publishing typed [`zingolib_voting::VotingEvent`]s to the
    /// handle [`LightClient::begin_voting`] returned. Durable progress
    /// survives every interruption: call again to resume from the sidecar.
    ///
    /// # Errors
    ///
    /// Returns [`LightClientError::Voting`] with
    /// [`VotingFailure::SessionNotArmed`](zingolib_voting::VotingFailure::SessionNotArmed)
    /// when [`LightClient::begin_voting`] has not armed the requested round,
    /// and the first lifecycle failure otherwise.
    pub async fn cast_vote(
        &mut self,
        round_id: &str,
        decisions: Vec<BallotDecision>,
    ) -> Result<VotingRoundOutcome, LightClientError> {
        let session = self
            .voting
            .take()
            .filter(|session| session.round_id() == round_id)
            .ok_or_else(|| zingolib_voting::VotingFailure::SessionNotArmed {
                round_id: round_id.to_string(),
            })?;
        let signer = self.voting_signer()?;
        cast_ballot(session, &decisions, signer)
            .await
            .map_err(LightClientError::Voting)
    }

    /// Every round the configured vote network publishes: the
    /// network-wide overview. Needs the config source to resolve, so it
    /// reaches staging today and production once that deploys.
    pub async fn network_rounds(
        &self,
    ) -> Result<Vec<zingolib_voting::ServerRound>, LightClientError> {
        let config = self.voting_config()?;
        let (_resolved, rounds) = zingolib_voting::list_network_rounds(&config).await?;
        Ok(rounds)
    }

    /// A finalized round's network tally: aggregate zatoshi per
    /// `(proposal, decision)`. Tries the config's vote servers in order.
    pub async fn tally_results(
        &self,
        round_id: &str,
    ) -> Result<Vec<zingolib_voting::TallyRow>, LightClientError> {
        let config = self.voting_config()?;
        let route = zingolib_voting::DirectRoute::default();
        let resolved =
            zingolib_voting::resolve_voting_config(&route, &config.static_config_source).await?;
        let mut last_error = None;
        for server in &resolved.vote_servers {
            match zingolib_voting::tally_results(&route, &server.url, round_id).await {
                Ok(tallies) => return Ok(tallies),
                Err(error) => last_error = Some(error),
            }
        }
        Err(LightClientError::Voting(last_error.unwrap_or(
            zingolib_voting::VotingFailure::VoteServer("no vote servers configured".to_string()),
        )))
    }

    /// The wallet's past voting rounds with their recorded votes, newest
    /// first, from the sidecar.
    pub async fn past_ballots(&self) -> Result<Vec<zingolib_voting::PastBallot>, LightClientError> {
        let db = self.voting_sidecar()?;
        zingolib_voting::past_ballots(&db).map_err(LightClientError::Voting)
    }

    /// Reads a round's durable results from its sidecar.
    pub async fn voting_results(&self, round_id: &str) -> Result<VotingResults, LightClientError> {
        let db = self.voting_sidecar()?;
        voting_results(&db, round_id).map_err(LightClientError::Voting)
    }

    /// Reads back a journaled decision per proposal, for UI state across
    /// restarts.
    pub async fn journaled_decisions(
        &self,
        round_id: &str,
    ) -> Result<Vec<BallotDecision>, LightClientError> {
        let db = self.voting_sidecar()?;
        zingolib_voting::journal::journaled_decisions(&db, round_id)
            .map(|intents| {
                intents
                    .into_iter()
                    .map(|intent| BallotDecision {
                        proposal_id: intent.proposal_id,
                        decision: intent.decision,
                    })
                    .collect()
            })
            .map_err(LightClientError::Voting)
    }

    /// Clears one journaled decision before any vote artifact exists.
    pub async fn clear_voting_decision(
        &self,
        round_id: &str,
        proposal_id: u32,
    ) -> Result<(), LightClientError> {
        let db = self.voting_sidecar()?;
        zingolib_voting::journal::clear_decision(&db, round_id, proposal_id)
            .map_err(LightClientError::Voting)
    }

    /// The session config for this client's chain and indexer: the
    /// production source by default. Staging and custom networks opt in
    /// through `ZINGO_VOTING_STATIC_CONFIG_SOURCE`, which overrides every
    /// configured source at resolution time.
    fn voting_config(&self) -> Result<VotingSessionConfig, LightClientError> {
        let indexer = self.require_indexer()?;
        Ok(VotingSessionConfig::production(
            voting_network(self.chain_type()),
            indexer.uri().to_string(),
        ))
    }

    /// Opens (or attaches to) the voting sidecar next to the wallet file,
    /// scoped to this wallet's seed fingerprint.
    fn voting_sidecar(&self) -> Result<Arc<VotingDb>, LightClientError> {
        let wallet_id = self.voting_wallet_id().ok_or(LightClientError::Voting(
            zingolib_voting::VotingFailure::SeedUnavailable,
        ))?;
        VotingDb::open_wallet_sidecar(&self.wallet_path(), &wallet_id)
            .map_err(|error| voting_failure(zingolib_voting::VotingFailure::Sdk(error)))
    }

    /// The wallet scope every sidecar row is keyed by: the ZIP-32 seed
    /// fingerprint hex, stable across renames and restores.
    fn voting_wallet_id(&self) -> Option<String> {
        self.voting_seed().map(|seed| {
            SeedFingerprint::from_seed(&seed)
                .map(|fingerprint| {
                    fingerprint
                        .to_bytes()
                        .iter()
                        .map(|byte| format!("{byte:02x}"))
                        .collect::<String>()
                })
                .unwrap_or_default()
        })
    }

    /// The wallet's seed bytes, when this is a spending wallet.
    fn voting_seed(&self) -> Option<[u8; 64]> {
        self.wallet
            .mnemonic
            .as_ref()
            .map(|mnemonic| mnemonic.to_seed(""))
    }

    /// Selects the wallet's snapshot-eligible Ironwood notes and witnesses
    /// them at the snapshot height from the orchard shard tree.
    ///
    /// Notes the wallet cannot witness (the snapshot checkpoint is pruned)
    /// fail the whole snapshot: a round needs every selected note proven, so
    /// a partial set would silently drop voting weight.
    async fn voting_snapshot(
        &self,
        account_id: zip32::AccountId,
        snapshot_height: u64,
    ) -> Result<VotingSnapshot, LightClientError> {
        let snapshot_height = u32::try_from(snapshot_height).map_err(|_| {
            LightClientError::Voting(zingolib_voting::VotingFailure::VoteServer(format!(
                "snapshot height {snapshot_height} does not fit in u32"
            )))
        })?;
        let anchor = BlockHeight::from_u32(snapshot_height);
        let wallet = self.wallet().read().await;
        let key_store = wallet
            .unified_key_store
            .get(&account_id)
            .ok_or_else(|| key_error(KeyError::NoAccountKeys))?;
        let ufvk = UnifiedFullViewingKey::try_from(key_store).map_err(key_error)?;
        let orchard_fvk = ufvk
            .orchard()
            .ok_or_else(|| key_error(KeyError::NoViewCapability))?;
        let seed_fingerprint = SeedFingerprint::from_seed(
            &self
                .voting_seed()
                .ok_or_else(|| key_error(KeyError::NoSpendCapability))?,
        )
        .ok_or_else(|| key_error(KeyError::NoSpendCapability))?
        .to_bytes();
        let account = DelegationAccountKeys {
            account_index: u32::from(account_id),
            orchard_fvk_bytes: orchard_fvk.to_bytes(),
            seed_fingerprint,
        };

        let exclude: Vec<pepper_sync::wallet::OutputId> = Vec::new();
        let notes = wallet.spendable_notes::<IronwoodNote>(anchor, &exclude, account_id, false)?;
        let mut note_infos = Vec::with_capacity(notes.len());
        let mut witnesses = Vec::with_capacity(notes.len());
        for note in notes {
            let position = u64::from(
                note.position()
                    .ok_or_else(|| key_error(KeyError::NoViewCapability))?,
            );
            let path = wallet
                .shard_trees
                .orchard
                .witness_at_checkpoint_id(Position::from(position), &anchor)
                .map_err(|error| {
                    LightClientError::Voting(zingolib_voting::VotingFailure::Sdk(
                        zingolib_voting::VotingError::Internal {
                            message: format!("shard tree witness failed: {error}"),
                        },
                    ))
                })?
                .ok_or(LightClientError::Voting(
                    zingolib_voting::VotingFailure::SnapshotWitnessPruned {
                        height: u64::from(snapshot_height),
                        position,
                    },
                ))?;
            let note_info = zingolib_voting::NoteInfo::from_orchard_note(
                note.note(),
                position,
                note.key_id().scope,
                &ufvk,
                &self.chain_type(),
            )
            .map_err(sdk_failure)?;
            let commitment = note.note().commitment().into();
            let leaf = MerkleHashOrchard::from_cmx(&commitment);
            witnesses.push(zingolib_voting::WitnessData {
                note_commitment: commitment.to_bytes().to_vec(),
                position,
                root: path.root(leaf).to_bytes().to_vec(),
                auth_path: path
                    .path_elems()
                    .iter()
                    .map(|hash| hash.to_bytes().to_vec())
                    .collect(),
            });
            note_infos.push(note_info);
        }

        Ok(VotingSnapshot {
            note_infos,
            witnesses,
            account,
        })
    }

    /// The software signer, built inside the client's seed boundary.
    ///
    /// The closure verifies the request routes to this wallet's seed and
    /// derives the account spending key the client already owns, randomizes
    /// by the request's `alpha`, and signs the request's ZIP-244 sighash with
    /// RedPallas. It never returns key material, only the signature.
    fn voting_signer(&self) -> Result<DelegationSigner, LightClientError> {
        let chain_type = self.chain_type();
        let seed = self
            .voting_seed()
            .ok_or_else(|| voting_failure(zingolib_voting::VotingFailure::SeedUnavailable))?;
        let fingerprint = SeedFingerprint::from_seed(&seed)
            .ok_or_else(|| key_error(KeyError::NoSpendCapability))?
            .to_bytes();
        let signer = move |request: DelegationSigningRequest| -> Result<
            [u8; 64],
            zingolib_voting::VotingError,
        > {
            if request.seed_fingerprint != fingerprint {
                return Err(zingolib_voting::VotingError::InvalidInput {
                    message:
                        "delegation signing request targets another wallet's seed fingerprint"
                            .to_string(),
                });
            }
            let account = zip32::AccountId::try_from(request.account_index)
                .map_err(|error| zingolib_voting::VotingError::InvalidInput {
                    message: format!(
                        "delegation signing request account index {} is invalid: {error}",
                        request.account_index
                    ),
                })?;
            let usk = UnifiedSpendingKey::from_seed(&chain_type, &seed, account).map_err(
                |_| zingolib_voting::VotingError::InvalidInput {
                    message: format!(
                        "account index {} has no spending key derived from this wallet's seed",
                        request.account_index
                    ),
                },
            )?;
            use zingolib_voting::pasta_curves::group::ff::PrimeField as _;
            let alpha =
                Option::<zingolib_voting::pasta_curves::pallas::Scalar>::from(
                    zingolib_voting::pasta_curves::pallas::Scalar::from_repr(request.alpha),
                )
                .ok_or(zingolib_voting::VotingError::InvalidInput {
                    message: "delegation alpha is not a valid Pallas scalar".to_string(),
                })?;
            let ask = orchard::keys::SpendAuthorizingKey::from(usk.orchard());
            let randomized = ask.randomize(&alpha);
            let signature = randomized.sign(&mut crate::utils::system_rng(), &request.sighash);
            Ok((&signature).into())
        };
        Ok(DelegationSigner::Software(Arc::new(signer)))
    }
}

/// Maps this client's chain to the vote protocol's network.
fn voting_network(chain_type: crate::config::ChainType) -> zingolib_voting::Network {
    match chain_type {
        crate::config::ChainType::Mainnet => zingolib_voting::Network::Mainnet,
        crate::config::ChainType::Testnet => zingolib_voting::Network::Testnet,
        crate::config::ChainType::Regtest(_) => zingolib_voting::Network::Regtest,
    }
}
