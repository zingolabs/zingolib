use super::*;

/// Use `String` as the wallet error type for testing.
type TestSyncError = SyncError<String>;

/// The number of times one detail may appear across a whole cause chain.
const DETAIL_RENDERINGS: usize = 1;

/// Renders `error` and every link of its source chain, one line per link.
fn chain_rendering(error: &(dyn std::error::Error + 'static)) -> String {
    let mut rendered = error.to_string();
    let mut cursor = error.source();
    while let Some(cause) = cursor {
        rendered.push('\n');
        rendered.push_str(&cause.to_string());
        cursor = cause.source();
    }
    rendered
}

/// HYPOTHESIS: every layer wrapping a failed server request states its
/// own layer only, so the transport status text reaches the reader
/// exactly once across the whole cause chain. Falsified if the status
/// text appears more than once.
#[test]
fn a_failed_server_request_renders_its_status_once() {
    const DETAIL: &str = "the indexer refused the block range";
    let failure: TestSyncError =
        ServerError::RequestFailed(tonic::Status::unavailable(DETAIL)).into();
    let rendered = chain_rendering(&failure);
    assert_eq!(
        rendered.matches(DETAIL).count(),
        DETAIL_RENDERINGS,
        "the chain rendered was {rendered}"
    );
}

/// HYPOTHESIS: the rescan announcement is one self-contained sentence
/// naming the consequence (records cleared, rescan from a height) and
/// the cause (which height disagreed, both tree sizes), so a user
/// reading a single log or error line knows what happened and why.
/// Falsified if any of those five facts leaves the rendering.
#[test]
fn the_rescan_announcement_names_the_consequence_and_the_cause() {
    let announcement: TestSyncError = SyncError::PoolHistoryReopened {
        pool: PoolType::IRONWOOD,
        rescan_from: BlockHeight::from_u32(3_000_000),
        disagreed_at: BlockHeight::from_u32(3_100_000),
        block_metadata_size: 999,
        calculated_size: 7,
    };
    let rendered = announcement.to_string();
    assert!(rendered.starts_with("RESCAN TRIGGERED"), "{rendered}");
    for fact in [
        "cleared back to height 3000000",
        "at height 3100000",
        "commitment tree of 7",
        "the chain reports 999",
    ] {
        assert!(rendered.contains(fact), "missing '{fact}' in: {rendered}");
    }
}

/// HYPOTHESIS: the tree-size refusal names the block whose sizes
/// disagreed, so the trigger is locatable from the error alone.
/// Falsified if the height leaves the rendering.
#[test]
fn the_tree_size_refusal_names_the_disagreeing_block() {
    let refusal = ScanError::IncorrectTreeSize {
        shielded_protocol: PoolType::IRONWOOD,
        height: BlockHeight::from_u32(3_100_000),
        block_metadata_size: 999,
        calculated_size: 7,
    };
    assert!(
        refusal.to_string().contains("at height 3100000"),
        "{refusal}"
    );
}

/// Stands in for `tonic::transport::Error`, which cannot be built
/// outside tonic, wrapping the client-side timeout as its cause.
#[derive(Debug)]
struct TransportError(tonic::TimeoutExpired);

impl std::fmt::Display for TransportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "transport error")
    }
}

impl std::error::Error for TransportError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.0)
    }
}

/// A request cut off by the client-side timeout, as tonic reports it:
/// `Cancelled` with the transport timeout in the source chain.
fn client_timeout() -> ServerError {
    let status = tonic::Status::from_error(Box::new(TransportError(tonic::TimeoutExpired(()))));
    assert_eq!(status.code(), tonic::Code::Cancelled);
    ServerError::RequestFailed(status)
}

/// A connection the peer reset. tonic finds no code in the cause chain
/// and reports `Unknown`, keeping the I/O error as the source.
fn connection_reset() -> ServerError {
    let status = tonic::Status::from_error(Box::new(std::io::Error::from(
        std::io::ErrorKind::ConnectionReset,
    )));
    assert_eq!(status.code(), tonic::Code::Unknown);
    ServerError::RequestFailed(status)
}

