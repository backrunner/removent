//! Local loopback E2E: pairing → admission → negotiation → video/audio decode → resume token.

use removent_client::{ClientConfig, connect_session, quick_resume};
use removent_core::{
    AdmissionMode, DataPaths, DeviceIdentity, PeersStore, QualityPreset,
    adapt::AdaptationController, identity,
};
use removent_host::{
    HostConfig, HostInteractions, serve_connection, spawn_audio_loop, spawn_video_loop,
};
use removent_net::{PinState, RvpConnection, make_client_endpoint, make_server_endpoint};
use removent_proto::{Caps, CodecId};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};

const W: usize = 320;
const H: usize = 240;

fn identity_for(name: &str) -> (DeviceIdentity, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let p = DataPaths {
        root: dir.path().to_path_buf(),
    };
    (identity::load_or_create(&p, name).unwrap(), dir)
}

fn synthetic_bgra(t: usize) -> Vec<u8> {
    let mut buf = vec![0u8; W * H * 4];
    for y in 0..H {
        for x in 0..W {
            let i = (y * W + x) * 4;
            buf[i] = ((x + t * 13) % 256) as u8;
            buf[i + 1] = ((y + t * 5) % 256) as u8;
            buf[i + 2] = 77;
            buf[i + 3] = 255;
        }
    }
    buf
}

#[path = "session_loopback/admission.rs"]
mod admission;

#[path = "session_loopback/media.rs"]
mod media;

#[path = "session_loopback/resume.rs"]
mod resume;

#[path = "session_loopback/pairing.rs"]
mod pairing;

#[path = "session_loopback/recovery.rs"]
mod recovery;

#[path = "session_loopback/busy.rs"]
mod busy;

#[path = "session_loopback/audio.rs"]
mod audio;

#[path = "session_loopback/cancellation.rs"]
mod cancellation;

#[path = "session_loopback/authentication.rs"]
mod authentication;

#[path = "session_loopback/invitations.rs"]
mod invitations;

#[path = "session_loopback/security.rs"]
mod security;
