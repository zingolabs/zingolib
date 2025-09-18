//! Local network configuration for testing
use zcash_protocol::consensus::{BlockHeight, Parameters};
use zingo_infra_services::network::ActivationHeights;

/// A struct representing a Local Network.
/// Used as a hack for compatiblility with `ActivationHeights` from `infra`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ZingolibLocalNetwork {
    inner: zcash_protocol::local_consensus::LocalNetwork,
}

impl Parameters for ZingolibLocalNetwork {
    fn network_type(&self) -> zcash_protocol::consensus::NetworkType {
        self.inner.network_type()
    }

    fn activation_height(
        &self,
        nu: zcash_protocol::consensus::NetworkUpgrade,
    ) -> Option<BlockHeight> {
        self.inner.activation_height(nu)
    }
}

impl Default for ZingolibLocalNetwork {
    fn default() -> Self {
        ZingolibLocalNetwork {
            inner: zcash_protocol::local_consensus::LocalNetwork {
                overwinter: Some(BlockHeight::from_u32(1)),
                sapling: Some(BlockHeight::from_u32(1)),
                blossom: Some(BlockHeight::from_u32(1)),
                heartwood: Some(BlockHeight::from_u32(1)),
                canopy: Some(BlockHeight::from_u32(1)),
                nu5: Some(BlockHeight::from_u32(1)),
                nu6: Some(BlockHeight::from_u32(1)),
                nu6_1: Some(BlockHeight::from_u32(1)),
            },
        }
    }
}

impl From<ActivationHeights> for ZingolibLocalNetwork {
    fn from(activation_heights: ActivationHeights) -> Self {
        ZingolibLocalNetwork {
            inner: zcash_protocol::local_consensus::LocalNetwork {
                overwinter: Some(
                    activation_heights
                        .activation_height(zcash_protocol::consensus::NetworkUpgrade::Overwinter)
                        .unwrap_or(1.into()),
                ),
                sapling: activation_heights
                    .activation_height(zcash_protocol::consensus::NetworkUpgrade::Sapling)
                    .unwrap_or(1.into())
                    .into(),
                blossom: activation_heights
                    .activation_height(zcash_protocol::consensus::NetworkUpgrade::Blossom)
                    .unwrap_or(1.into())
                    .into(),
                heartwood: activation_heights
                    .activation_height(zcash_protocol::consensus::NetworkUpgrade::Heartwood)
                    .unwrap_or(1.into())
                    .into(),
                canopy: activation_heights
                    .activation_height(zcash_protocol::consensus::NetworkUpgrade::Canopy)
                    .unwrap_or(1.into())
                    .into(),
                nu5: activation_heights
                    .activation_height(zcash_protocol::consensus::NetworkUpgrade::Nu5)
                    .unwrap_or(1.into())
                    .into(),
                nu6: activation_heights
                    .activation_height(zcash_protocol::consensus::NetworkUpgrade::Nu6)
                    .unwrap_or(1.into())
                    .into(),
                nu6_1: activation_heights
                    .activation_height(zcash_protocol::consensus::NetworkUpgrade::Nu6_1)
                    .unwrap_or(1.into())
                    .into(),
            },
        }
    }
}

impl From<ZingolibLocalNetwork> for ActivationHeights {
    fn from(zingolib_local_network: ZingolibLocalNetwork) -> Self {
        ActivationHeights::new(zingolib_local_network.inner)
    }
}