/// The codes gRPC names as transient, as a server or the transport
/// reports them.
fn network_weather_statuses() -> [tonic::Status; 5] {
    [
        tonic::Status::unavailable("down"),
        tonic::Status::deadline_exceeded("timeout"),
        tonic::Status::cancelled("cancelled"),
        tonic::Status::resource_exhausted("overloaded"),
        tonic::Status::aborted("aborted"),
    ]
}

/// Codes a server answers with when it cannot serve the request.
fn server_refusal_statuses() -> [tonic::Status; 5] {
    [
        tonic::Status::unimplemented("no such method"),
        tonic::Status::invalid_argument("bad request"),
        tonic::Status::not_found("no such block"),
        tonic::Status::internal("indexer bug"),
        tonic::Status::unknown("unknown"),
    ]
}

mod recommend_same_server {
    use super::*;

    mod server_error {
        use super::*;

        #[test]
        fn fetcher_dropped() {
            assert!(ServerError::FetcherDropped.recommend_same_server());
        }

        #[test]
        fn chain_height_below_scan_range() {
            assert!(
                ServerError::ChainHeightBelowScanRange {
                    chain_height: BlockHeight::from_u32(99),
                    scan_range_end: BlockHeight::from_u32(100),
                }
                .recommend_same_server()
            );
        }

        #[test]
        fn request_network_weather() {
            for status in network_weather_statuses() {
                let code = status.code();
                let e = ServerError::RequestFailed(status);
                assert!(e.recommend_same_server(), "{code:?}");
            }
        }

        #[test]
        fn request_client_timeout() {
            assert!(client_timeout().recommend_same_server());
        }

        #[test]
        fn request_connection_reset() {
            assert!(connection_reset().recommend_same_server());
        }
    }

    mod sync_error {
        use super::*;

        #[test]
        fn mempool_error() {
            let e: TestSyncError = MempoolError::ShutdownWithoutStream.into();
            assert!(e.recommend_same_server());
        }

        /// The wallet has already reopened the pool, so the very next
        /// sync against this same server performs the rescan. Reporting
        /// this as a reason to change server would send wallets hunting
        /// for a server that was never at fault.
        #[test]
        fn pool_history_reopened() {
            let e: TestSyncError = SyncError::PoolHistoryReopened {
                pool: PoolType::IRONWOOD,
                rescan_from: BlockHeight::from_u32(100),
                disagreed_at: BlockHeight::from_u32(150),
                block_metadata_size: 999,
                calculated_size: 7,
            };
            assert!(e.recommend_same_server());
        }
    }
}

mod recommend_change_server {
    use super::*;

    mod server_error {
        use super::*;

        #[test]
        fn request_refused_by_server() {
            for status in server_refusal_statuses() {
                let code = status.code();
                let e = ServerError::RequestFailed(status);
                assert!(!e.recommend_same_server(), "{code:?}");
            }
        }

        #[test]
        fn invalid_frontier() {
            let e = ServerError::InvalidFrontier(std::io::Error::other("bad frontier"));
            assert!(!e.recommend_same_server());
        }

        #[test]
        fn invalid_transaction() {
            let e = ServerError::InvalidTransaction(std::io::Error::other("bad tx"));
            assert!(!e.recommend_same_server());
        }

        #[test]
        fn invalid_subtree_root() {
            assert!(!ServerError::InvalidSubtreeRoot.recommend_same_server());
        }

        #[test]
        fn chain_verification_error() {
            assert!(!ServerError::ChainVerificationError.recommend_same_server());
        }

        #[test]
        fn genesis_block_only() {
            assert!(!ServerError::GenesisBlockOnly.recommend_same_server());
        }
    }

    mod sync_error {
        use super::*;

        #[test]
        fn server_request_refused() {
            let e: TestSyncError =
                ServerError::RequestFailed(tonic::Status::unimplemented("no such method")).into();
            assert!(!e.recommend_same_server());
        }

