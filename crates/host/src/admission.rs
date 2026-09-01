//! Host-side admission decisions (FR-06) and trusted-device capability checks.

use removent_core::{AdmissionMode, PeersStore};
use removent_proto::Caps;

#[derive(Debug, PartialEq)]
pub enum AdmissionDecision {
    Allow,
    /// Requires a user confirmation prompt.
    Ask,
    Deny(&'static str),
}

/// Pure decision function: `peer_fp_hex` is the hex of the peer's full certificate fingerprint.
pub fn decide(
    mode: AdmissionMode,
    peers: &PeersStore,
    peer_fp_hex: &str,
    requested: Caps,
) -> AdmissionDecision {
    match mode {
        AdmissionMode::DenyAll => AdmissionDecision::Deny("host disabled incoming"),
        AdmissionMode::AlwaysAsk => {
            if let Some(p) = peers.by_fingerprint(peer_fp_hex)
                && p.trusted
                && caps_covered(p.granted_caps, requested)
            {
                return AdmissionDecision::Allow;
            }
            AdmissionDecision::Ask
        }
        AdmissionMode::TrustedAuto => {
            match peers.by_fingerprint(peer_fp_hex) {
                Some(p) if p.trusted => {
                    if caps_covered(p.granted_caps, requested) {
                        AdmissionDecision::Allow
                    } else {
                        // Trusted but requesting ungranted capabilities: still ask to expand grants.
                        AdmissionDecision::Ask
                    }
                }
                _ => AdmissionDecision::Ask,
            }
        }
    }
}

fn caps_covered(granted: Caps, requested: Caps) -> bool {
    let covers = |g: bool, r: bool| !r || g;
    covers(granted.video, requested.video)
        && covers(granted.audio, requested.audio)
        && covers(granted.input, requested.input)
        && covers(granted.clipboard, requested.clipboard)
        && covers(granted.file, requested.file)
}

#[cfg(test)]
mod tests {
    use super::*;
    use removent_core::PeerRecord;

    fn store_with(rec: Option<PeerRecord>) -> PeersStore {
        let mut s = PeersStore::in_memory();
        if let Some(r) = rec {
            s.upsert(r).unwrap();
        }
        s
    }

    const FP_A: &str = "aaaa"; // registered
    const FP_X: &str = "xxxx"; // unknown

    fn record(trusted: bool, caps: Caps) -> PeerRecord {
        PeerRecord {
            fingerprint: FP_A.into(),
            name: "A".into(),
            short_fp: "aaaa".into(),
            granted_caps: caps,
            trusted,
            added_at_unix: 0,
            last_connected_unix: 0,
        }
    }

    fn video_only() -> Caps {
        Caps {
            video: true,
            ..Caps::none()
        }
    }
    fn all() -> Caps {
        Caps::all()
    }

    #[test]
    fn deny_all_denies_everything() {
        let d = decide(AdmissionMode::DenyAll, &store_with(None), FP_A, all());
        assert!(matches!(d, AdmissionDecision::Deny(_)));
    }

    #[test]
    fn always_ask_allows_trusted_within_grants() {
        let store = store_with(Some(record(true, video_only())));
        assert_eq!(
            decide(AdmissionMode::AlwaysAsk, &store, FP_A, video_only()),
            AdmissionDecision::Allow
        );
        // beyond granted scope → ask
        assert_eq!(
            decide(AdmissionMode::AlwaysAsk, &store, FP_A, all()),
            AdmissionDecision::Ask
        );
    }

    #[test]
    fn always_ask_asks_for_unknown() {
        let store = store_with(None);
        assert_eq!(
            decide(AdmissionMode::AlwaysAsk, &store, FP_X, video_only()),
            AdmissionDecision::Ask
        );
    }

    #[test]
    fn untrusted_identity_still_asks() {
        let store = store_with(Some(record(false, all())));
        assert_eq!(
            decide(AdmissionMode::TrustedAuto, &store, FP_A, all()),
            AdmissionDecision::Ask
        );
    }

    #[test]
    fn trusted_auto_expansion_requires_confirm() {
        let store = store_with(Some(record(true, video_only())));
        assert_eq!(
            decide(AdmissionMode::TrustedAuto, &store, FP_A, all()),
            AdmissionDecision::Ask
        );
        assert_eq!(
            decide(AdmissionMode::TrustedAuto, &store, FP_A, video_only()),
            AdmissionDecision::Allow
        );
    }
}
