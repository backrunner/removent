//! Clipboard snapshots use separate low-priority bidi streams after session
//! negotiation. A multi-megabyte paste must not precede key-up on the input stream.
use crate::{ControlItem, ControlSource, NetError, Result, RvpConnection};
use futures::StreamExt;
use removent_proto::{ControlDecodeOutcome, ControlMsg, decode_control, encode_control};
use std::time::Duration;
use tokio::sync::{mpsc, watch};

const MAGIC: &[u8; 4] = b"CLP1";
const TIMEOUT: Duration = Duration::from_secs(30);
const MAX_WIRE: usize = removent_core::clip::MAX_CLIPBOARD_BYTES + 128;
#[derive(Clone)]
pub struct ClipboardSender {
    snapshots: watch::Sender<Option<ControlMsg>>,
    reliable: mpsc::Sender<ControlMsg>,
}

impl ClipboardSender {
    /// Paste snapshots must survive replacement of the background sync slot.
    /// The caller waits for the peer's ClipboardAck before sending the key.
    pub async fn send_reliable(&self, message: ControlMsg) -> Result<()> {
        if !valid_snapshot(&message) {
            return Err(NetError::Rejected("invalid clipboard snapshot".into()));
        }
        self.reliable
            .send(message)
            .await
            .map_err(|_| NetError::Rejected("clipboard sender closed".into()))
    }
}

pub struct ClipboardReader {
    incoming: mpsc::Receiver<ControlMsg>,
    tasks: Vec<tokio::task::JoinHandle<()>>,
}

impl Drop for ClipboardReader {
    fn drop(&mut self) {
        for task in &self.tasks {
            task.abort();
        }
    }
}

impl ClipboardReader {
    /// The session must already have accepted its control/pairing streams.
    pub fn new(connection: RvpConnection) -> (Self, ClipboardSender) {
        let (outgoing, mut snapshots) = watch::channel(None::<ControlMsg>);
        let (reliable, mut paste) = mpsc::channel::<ControlMsg>(1);
        let (incoming_tx, incoming) = mpsc::channel(1);
        let conn = connection.clone();
        let sender = tokio::spawn(async move {
            let mut paste_seq: Option<u32> = None;
            loop {
                let (message, reliable) = tokio::select! {
                    biased;
                    Some(message) = paste.recv() => (message, true),
                    result = snapshots.changed() => {
                        if result.is_err() { break; }
                        let Some(message) = snapshots.borrow_and_update().clone() else { continue };
                        (message, false)
                    }
                };
                let ControlMsg::ClipboardSync { seq, .. } = &message else {
                    continue;
                };
                // A poll snapshot can have been queued before a forced paste but
                // reach this task later. Never let it overwrite the pasted text.
                if !reliable && paste_seq.is_some_and(|last| (*seq).wrapping_sub(last) as i32 <= 0)
                {
                    continue;
                }
                if reliable {
                    paste_seq = Some(*seq);
                }
                let Ok(wire) = encode_control(&message) else {
                    continue;
                };
                let Ok((mut send, _recv)) = conn.inner().open_bi().await else {
                    break;
                };
                let _ = send.set_priority(-20);
                let write = async {
                    send.write_all(MAGIC)
                        .await
                        .map_err(|e| NetError::Framing(e.to_string()))?;
                    crate::session::write_media(&mut send, &wire).await?;
                    send.finish()
                        .map_err(|e| NetError::Framing(e.to_string()))?;
                    send.stopped()
                        .await
                        .map_err(|e| NetError::Framing(e.to_string()))?;
                    Ok::<_, NetError>(())
                };
                if !matches!(tokio::time::timeout(TIMEOUT, write).await, Ok(Ok(()))) {
                    let _ = send.reset(0u32.into());
                    tracing::warn!("clipboard transfer expired; keeping input session alive");
                }
            }
        });
        let receiver = tokio::spawn(async move {
            while let Ok((_send, mut recv)) = connection.inner().accept_bi().await {
                let read = async {
                    let mut magic = [0; 4];
                    recv.read_exact(&mut magic).await.ok()?;
                    if &magic != MAGIC {
                        return None;
                    }
                    let wire = recv.read_to_end(MAX_WIRE).await.ok()?;
                    let (ControlDecodeOutcome::Msg(message), used) = decode_control(&wire).ok()?
                    else {
                        return None;
                    };
                    if used != wire.len() || !valid_snapshot(&message) {
                        return None;
                    }
                    Some(*message)
                };
                match tokio::time::timeout(TIMEOUT, read).await {
                    Ok(Some(message)) => {
                        if incoming_tx.send(message).await.is_err() {
                            break;
                        }
                    }
                    _ => {
                        let _ = recv.stop(0u32.into());
                    }
                }
            }
        });
        (
            Self {
                incoming,
                tasks: vec![sender, receiver],
            },
            ClipboardSender {
                snapshots: outgoing,
                reliable,
            },
        )
    }

    pub async fn next(&mut self, control: &mut ControlSource) -> Option<Result<ControlItem>> {
        tokio::select! {
            item = control.next() => item,
            Some(message) = self.incoming.recv() => Some(Ok(ControlItem::Msg(Box::new(message)))),
        }
    }
}

pub fn enqueue(sender: &ClipboardSender, message: ControlMsg) -> Result<()> {
    if !valid_snapshot(&message) {
        return Err(NetError::Rejected("invalid clipboard snapshot".into()));
    }
    // Only the newest unsent snapshot matters; in-flight transfer stays bounded.
    sender.snapshots.send_replace(Some(message));
    Ok(())
}

fn valid_snapshot(message: &ControlMsg) -> bool {
    matches!(message, ControlMsg::ClipboardSync { data, .. } if data.len() <= removent_core::clip::MAX_CLIPBOARD_BYTES)
}