        #[test]
        fn sync_mode_error() {
            let e: TestSyncError = SyncModeError::SyncAlreadyRunning.into();
            assert!(!e.recommend_same_server());
        }

        #[test]
        fn chain_error() {
            let e: TestSyncError = SyncError::ChainError(100, 50, 50);
            assert!(!e.recommend_same_server());
        }

        #[test]
        fn birthday_below_sapling() {
            let e: TestSyncError = SyncError::BirthdayBelowSapling(100, 419200);
            assert!(!e.recommend_same_server());
        }

        #[test]
        fn wallet_error() {
            let e: TestSyncError = SyncError::WalletError("db locked".to_string());
            assert!(!e.recommend_same_server());
        }
    }
}

mod recovery_recommendation {
    use super::*;

    mod retry_same_server {
        use super::*;

        #[test]
        fn fetcher_dropped() {
            assert_eq!(
                ServerError::FetcherDropped.recovery_recommendation(),
                SyncRecoveryObservables::MaybeRecoverableServer
            );
        }

        #[test]
        fn request_network_weather() {
            for status in network_weather_statuses() {
                let code = status.code();
                let e = ServerError::RequestFailed(status);
                assert_eq!(
                    e.recovery_recommendation(),
                    SyncRecoveryObservables::MaybeRecoverableServer,
                    "{code:?}"
                );
            }
        }

        #[test]
        fn sync_error_from_unavailable() {
            let e: TestSyncError =
                ServerError::RequestFailed(tonic::Status::unavailable("down")).into();
            assert_eq!(
                e.recovery_recommendation(),
                SyncRecoveryObservables::MaybeRecoverableServer
            );
        }

        /// A reset connection carries no transient code, only the I/O
        /// error behind it, and is still network weather.
        #[test]
        fn request_connection_reset() {
            assert_eq!(
                connection_reset().recovery_recommendation(),
                SyncRecoveryObservables::MaybeRecoverableServer
            );
        }

        /// A connection left idle until the client-side timeout expires
        /// is replaced by the next request, so the same server is kept.
        #[test]
        fn sync_error_from_client_timeout() {
            let e: TestSyncError = client_timeout().into();
            assert_eq!(
                e.recovery_recommendation(),
                SyncRecoveryObservables::MaybeRecoverableServer
            );
        }

        #[test]
        fn mempool_error() {
            let e: TestSyncError = MempoolError::ShutdownWithoutStream.into();
            assert_eq!(
                e.recovery_recommendation(),
                SyncRecoveryObservables::MaybeRecoverableServer
            );
        }

        #[test]
        fn chain_height_below_scan_range() {
            let e: TestSyncError = ServerError::ChainHeightBelowScanRange {
                chain_height: BlockHeight::from_u32(99),
                scan_range_end: BlockHeight::from_u32(100),
            }
            .into();
            assert_eq!(
                e.recovery_recommendation(),
                SyncRecoveryObservables::MaybeRecoverableServer
            );
        }

        /// Syncing again is the recovery, so this must never be reported
        /// as needing the user to intervene.
        #[test]
        fn pool_history_reopened() {
            let e: TestSyncError = SyncError::PoolHistoryReopened {
                pool: PoolType::IRONWOOD,
                rescan_from: BlockHeight::from_u32(100),
                disagreed_at: BlockHeight::from_u32(150),
                block_metadata_size: 999,
                calculated_size: 7,
            };
            assert_eq!(
                e.recovery_recommendation(),
                SyncRecoveryObservables::MaybeRecoverableServer
            );
        }
    }

    mod try_different_server {
        use super::*;

        #[test]
        fn request_refused_by_server() {
            for status in server_refusal_statuses() {
                let code = status.code();
                let e = ServerError::RequestFailed(status);
                assert_eq!(
                    e.recovery_recommendation(),
                    SyncRecoveryObservables::ServerUnavailable,
                    "{code:?}"
                );
            }
        }

