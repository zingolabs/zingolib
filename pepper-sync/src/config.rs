//! Sync configuration.

#[cfg(feature = "wallet_essentials")]
use std::io::{Read, Write};

#[cfg(feature = "wallet_essentials")]
use byteorder::{LittleEndian, ReadBytesExt, WriteBytesExt};

#[allow(missing_docs)]
pub const DEFAULT_MAX_NULLIFIER_MAP_SIZE: usize = 2_000_000;

#[allow(missing_docs)]
pub const LOW_MEMORY_MAX_NULLIFIER_MAP_SIZE: usize = 125_000;

/// Sync configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncConfig {
    /// Transparent address discovery configuration.
    pub transparent_address_discovery: TransparentAddressDiscovery,
    #[allow(missing_docs)]
    pub max_nullifier_map_size: usize,
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

impl Default for SyncConfig {
    fn default() -> Self {
        Self {
            transparent_address_discovery: TransparentAddressDiscovery::default(),
            max_nullifier_map_size: DEFAULT_MAX_NULLIFIER_MAP_SIZE,
            shutdown_on_completion: false,
        }
    }
}

#[cfg(feature = "wallet_essentials")]
impl SyncConfig {
    fn serialized_version() -> u8 {
        3
    }

    /// Deserialize into `reader`
    pub fn read<R: Read>(mut reader: R) -> std::io::Result<Self> {
        const RETIRED_SETTING_VERSION: u8 = 1;
        const SIZE_VERSION: u8 = 3;
        const RETIRED_SETTING_SIZES: [usize; 4] = [
            0,
            LOW_MEMORY_MAX_NULLIFIER_MAP_SIZE,
            DEFAULT_MAX_NULLIFIER_MAP_SIZE,
            usize::MAX,
        ];

        let version = crate::wallet::serialization::read_version(
            &mut reader,
            "SyncConfig",
            Self::serialized_version(),
        )?;

        let gap_limit = reader.read_u8()?;
        let scopes = reader.read_u8()?;
        let max_nullifier_map_size = if version >= SIZE_VERSION {
            usize::try_from(reader.read_u64::<LittleEndian>()?).unwrap_or(usize::MAX)
        } else if version >= RETIRED_SETTING_VERSION {
            let _retired_setting_version = reader.read_u8()?;
            let retired_setting = usize::from(reader.read_u8()?);
            RETIRED_SETTING_SIZES
                .get(retired_setting)
                .copied()
                .ok_or_else(|| {
                    std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        "failed to read valid maximum nullifier map size",
                    )
                })?
        } else {
            DEFAULT_MAX_NULLIFIER_MAP_SIZE
        };
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
            max_nullifier_map_size,
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
        writer.write_u64::<LittleEndian>(
            u64::try_from(self.max_nullifier_map_size).unwrap_or(u64::MAX),
        )?;
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

    const VERSION_TWO: u8 = 2;
    const GAP_LIMIT: u8 = 7;
    const EXTERNAL_AND_REFUND_SCOPES: u8 = 0b101;
    const RETIRED_SETTING_VERSION: u8 = 0;
    const SHUTDOWN_ON_COMPLETION: u8 = 1;

    fn version_two_config(retired_setting: u8) -> [u8; 6] {
        [
            VERSION_TWO,
            GAP_LIMIT,
            EXTERNAL_AND_REFUND_SCOPES,
            RETIRED_SETTING_VERSION,
            retired_setting,
            SHUTDOWN_ON_COMPLETION,
        ]
    }

    #[test]
    fn reader_keeps_the_nullifier_map_size_of_a_version_two_config() {
        for (retired_setting, max_nullifier_map_size) in [
            (0, 0),
            (1, LOW_MEMORY_MAX_NULLIFIER_MAP_SIZE),
            (2, DEFAULT_MAX_NULLIFIER_MAP_SIZE),
            (3, usize::MAX),
        ] {
            let serialized = version_two_config(retired_setting);
            let mut reader = serialized.as_slice();

            assert_eq!(
                SyncConfig::read(&mut reader).expect("a version two config reads"),
                SyncConfig {
                    transparent_address_discovery: TransparentAddressDiscovery {
                        gap_limit: GAP_LIMIT,
                        scopes: TransparentAddressDiscoveryScopes::default(),
                    },
                    max_nullifier_map_size,
                    shutdown_on_completion: true,
                },
                "retired setting {retired_setting}"
            );
            assert!(reader.is_empty(), "retired setting {retired_setting}");
        }
    }

    #[test]
    fn reader_refuses_an_unknown_retired_setting() {
        const UNKNOWN_RETIRED_SETTING: u8 = 4;

        assert_eq!(
            SyncConfig::read(version_two_config(UNKNOWN_RETIRED_SETTING).as_slice())
                .expect_err("an unknown retired setting is refused")
                .kind(),
            std::io::ErrorKind::InvalidData,
        );
    }

    #[test]
    fn written_config_reads_back() {
        for max_nullifier_map_size in [0, LOW_MEMORY_MAX_NULLIFIER_MAP_SIZE, usize::MAX] {
            let mut config = SyncConfig {
                transparent_address_discovery: TransparentAddressDiscovery::recovery(),
                max_nullifier_map_size,
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
}
