//! The chain the sync engine syncs against.

use zcash_protocol::{
    consensus::{self, BlockHeight, NetworkType, NetworkUpgrade},
    local_consensus::LocalNetwork,
};

/// The consensus parameters the sync engine owns.
///
/// Sync decisions are chain-dependent: which pools exist, and from which
/// height each one starts. Owning the parameters lets the state machine
/// answer those questions about a wallet it already holds, instead of
/// requiring every caller to supply the chain alongside it. The heights
/// live in a [`LocalNetwork`] because its field set is the canonical one,
/// so a new network upgrade fails to compile here rather than silently
/// resolving as never activated.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ChainParameters {
    network_type: NetworkType,
    activations: LocalNetwork,
}

impl ChainParameters {
    /// Resolves any parameter set into the owned form, without loss: the
    /// network type and every activation height are copied out.
    #[must_use]
    pub fn of(parameters: &impl consensus::Parameters) -> Self {
        ChainParameters {
            network_type: parameters.network_type(),
            activations: LocalNetwork {
                overwinter: parameters.activation_height(NetworkUpgrade::Overwinter),
                sapling: parameters.activation_height(NetworkUpgrade::Sapling),
                blossom: parameters.activation_height(NetworkUpgrade::Blossom),
                heartwood: parameters.activation_height(NetworkUpgrade::Heartwood),
                canopy: parameters.activation_height(NetworkUpgrade::Canopy),
                nu5: parameters.activation_height(NetworkUpgrade::Nu5),
                nu6: parameters.activation_height(NetworkUpgrade::Nu6),
                nu6_1: parameters.activation_height(NetworkUpgrade::Nu6_1),
                nu6_2: parameters.activation_height(NetworkUpgrade::Nu6_2),
                nu6_3: parameters.activation_height(NetworkUpgrade::Nu6_3),
            },
        }
    }
}

impl consensus::Parameters for ChainParameters {
    fn network_type(&self) -> NetworkType {
        self.network_type
    }

    fn activation_height(&self, nu: NetworkUpgrade) -> Option<BlockHeight> {
        self.activations.activation_height(nu)
    }
}

#[cfg(test)]
mod tests {
    use zcash_protocol::consensus::{MAIN_NETWORK, Parameters};

    use super::*;

    /// Resolving mainnet and reading the result back must agree with the
    /// source on every question the trait can be asked, so that holding the
    /// parameters is never weaker than being handed them.
    #[test]
    fn resolving_preserves_network_type_and_activations() {
        let owned = ChainParameters::of(&MAIN_NETWORK);

        assert_eq!(owned.network_type(), MAIN_NETWORK.network_type());
        for upgrade in [
            NetworkUpgrade::Overwinter,
            NetworkUpgrade::Sapling,
            NetworkUpgrade::Blossom,
            NetworkUpgrade::Heartwood,
            NetworkUpgrade::Canopy,
            NetworkUpgrade::Nu5,
            NetworkUpgrade::Nu6,
            NetworkUpgrade::Nu6_1,
            NetworkUpgrade::Nu6_2,
            NetworkUpgrade::Nu6_3,
        ] {
            assert_eq!(
                owned.activation_height(upgrade),
                MAIN_NETWORK.activation_height(upgrade),
                "{upgrade} activation height was not preserved"
            );
        }
    }

    /// A regtest parameter set keeps its own network type rather than
    /// collapsing to a built-in network's.
    #[test]
    fn resolving_preserves_a_regtest_network() {
        let regtest = LocalNetwork {
            overwinter: Some(BlockHeight::from_u32(1)),
            sapling: Some(BlockHeight::from_u32(1)),
            blossom: Some(BlockHeight::from_u32(1)),
            heartwood: Some(BlockHeight::from_u32(1)),
            canopy: Some(BlockHeight::from_u32(1)),
            nu5: Some(BlockHeight::from_u32(1)),
            nu6: Some(BlockHeight::from_u32(1)),
            nu6_1: Some(BlockHeight::from_u32(1)),
            nu6_2: Some(BlockHeight::from_u32(1)),
            nu6_3: Some(BlockHeight::from_u32(100)),
        };

        let owned = ChainParameters::of(&regtest);

        assert_eq!(owned.network_type(), NetworkType::Regtest);
        assert_eq!(
            owned.activation_height(NetworkUpgrade::Nu6_3),
            Some(BlockHeight::from_u32(100))
        );
    }
}