        #[test]
        fn sync_error_from_request_refused() {
            let e: TestSyncError =
                ServerError::RequestFailed(tonic::Status::unimplemented("no such method")).into();
            assert_eq!(
                e.recovery_recommendation(),
                SyncRecoveryObservables::ServerUnavailable
            );
        }

        #[test]
        fn invalid_frontier() {
            let e = ServerError::InvalidFrontier(std::io::Error::other("bad"));
            assert_eq!(
                e.recovery_recommendation(),
                SyncRecoveryObservables::ServerUnavailable
            );
        }

        #[test]
        fn invalid_transaction() {
            let e = ServerError::InvalidTransaction(std::io::Error::other("bad"));
            assert_eq!(
                e.recovery_recommendation(),
                SyncRecoveryObservables::ServerUnavailable
            );
        }

        #[test]
        fn invalid_subtree_root() {
            assert_eq!(
                ServerError::InvalidSubtreeRoot.recovery_recommendation(),
                SyncRecoveryObservables::ServerUnavailable
            );
        }

        #[test]
        fn chain_verification_error() {
            assert_eq!(
                ServerError::ChainVerificationError.recovery_recommendation(),
                SyncRecoveryObservables::ServerUnavailable
            );
        }

        #[test]
        fn sync_error_from_invalid_frontier() {
            let e: TestSyncError =
                ServerError::InvalidFrontier(std::io::Error::other("bad")).into();
            assert_eq!(
                e.recovery_recommendation(),
                SyncRecoveryObservables::ServerUnavailable
            );
        }

        #[test]
        fn scan_error_wrapping_server_error() {
            let e: TestSyncError = ScanError::ServerError(ServerError::InvalidSubtreeRoot).into();
            assert_eq!(
                e.recovery_recommendation(),
                SyncRecoveryObservables::ServerUnavailable
            );
        }

        #[test]
        fn scan_error_transaction_not_served() {
            let e: TestSyncError = ScanError::TransactionNotServed {
                txid: TxId::from_bytes([1; 32]),
                block_height: BlockHeight::from_u32(100),
                status: tonic::Status::not_found("no such transaction"),
            }
            .into();
            assert_eq!(
                e.recovery_recommendation(),
                SyncRecoveryObservables::ServerUnavailable
            );
        }
    }

    mod abort {
        use super::*;

        /// A hash discontinuity at the first block of a scan range is
        /// handled within the sync session. One that ends the session is
        /// between blocks the server served together, or with a scanned
        /// block above the scan range, which syncing again repeats.
        #[test]
        fn continuity_error() {
            for continuity_error in [
                ContinuityError::HashDiscontinuity {
                    height: BlockHeight::from_u32(100),
                    prev_hash: BlockHash([1; 32]),
                    previous_block_hash: BlockHash([2; 32]),
                },
                ContinuityError::HeightDiscontinuity {
                    height: BlockHeight::from_u32(102),
                    previous_block_height: BlockHeight::from_u32(100),
                },
            ] {
                let e: TestSyncError = ScanError::ContinuityError(continuity_error).into();
                assert_eq!(e.recovery_recommendation(), SyncRecoveryObservables::Abort);
            }
        }

        #[test]
        fn genesis_block_only() {
            assert_eq!(
                ServerError::GenesisBlockOnly.recovery_recommendation(),
                SyncRecoveryObservables::Abort
            );
        }

        #[test]
        fn sync_mode_error() {
            let e: TestSyncError = SyncModeError::SyncAlreadyRunning.into();
            assert_eq!(e.recovery_recommendation(), SyncRecoveryObservables::Abort);
        }

        #[test]
        fn chain_error() {
            let e: TestSyncError = SyncError::ChainError(100, 50, 50);
            assert_eq!(e.recovery_recommendation(), SyncRecoveryObservables::Abort);
        }

        #[test]
        fn wallet_error() {
            let e: TestSyncError = SyncError::WalletError("db locked".to_string());
            assert_eq!(e.recovery_recommendation(), SyncRecoveryObservables::Abort);
        }
    }
}
