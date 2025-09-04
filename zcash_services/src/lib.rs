#![warn(missing_docs)]
//! # Overview
//!
//! Utilities that launch and manage Zcash processes. This is used for integration
//! testing in the development of:
//!
//!   - lightclients
//!   - indexers
//!   - validators
//!
//!
//! # List of Managed Processes
//! - Zebrad
//! - Zcashd
//! - Zainod
//! - Lightwalletd
//!
//! # Prerequisites
//!
//! An internet connection will be needed (during the fist build at least) in order to fetch the required testing binaries.
//! The binaries will be automagically checked and downloaded on `cargo build/check/test`. If you specify `None` in a process `launch` config, these binaries will be used.
//! The path to the binaries can be specified when launching a process. In that case, you are responsible for compiling the needed binaries.
//! Each processes `launch` fn and [`crate::LocalNet::launch`] take config structs for defining parameters such as path
//! locations.
//! See the config structs for each process in validator.rs and indexer.rs for more details.
//!
//! ## Launching multiple processes
//!
//! See [`crate::LocalNet`].
//!
//! # Testing
//!
//! See [`crate::test_fixtures`] doc comments for running client rpc tests from external crates for indexer/validator development.
//!
//! The `test_fixtures` feature is enabled by default to allow tests to run.
//!

pub mod config;
pub mod error;
pub mod indexer;
pub mod network;
pub mod utils;
pub mod validator;

mod launch;
mod logs;

use indexer::{
    Empty, EmptyConfig, Indexer, Lightwalletd, LightwalletdConfig, Zainod, ZainodConfig,
};
use validator::{Validator, Zcashd, ZcashdConfig, Zebrad, ZebradConfig};

/// All processes currently supported
#[derive(Clone, Copy)]
enum Process {
    Zcashd,
    Zebrad,
    Zainod,
    Lightwalletd,
}

impl std::fmt::Display for Process {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let process = match self {
            Self::Zcashd => "zcashd",
            Self::Zebrad => "zebrad",
            Self::Zainod => "zainod",
            Self::Lightwalletd => "lightwalletd",
        };
        write!(f, "{}", process)
    }
}

/// This struct is used to represent and manage the local network.
///
/// May be used to launch an indexer and validator together. This simplifies launching a Zcash test environment and
/// managing multiple processes as well as allowing generic test framework of processes that implement the
/// [`crate::validator::Validator`] or [`crate::indexer::Indexer`] trait.
pub struct LocalNet<I, V>
where
    I: Indexer,
    V: Validator,
{
    indexer: I,
    validator: V,
}

impl<I, V> LocalNet<I, V>
where
    I: Indexer,
    V: Validator,
{
    /// Gets indexer.
    pub fn indexer(&self) -> &I {
        &self.indexer
    }

    /// Gets indexer as mut.
    pub fn indexer_mut(&mut self) -> &mut I {
        &mut self.indexer
    }

    /// Gets validator.
    pub fn validator(&self) -> &V {
        &self.validator
    }

    /// Gets validator as mut.
    pub fn validator_mut(&mut self) -> &mut V {
        &mut self.validator
    }
}

impl LocalNet<Zainod, Zcashd> {
    /// Launch LocalNet.
    ///
    /// The `validator_port` field of [`crate::indexer::ZainodConfig`] will be overwritten to match the validator's RPC port.
    pub async fn launch(mut indexer_config: ZainodConfig, validator_config: ZcashdConfig) -> Self {
        let validator = Zcashd::launch(validator_config).await.unwrap();
        indexer_config.validator_port = validator.port();
        let indexer = Zainod::launch(indexer_config).unwrap();

        LocalNet { indexer, validator }
    }
}

impl LocalNet<Zainod, Zebrad> {
    /// Launch LocalNet.
    ///
    /// The `validator_port` field of [`crate::indexer::ZainodConfig`] will be overwritten to match the validator's RPC port.
    pub async fn launch(mut indexer_config: ZainodConfig, validator_config: ZebradConfig) -> Self {
        let validator = Zebrad::launch(validator_config).await.unwrap();
        indexer_config.validator_port = validator.rpc_listen_port();
        let indexer = Zainod::launch(indexer_config).unwrap();

        LocalNet { indexer, validator }
    }
}

impl LocalNet<Lightwalletd, Zcashd> {
    /// Launch LocalNet.
    ///
    /// The `validator_conf` field of [`crate::indexer::LightwalletdConfig`] will be overwritten to match the validator's config path.
    pub async fn launch(
        mut indexer_config: LightwalletdConfig,
        validator_config: ZcashdConfig,
    ) -> Self {
        let validator = Zcashd::launch(validator_config).await.unwrap();
        indexer_config.zcashd_conf = validator.config_path();
        let indexer = Lightwalletd::launch(indexer_config).unwrap();

        LocalNet { indexer, validator }
    }
}

impl LocalNet<Lightwalletd, Zebrad> {
    /// Launch LocalNet.
    ///
    /// The `validator_conf` field of [`crate::indexer::LightwalletdConfig`] will be overwritten to match the validator's config path.
    pub async fn launch(
        mut indexer_config: LightwalletdConfig,
        validator_config: ZebradConfig,
    ) -> Self {
        let validator = Zebrad::launch(validator_config).await.unwrap();
        indexer_config.zcashd_conf = validator.config_dir().path().join(config::ZCASHD_FILENAME);
        let indexer = Lightwalletd::launch(indexer_config).unwrap();

        LocalNet { indexer, validator }
    }
}

impl LocalNet<Empty, Zcashd> {
    /// Launch LocalNet.
    pub async fn launch(indexer_config: EmptyConfig, validator_config: ZcashdConfig) -> Self {
        let validator = Zcashd::launch(validator_config).await.unwrap();
        let indexer = Empty::launch(indexer_config).unwrap();

        LocalNet { indexer, validator }
    }
}

impl LocalNet<Empty, Zebrad> {
    /// Launch LocalNet.
    pub async fn launch(indexer_config: EmptyConfig, validator_config: ZebradConfig) -> Self {
        let validator = Zebrad::launch(validator_config).await.unwrap();
        let indexer = Empty::launch(indexer_config).unwrap();

        LocalNet { indexer, validator }
    }
}
