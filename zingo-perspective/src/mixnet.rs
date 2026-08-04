//! The mixnet projection: the one place Mixnet Mode is rendered for
//! consumers, re-derived from the typed status snapshot after the
//! five-state rework (ADR 0024) retired the old tri-state projection.
//!
//! zingolib owns the states, the minted wire tokens, and the shared
//! semantic sentences — the IP-correlation disclaimer above all; this
//! module owns only their composition into display forms. The JSON view
//! is a rendering of zingolib's serde wire (one mint, N renderers),
//! never a second hand-built shape.
#![forbid(unsafe_code)]

use zingolib::lightclient::LightClient;
use zingolib::nym::{IP_CORRELATION_DISCLAIMER, MixnetMode, MixnetStatus};

/// The status line for a Mixnet Mode snapshot: the mode, the live
/// bootstrap progress while bootstrapping, and the local SOCKS5 address
/// when ready. Pure, so the user-facing mode strings are pinned by the
/// tests below and every frontend renders the same line.
///
/// ```
/// use zingolib::nym::{MixnetMode, MixnetStatus};
///
/// let status = MixnetStatus {
///     mode: MixnetMode::Ready,
///     socks5_addr: Some("127.0.0.1:1080".into()),
///     bootstrap_detail: None,
///     death: None,
/// };
/// assert_eq!(
///     zingo_perspective::mixnet::status_line(&status),
///     "Mixnet Mode: ready (SOCKS5 127.0.0.1:1080)",
/// );
/// ```
#[must_use]
pub fn status_line(status: &MixnetStatus) -> String {
    match status.mode {
        MixnetMode::Unattached => "Mixnet Mode: unattached. The mixnet has not been enabled, \
             and no consent to clearnet has been given: send and price-fetch refuse. Run \
             `nym on` to enable the mixnet, or `nym off` to use clearnet."
            .to_string(),
        MixnetMode::SwitchedOff => {
            "Mixnet Mode: switched off (send and price-fetch use clearnet)".to_string()
        }
        MixnetMode::Bootstrapping => match status.bootstrap_detail.as_deref() {
            Some(detail) => format!(
                "Mixnet Mode: bootstrapping, {detail} (send and price-fetch are unavailable \
                 until ready)"
            ),
            None => "Mixnet Mode: bootstrapping (send and price-fetch are unavailable until ready)"
                .to_string(),
        },
        MixnetMode::Ready => match status.socks5_addr.as_deref() {
            Some(addr) => format!("Mixnet Mode: ready (SOCKS5 {addr})"),
            None => "Mixnet Mode: ready".to_string(),
        },
        MixnetMode::Died => "Mixnet Mode: died. The proxy exited unexpectedly. Send and \
             price-fetch refuse and will not fall back to clearnet. Run `nym on` to \
             restart the proxy."
            .to_string(),
    }
}

/// The complete status output: the Mixnet Mode line followed by the
/// IP-correlation disclaimer. The disclaimer always accompanies the
/// status (ZIP-0318), because Mixnet Mode obfuscates only send and
/// price-fetch while synchronization stays on the ordinary connector, so
/// a bare "ready" must never be read as end-to-end IP protection. The
/// canonical text lives in
/// [`zingolib::nym::IP_CORRELATION_DISCLAIMER`] so every frontend shows
/// the same wording.
#[must_use]
pub fn status_with_disclaimer(status: &MixnetStatus) -> String {
    format!("{}\n\n{}", status_line(status), IP_CORRELATION_DISCLAIMER,)
}

/// The JSON view of a snapshot: zingolib's golden-pinned serde wire,
/// rendered into the `json` crate's value so stringly consumers receive
/// the identical shape the neon boundary carries. One mint, N renderers
/// — this function never builds a second JSON vocabulary.
///
/// ```
/// use zingolib::nym::{MixnetMode, MixnetStatus};
///
/// let status = MixnetStatus {
///     mode: MixnetMode::Unattached,
///     socks5_addr: None,
///     bootstrap_detail: None,
///     death: None,
/// };
/// assert_eq!(
///     zingo_perspective::mixnet::status_json(&status).dump(),
///     r#"{"mode":"unattached"}"#,
/// );
/// ```
#[must_use]
pub fn status_json(status: &MixnetStatus) -> json::JsonValue {
    let wire = serde_json::to_string(status).expect("the wire mint serializes every snapshot");
    json::parse(&wire).expect("the wire mint emits valid JSON")
}

/// The snapshot pull over a [`LightClient`]: one borrow of the session's
/// status channel, so a frontend that renders on demand reads the same
/// typed snapshot a subscriber would have been pushed.
pub trait MixnetStatusSnapshotExt {
    /// The current [`MixnetStatus`], read whole.
    fn mixnet_status_snapshot(&self) -> MixnetStatus;
}

