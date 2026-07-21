//! Presentation of Mixnet Mode (ADR 0011): the one place the tri-state
//! is rendered for consumers, so the CLI and mobile UIs never invent
//! their own wording.

use json::JsonValue;

use zingolib::lightclient::LightClient;
use zingolib::nym::MixnetMode;

/// A consumer-facing snapshot of Mixnet Mode: the tri-state plus the
/// local SOCKS5 address once the mixnet is reachable.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MixnetStatusView {
    /// The tri-state (off / bootstrapping / ready).
    pub mode: MixnetMode,
    /// The spawned proxy's local SOCKS5 address, when ready.
    pub socks5_addr: Option<String>,
}

impl MixnetStatusView {
    /// The lowercase state name (`"off"`, `"bootstrapping"`, `"ready"`).
    #[must_use]
    pub fn mode_name(&self) -> &'static str {
        match self.mode {
            MixnetMode::Off => "off",
            MixnetMode::Bootstrapping => "bootstrapping",
            MixnetMode::Ready => "ready",
        }
    }
}

impl std::fmt::Display for MixnetStatusView {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.mode {
            MixnetMode::Off => {
                write!(f, "Mixnet Mode: off (send and price-fetch use clearnet)")
            }
            MixnetMode::Bootstrapping => write!(
                f,
                "Mixnet Mode: bootstrapping (send and price-fetch are unavailable until ready)"
            ),
            MixnetMode::Ready => match self.socks5_addr.as_ref() {
                Some(addr) => write!(f, "Mixnet Mode: ready (SOCKS5 {addr})"),
                None => write!(f, "Mixnet Mode: ready"),
            },
        }
    }
}

impl From<MixnetStatusView> for JsonValue {
    fn from(view: MixnetStatusView) -> Self {
        let mode_name = view.mode_name();
        json::object! {
            "mode" => mode_name,
            "socks5_addr" => view.socks5_addr,
        }
    }
}

/// The mixnet-status presentation over a [`LightClient`].
pub trait MixnetStatusViewExt {
    /// A snapshot of Mixnet Mode shaped for display.
    fn mixnet_status_view(&self) -> MixnetStatusView;
}

impl MixnetStatusViewExt for LightClient {
    fn mixnet_status_view(&self) -> MixnetStatusView {
        MixnetStatusView {
            mode: self.mixnet_mode(),
            socks5_addr: self.mixnet_socks5_addr(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The Display wording is the CLI's `nym status` output: these
    /// literals moved here from zingo-cli's nym_command verbatim, and
    /// the CLI now renders the view, so this test pins byte-identity
    /// with the pre-extraction strings.
    #[test]
    fn display_matches_the_pre_extraction_cli_wording() {
        assert_eq!(
            MixnetStatusView {
                mode: MixnetMode::Off,
                socks5_addr: None,
            }
            .to_string(),
            "Mixnet Mode: off (send and price-fetch use clearnet)"
        );
        assert_eq!(
            MixnetStatusView {
                mode: MixnetMode::Bootstrapping,
                socks5_addr: None,
            }
            .to_string(),
            "Mixnet Mode: bootstrapping (send and price-fetch are unavailable until ready)"
        );
        assert_eq!(
            MixnetStatusView {
                mode: MixnetMode::Ready,
                socks5_addr: Some("127.0.0.1:1080".to_string()),
            }
            .to_string(),
            "Mixnet Mode: ready (SOCKS5 127.0.0.1:1080)"
        );
        assert_eq!(
            MixnetStatusView {
                mode: MixnetMode::Ready,
                socks5_addr: None,
            }
            .to_string(),
            "Mixnet Mode: ready"
        );
    }

    #[test]
    fn json_carries_mode_and_address() {
        let ready = JsonValue::from(MixnetStatusView {
            mode: MixnetMode::Ready,
            socks5_addr: Some("127.0.0.1:1080".to_string()),
        });
        assert_eq!(ready["mode"], "ready");
        assert_eq!(ready["socks5_addr"], "127.0.0.1:1080");

        let off = JsonValue::from(MixnetStatusView {
            mode: MixnetMode::Off,
            socks5_addr: None,
        });
        assert_eq!(off["mode"], "off");
        assert!(off["socks5_addr"].is_null());
    }
}
