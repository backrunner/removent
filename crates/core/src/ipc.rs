//! IPC protocol between the daemon and management clients (tray / app / cli).
//!
//! Transport: Unix domain socket (`DataPaths::daemon_socket()`), JSON Lines encoding.
//! Requests and responses are paired in order within each connection; once connected,
//! the daemon keeps pushing events (event stream subscription).

use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncWriteExt};
use tokio::net::UnixStream;

/// Protocol version (bump on incompatible changes).
pub const IPC_VERSION: u32 = 1;

/// Management client → daemon requests.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum IpcRequest {
    /// Query the full status snapshot.
    Status,
    /// Enable/disable the host service (the daemon process stays resident).
    SetEnabled { on: bool },
    /// Reload settings from disk (sent after the app settings page saves; takes effect on the runner's next restart).
    ReloadSettings,
    /// Admission decision reply (corresponds to the AdmissionRequest event).
    AdmissionReply { request_id: u64, allow: bool },
    /// Disconnect the given session.
    KickSession { session_id: u64 },
    /// Stop the daemon process.
    Shutdown,
}

/// Session summary (shared by status snapshots and events).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionInfo {
    pub id: u64,
    pub peer_name: String,
    pub peer_fp16: String,
    pub since_unix: u64,
    pub video_codec: String,
}

/// Status snapshot (Status response + tray polling).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct StatusReport {
    /// Host service (listener + broadcaster) is enabled.
    pub running: bool,
    pub port: u16,
    pub device_name: String,
    pub fp_short: String,
    pub sessions: Vec<SessionInfo>,
    /// In-progress pairing PIN (if any).
    pub pending_pin: Option<String>,
    /// Whether a management client (tray/app) is online.
    pub tray_connected: bool,
    /// Screen Recording TCC permission held by the daemon process (preflighted
    /// at snapshot time; capture silently produces nothing without it).
    pub screen_recording_granted: bool,
    /// Accessibility TCC permission held by the daemon process (preflighted at
    /// snapshot time; input injection fails without it).
    pub accessibility_granted: bool,
}

/// daemon → management client responses (paired one-to-one with requests, in order).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum IpcResponse {
    Status(Box<StatusReport>),
    Ok,
    Error { message: String },
}

/// daemon → management client events (no request needed; broadcast to all connections).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum IpcEvent {
    StateChanged {
        running: bool,
    },
    SessionStarted {
        session: SessionInfo,
    },
    SessionEnded {
        session_id: u64,
        reason: String,
    },
    /// Admission request; auto-denied after 30s without an AdmissionReply (protocol §4.4).
    AdmissionRequest {
        request_id: u64,
        peer_name: String,
        peer_fp16: String,
    },
    AdmissionResolved {
        request_id: u64,
        allow: bool,
    },
    /// Pairing PIN display (valid for 5 minutes).
    PairingPin {
        pin: String,
    },
    PairingDone {
        peer_name: String,
    },
}

/// Write one JSON Lines message.
pub async fn write_msg<W, T>(w: &mut W, value: &T) -> std::io::Result<()>
where
    W: AsyncWriteExt + Unpin,
    T: Serialize,
{
    let mut line = serde_json::to_vec(value)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    line.push(b'\n');
    w.write_all(&line).await?;
    w.flush().await
}

/// Maximum length of one JSON Lines frame; longer lines are rejected instead of
/// being buffered without bound.
const MAX_IPC_LINE_BYTES: usize = 1 << 20;

/// Read one JSON Lines message; returns Ok(None) on EOF.
///
/// Blank lines are skipped (a stray `\n` is not a disconnect); lines longer than
/// [`MAX_IPC_LINE_BYTES`] are rejected with an error.
pub async fn read_msg<R, T>(r: &mut R) -> std::io::Result<Option<T>>
where
    R: AsyncBufRead + Unpin,
    T: for<'de> Deserialize<'de>,
{
    loop {
        let mut line = String::new();
        let n = r.read_line(&mut line).await?;
        if n == 0 {
            return Ok(None);
        }
        if line.len() > MAX_IPC_LINE_BYTES {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "ipc line too long",
            ));
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let value = serde_json::from_str(trimmed)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        return Ok(Some(value));
    }
}

