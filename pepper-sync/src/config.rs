//! Sync configuration.

#[cfg(feature = "wallet_essentials")]
use std::io::{Read, Write};

#[cfg(feature = "wallet_essentials")]
use byteorder::{ReadBytesExt, WriteBytesExt};

/// Sync configuration.
#[derive(Default, Debug, Clone, PartialEq, Eq)]
pub struct SyncConfig {
    /// Transparent address discovery configuration.
    pub transparent_address_discovery: TransparentAddressDiscovery,
    /// Shutdown on completion
    ///
    /// If not set, sync will not shutdown until the consumer sets the `SyncMode` to `Shutdown` variant.
    /// The sync engine will regularly check for new blocks mined so the wallet will always be updated to the state
    /// of the latest chain.
    ///
    /// If set, sync will still check for any newly mined blocks during scanning. But when the wallet is completely
    /// up-to-date with the latest chain, a running sync will shutdown. A sync the consumer has paused stays paused,
    /// and shuts down once it is resumed and completes again.
    pub shutdown_on_completion: bool,
}

#[cfg(feature = "wallet_essentials")]
impl SyncConfig {
    fn serialized_version() -> u8 {
        3
    }

    /// Deserialize into `reader`
    pub fn read<R: Read>(mut reader: R) -> std::io::Result<Self> {
        const RETIRED_FIELD_VERSIONS: std::ops::RangeInclusive<u8> = 1..=2;
        const RETIRED_FIELD_LEN: usize = 2;

        let version = crate::wallet::serialization::read_version(
            &mut reader,
            "SyncConfig",
            Self::serialized_version(),
        )?;

        let gap_limit = reader.read_u8()?;
        let scopes = reader.read_u8()?;
        if RETIRED_FIELD_VERSIONS.contains(&version) {
            reader.read_exact(&mut [0; RETIRED_FIELD_LEN])?;
        }
        let shutdown_on_completion = if version >= 2 {
            reader.read_u8()? != 0
        } else {
            false
        };

        Ok(Self {
            transparent_address_discovery: TransparentAddressDiscovery {
                gap_limit,
                scopes: TransparentAddressDiscoveryScopes {
                    external: scopes & 0b1 != 0,
                    internal: scopes & 0b10 != 0,
                    refund: scopes & 0b100 != 0,
                },
            },
            shutdown_on_completion,
        })
    }

    /// Serialize into `writer`
    pub fn write<W: Write>(&mut self, mut writer: W) -> std::io::Result<()> {
        writer.write_u8(Self::serialized_version())?;
        writer.write_u8(self.transparent_address_discovery.gap_limit)?;
        let mut scopes = 0;
        if self.transparent_address_discovery.scopes.external {
            scopes |= 0b1;
        }
        if self.transparent_address_discovery.scopes.internal {
            scopes |= 0b10;
        }
        if self.transparent_address_discovery.scopes.refund {
            scopes |= 0b100;
        }
        writer.write_u8(scopes)?;
        writer.write_u8(self.shutdown_on_completion as u8)?;

        Ok(())
    }
}

/// Transparent address configuration.
///
/// Sets which `scopes` will be searched for addresses in use, scanning relevant transactions, up to a given `gap_limit`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransparentAddressDiscovery {
    /// Sets the gap limit for transparent address discovery.
    pub gap_limit: u8,
    /// Sets the scopes for transparent address discovery.
    pub scopes: TransparentAddressDiscoveryScopes,
}

impl Default for TransparentAddressDiscovery {
    fn default() -> Self {
        Self {
            gap_limit: 10,
            scopes: TransparentAddressDiscoveryScopes::default(),
        }
    }
}

impl TransparentAddressDiscovery {
    /// Constructs a transparent address discovery config with a gap limit of 1 and ignoring the internal scope.
    #[must_use]
    pub fn minimal() -> Self {
        Self {
            gap_limit: 1,
            scopes: TransparentAddressDiscoveryScopes::default(),
        }
    }

    /// Constructs a transparent address discovery config with a gap limit of 20 for all scopes.
    #[must_use]
    pub fn recovery() -> Self {
        Self {
            gap_limit: 20,
            scopes: TransparentAddressDiscoveryScopes::recovery(),
        }
    }

    /// Disables transparent address discovery. Sync will only scan transparent outputs for addresses already in the
    /// wallet in transactions that also contain shielded inputs or outputs relevant to the wallet.
    #[must_use]
    pub fn disabled() -> Self {
        Self {
            gap_limit: 0,
            scopes: TransparentAddressDiscoveryScopes {
                external: false,
                internal: false,
                refund: false,
            },
        }
    }
}

/// Sets the active scopes for transparent address recovery.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransparentAddressDiscoveryScopes {
    /// External.
    pub external: bool,
    /// Internal.
    pub internal: bool,
    /// Refund.
    pub refund: bool,
}

impl Default for TransparentAddressDiscoveryScopes {
    fn default() -> Self {
        Self {
            external: true,
            internal: false,
            refund: true,
        }
    }
}

impl TransparentAddressDiscoveryScopes {
    /// Constructor with all all scopes active.
    #[must_use]
    pub fn recovery() -> Self {
        Self {
            external: true,
            internal: true,
            refund: true,
        }
    }
}

#[cfg(all(test, feature = "wallet_essentials"))]
mod tests {
    use super::*;

    /// The reader is given only a serialized version above the one `SyncConfig` writes. A reader that refuses the
    /// version returns invalid data. A reader that read on would report the end of the input.
    #[test]
    fn reader_refuses_a_serialized_version_above_its_own() {
        let newer_sync_config = [SyncConfig::serialized_version() + 1];

        assert_eq!(
            SyncConfig::read(newer_sync_config.as_slice())
                .expect_err("SyncConfig")
                .kind(),
            std::io::ErrorKind::InvalidData,
        );
    }

    #[test]
    fn reader_passes_the_retired_field_of_a_version_two_config() {
        const VERSION: u8 = 2;
        const GAP_LIMIT: u8 = 7;
        const EXTERNAL_AND_REFUND_SCOPES: u8 = 0b101;
        const RETIRED_FIELD: [u8; 2] = [0, 1];
        const SHUTDOWN_ON_COMPLETION: u8 = 1;

        let [retired_version, retired_setting] = RETIRED_FIELD;
        let version_two_config = [
            VERSION,
            GAP_LIMIT,
            EXTERNAL_AND_REFUND_SCOPES,
            retired_version,
            retired_setting,
            SHUTDOWN_ON_COMPLETION,
        ];
        let mut reader = version_two_config.as_slice();

        assert_eq!(
            SyncConfig::read(&mut reader).expect("a version two config reads"),
            SyncConfig {
                transparent_address_discovery: TransparentAddressDiscovery {
                    gap_limit: GAP_LIMIT,
                    scopes: TransparentAddressDiscoveryScopes::default(),
                },
                shutdown_on_completion: true,
            }
        );
        assert!(reader.is_empty());
    }

    #[test]
    fn written_config_reads_back() {
        let mut config = SyncConfig {
            transparent_address_discovery: TransparentAddressDiscovery::recovery(),
            shutdown_on_completion: true,
        };
        let mut serialized = Vec::new();
        config.write(&mut serialized).expect("a config writes");

        assert_eq!(
            SyncConfig::read(serialized.as_slice()).expect("a written config reads"),
            config
        );
    }
}
