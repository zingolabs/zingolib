# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added
- Initial release: the voting session crate behind zingolib's `LightClient`
  voting methods. `arm_session` resolves the pinned config, authenticates an
  open round, and prepares the sidecar, hotkey, and snapshot witnesses;
  `cast_ballot` journals the ballot and drives the SDK's round lifecycle to
  quiescence through a plan-driven loop over `resume_plan`, publishing typed
  `VotingEvent`s to a subscribable `VotingProgressHandle`; `voting_results`
  reads the round's durable state. The decision journal and the round's
  voting-hotkey secret persist in the sidecar DB next to the wallet.