impl MixnetStatusSnapshotExt for LightClient {
    fn mixnet_status_snapshot(&self) -> MixnetStatus {
        self.subscribe_mixnet_status().borrow().clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn slot_only(mode: MixnetMode) -> MixnetStatus {
        MixnetStatus {
            mode,
            socks5_addr: None,
            bootstrap_detail: None,
            death: None,
        }
    }

    /// Pins the status mode strings. These literals relocated verbatim
    /// from zingo-cli's `render_status`, which itself pinned them when it
    /// replaced the pre-ADR-0024 hand-rolled strings.
    #[test]
    fn status_lines_render_byte_identically_to_the_replaced_strings() {
        assert_eq!(
            status_line(&slot_only(MixnetMode::Unattached)),
            "Mixnet Mode: unattached. The mixnet has not been enabled, and no consent to \
             clearnet has been given: send and price-fetch refuse. Run `nym on` to enable \
             the mixnet, or `nym off` to use clearnet.",
            "absence is not consent: unattached names refusal, never clearnet"
        );
        assert_eq!(
            status_line(&slot_only(MixnetMode::SwitchedOff)),
            "Mixnet Mode: switched off (send and price-fetch use clearnet)"
        );
        assert_eq!(
            status_line(&slot_only(MixnetMode::Bootstrapping)),
            "Mixnet Mode: bootstrapping (send and price-fetch are unavailable until ready)"
        );
        assert_eq!(
            status_line(&MixnetStatus {
                mode: MixnetMode::Ready,
                socks5_addr: Some("127.0.0.1:43210".into()),
                bootstrap_detail: None,
                death: None,
            }),
            "Mixnet Mode: ready (SOCKS5 127.0.0.1:43210)"
        );
        assert_eq!(
            status_line(&slot_only(MixnetMode::Ready)),
            "Mixnet Mode: ready",
            "ready with no address yet still renders (the route resolver, \
             not the renderer, refuses that state)"
        );
        assert_eq!(
            status_line(&slot_only(MixnetMode::Died)),
            "Mixnet Mode: died. The proxy exited unexpectedly. Send and price-fetch \
             refuse and will not fall back to clearnet. Run `nym on` to restart the proxy.",
            "a died proxy is reported distinctly from switched off, and tells the user how to \
             recover"
        );
    }

    /// HYPOTHESIS: live bootstrap progress reaches the status line, so
    /// the connect race is narrated rather than an opaque wait. Falsified
    /// if the detail is dropped by the renderer. The detail is shown only
    /// while bootstrapping: a ready proxy has no bootstrap left to
    /// narrate. (A ready snapshot carrying a stale detail is refusable on
    /// the wire but constructible in-process, so the renderer's own guard
    /// is still pinned.)
    #[test]
    fn bootstrap_detail_reaches_the_status_line_only_while_bootstrapping() {
        assert_eq!(
            status_line(&MixnetStatus {
                mode: MixnetMode::Bootstrapping,
                socks5_addr: None,
                bootstrap_detail: Some("attempt 2/10: 2 in flight, 0 failed".into()),
                death: None,
            }),
            "Mixnet Mode: bootstrapping, attempt 2/10: 2 in flight, 0 failed \
             (send and price-fetch are unavailable until ready)"
        );
        assert_eq!(
            status_line(&MixnetStatus {
                mode: MixnetMode::Ready,
                socks5_addr: Some("127.0.0.1:1".into()),
                bootstrap_detail: Some("stale".into()),
                death: None,
            }),
            "Mixnet Mode: ready (SOCKS5 127.0.0.1:1)",
            "a stale detail must not leak into the ready line"
        );
    }

    /// HYPOTHESIS: the status always carries the IP-correlation
    /// disclaimer in every mode, so a "ready" mixnet is never mistaken
    /// for end-to-end IP protection while synchronization stays on
    /// clearnet (ZIP-0318). The mode line is preserved verbatim as the
    /// first line. Falsified if the disclaimer is dropped in any mode, no
    /// longer leads with the mode line, or omits the sync/IP/indexer/
    /// balance risk it must name.
    #[test]
    fn status_always_carries_the_ip_correlation_disclaimer() {
        for mode in [
            MixnetMode::Unattached,
            MixnetMode::SwitchedOff,
            MixnetMode::Bootstrapping,
            MixnetMode::Ready,
            MixnetMode::Died,
        ] {
            let status = MixnetStatus {
                mode,
                socks5_addr: Some("127.0.0.1:43210".into()),
                bootstrap_detail: None,
                death: None,
            };
            let out = status_with_disclaimer(&status);
            assert!(
                out.starts_with(&status_line(&status)),
                "the mode line must lead the status output: {out}"
            );
            for phrase in [
                "IP-correlation risk",
                "synchronization",
                "sync indexer",
                "total balance",
                "ZIP-0318",
            ] {
                assert!(
                    out.contains(phrase),
                    "the disclaimer must name {phrase}: {out}"
                );
            }
        }
    }

    /// The JSON view is the wire mint rendered, byte for byte: dumping
    /// the parsed value reproduces zingolib's serde serialization, so no
    /// second JSON vocabulary can drift into existence here.
    #[test]
    fn the_json_view_is_the_wire_mint_rendered() {
        let status = MixnetStatus {
            mode: MixnetMode::Ready,
            socks5_addr: Some("127.0.0.1:1080".into()),
            bootstrap_detail: None,
            death: None,
        };
        assert_eq!(
            status_json(&status).dump(),
            serde_json::to_string(&status).unwrap(),
        );
    }
}