/// Connect to the daemon's UDS (shared entry point for management clients).
pub async fn connect(
    paths: &crate::paths::DataPaths,
) -> std::io::Result<(
    tokio::io::BufReader<tokio::net::unix::OwnedReadHalf>,
    tokio::net::unix::OwnedWriteHalf,
)> {
    let stream = UnixStream::connect(paths.daemon_socket()).await?;
    let (r, w) = stream.into_split();
    Ok((tokio::io::BufReader::new(r), w))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn json_lines_roundtrip() {
        let (a, b) = UnixStream::pair().unwrap();
        let (ar, mut aw) = a.into_split();
        let (br, mut bw) = b.into_split();
        let mut ar = tokio::io::BufReader::new(ar);
        let mut br = tokio::io::BufReader::new(br);

        write_msg(&mut aw, &IpcRequest::SetEnabled { on: true })
            .await
            .unwrap();
        let got: IpcRequest = read_msg(&mut br).await.unwrap().unwrap();
        assert!(matches!(got, IpcRequest::SetEnabled { on: true }));

        let ev = IpcEvent::AdmissionRequest {
            request_id: 7,
            peer_name: "peer".into(),
            peer_fp16: "0123456789abcdef".into(),
        };
        write_msg(&mut bw, &ev).await.unwrap();
        let got: IpcEvent = read_msg(&mut ar).await.unwrap().unwrap();
        assert!(matches!(
            got,
            IpcEvent::AdmissionRequest { request_id: 7, .. }
        ));

        // StatusReport serialization snapshot (cross-checked against the Swift Codable side).
        let report = StatusReport {
            running: true,
            port: 48688,
            device_name: "Mac".into(),
            fp_short: "a1b2c3d4".into(),
            sessions: vec![SessionInfo {
                id: 1,
                peer_name: "peer".into(),
                peer_fp16: "0123456789abcdef".into(),
                since_unix: 1_700_000_000,
                video_codec: "Hevc".into(),
            }],
            pending_pin: Some("123456".into()),
            tray_connected: true,
            screen_recording_granted: true,
            accessibility_granted: false,
        };
        let json = serde_json::to_string(&report).unwrap();
        assert!(json.contains("\"running\":true"));
        assert!(json.contains("\"pending_pin\":\"123456\""));
        assert!(json.contains("\"screen_recording_granted\":true"));
    }

    #[tokio::test]
    async fn blank_lines_are_skipped_not_eof() {
        let (a, b) = UnixStream::pair().unwrap();
        let (ar, _aw) = a.into_split();
        let mut ar = tokio::io::BufReader::new(ar);
        let (_br, mut bw) = b.into_split();

        bw.write_all(b"\n  \n").await.unwrap();
        write_msg(&mut bw, &IpcResponse::Ok).await.unwrap();
        let got: IpcResponse = read_msg(&mut ar).await.unwrap().unwrap();
        assert!(matches!(got, IpcResponse::Ok));
    }

    #[tokio::test]
    async fn oversized_line_is_rejected() {
        let (a, b) = UnixStream::pair().unwrap();
        let (ar, _aw) = a.into_split();
        let mut ar = tokio::io::BufReader::new(ar);
        let (_br, mut bw) = b.into_split();

        // The payload far exceeds the socket buffer, so the write must be driven
        // concurrently with the read (writing first would deadlock).
        let writer = tokio::spawn(async move {
            let big = vec![b'x'; MAX_IPC_LINE_BYTES + 1];
            bw.write_all(&big).await.unwrap();
            bw.write_all(b"\n").await.unwrap();
        });
        let got: std::io::Result<Option<IpcResponse>> = read_msg(&mut ar).await;
        assert!(got.is_err());
        drop(ar);
        writer.await.unwrap();
    }
}
