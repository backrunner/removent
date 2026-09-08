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
    /// Explicit interactive setup, requested in the daemon's own TCC identity.
    RequestPermissions,
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
/// Use a persistent [`MessageReader`] when the read may be cancelled and retried.
///
/// Blank lines are skipped (a stray `\n` is not a disconnect); lines longer than
/// [`MAX_IPC_LINE_BYTES`] are rejected with an error.
pub async fn read_msg<R, T>(r: &mut R) -> std::io::Result<Option<T>>
where
    R: AsyncBufRead + Unpin,
    T: for<'de> Deserialize<'de>,
{
    MessageReader::default().read(r).await
}

/// Retains partial JSON Lines across cancellation (for use inside `select!`).
#[derive(Default)]
pub struct MessageReader {
    line: Vec<u8>,
}

impl MessageReader {
    pub async fn read<R, T>(&mut self, r: &mut R) -> std::io::Result<Option<T>>
    where
        R: AsyncBufRead + Unpin,
        T: for<'de> Deserialize<'de>,
    {
        loop {
            let chunk = r.fill_buf().await?;
            let eof = chunk.is_empty();
            let end = chunk.iter().position(|b| *b == b'\n');
            let len = end.map_or(chunk.len(), |i| i + 1);
            if self.line.len() + len > MAX_IPC_LINE_BYTES {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "ipc line too long",
                ));
            }
            self.line.extend_from_slice(&chunk[..len]);
            r.consume(len);
            if !eof && end.is_none() {
                continue;
            }
            let line = std::mem::take(&mut self.line);
            let text = std::str::from_utf8(&line)
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
            let trimmed = text.trim();
            if trimmed.is_empty() {
                if eof {
                    return Ok(None);
                }
                continue;
            }
            return serde_json::from_str(trimmed)
                .map(Some)
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e));
        }
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
    async fn cancelled_read_retains_partial_utf8_and_json() {
        let (mut writer, reader) = tokio::io::duplex(128);
        let mut reader = tokio::io::BufReader::new(reader);
        let mut messages = MessageReader::default();
        let json = "{\"name\":\"中\"}\n".as_bytes();
        let split = 10; // In the middle of the three-byte UTF-8 character.
        writer.write_all(&json[..split]).await.unwrap();
        tokio::select! {
            biased;
            result = messages.read::<_, serde_json::Value>(&mut reader) => panic!("partial message completed: {result:?}"),
            _ = std::future::ready(()) => {}
        }
        writer.write_all(&json[split..]).await.unwrap();
        let value: serde_json::Value = messages.read(&mut reader).await.unwrap().unwrap();
        assert_eq!(value["name"], "中");
    }

    #[tokio::test]
    async fn oversized_unterminated_line_is_rejected_without_waiting_for_eof() {
        let (mut writer, reader) = tokio::io::duplex(MAX_IPC_LINE_BYTES + 1);
        let mut reader = tokio::io::BufReader::new(reader);
        writer
            .write_all(&vec![b'x'; MAX_IPC_LINE_BYTES + 1])
            .await
            .unwrap();
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(1),
            read_msg::<_, IpcResponse>(&mut reader),
        )
        .await
        .expect("must reject before the sender closes or sends a newline");
        assert_eq!(result.unwrap_err().kind(), std::io::ErrorKind::InvalidData);
    }

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
            let _ = bw.write_all(&big).await;
        });
        let got: std::io::Result<Option<IpcResponse>> = read_msg(&mut ar).await;
        assert!(got.is_err());
        drop(ar);
        writer.await.unwrap();
    }
}
