//! Bounded, cancellation-safe control egress. A stalled write must not stop
//! inbound input, key releases, quality feedback or disconnect processing.
use crate::{ControlSink, NetError, Result};
use removent_proto::ControlMsg;
use std::{collections::VecDeque, future::Future, pin::Pin, time::Duration};

pub const CONTROL_PRIORITY: i32 = 10;
pub const MEDIA_PRIORITY: i32 = -10;
const MAX_PENDING: usize = 64;
const MAX_PENDING_BYTES: usize = 8 * 1024 * 1024;
type Write = Pin<Box<dyn Future<Output = Result<(ControlSink, bool)>> + Send>>;

pub struct ControlWriter {
    sink: Option<ControlSink>,
    pending: VecDeque<ControlMsg>,
    pending_bytes: usize,
    write: Option<Write>,
    ending: bool,
    clipboard: Option<crate::clipboard::ClipboardSender>,
}

impl ControlWriter {
    pub fn new(mut sink: ControlSink) -> Self {
        let _ = sink.get_mut().set_priority(CONTROL_PRIORITY);
        Self {
            sink: Some(sink),
            pending: VecDeque::new(),
            pending_bytes: 0,
            write: None,
            ending: false,
            clipboard: None,
        }
    }

    pub fn with_clipboard(sink: ControlSink, clipboard: crate::clipboard::ClipboardSender) -> Self {
        let mut writer = Self::new(sink);
        writer.clipboard = Some(clipboard);
        writer
    }

    /// Only adjacent motion with identical semantics may be replaced. Geometry,
    /// key/button/scroll transitions and the final pointer position stay ordered.
    pub fn enqueue(&mut self, message: ControlMsg) -> Result<()> {
        // SessionEnd is terminal; later responses must not interrupt its flush.
        if self.ending {
            return Ok(());
        }
        if matches!(message, ControlMsg::ClipboardSync { .. })
            && let Some(clipboard) = &self.clipboard
        {
            return crate::clipboard::enqueue(clipboard, message);
        }
        if let Some(previous) = self.pending.back_mut()
            && same_motion(previous, &message)
        {
            *previous = message;
            return Ok(());
        }
        let size = queued_size(&message);
        if self.pending.len() >= MAX_PENDING || self.pending_bytes + size > MAX_PENDING_BYTES {
            // The caller closes the session and releases held input; never
            // silently drop a key-up and leave the remote machine stuck.
            return Err(NetError::Rejected("control queue full".into()));
        }
        self.ending = matches!(message, ControlMsg::SessionEnd { .. });
        self.pending_bytes += size;
        self.pending.push_back(message);
        Ok(())
    }

    pub fn has_pending(&self) -> bool {
        self.write.is_some() || !self.pending.is_empty()
    }

    /// Apply backpressure to application producers while reserving reply space.
    pub fn accepts_commands(&self) -> bool {
        !self.ending
            && self.pending.len() < MAX_PENDING - 8
            && self.pending_bytes < MAX_PENDING_BYTES / 2 - 512
    }

    /// Safe to cancel in select!: a partially written frame stays in `write`
    /// and resumes at the same offset. Returns true after graceful SessionEnd.
    pub async fn progress(&mut self) -> Result<bool> {
        if self.write.is_none() {
            let Some(message) = self.pending.pop_front() else {
                return std::future::pending().await;
            };
            self.pending_bytes -= queued_size(&message);
            let mut sink = self.sink.take().expect("one control writer");
            self.write = Some(Box::pin(async move {
                let end = matches!(message, ControlMsg::SessionEnd { .. });
                crate::session::send_control(&mut sink, message).await?;
                if end {
                    sink.get_mut()
                        .finish()
                        .map_err(|_| NetError::Rejected("control stream closed".into()))?;
                    let _ = tokio::time::timeout(Duration::from_secs(1), sink.get_mut().stopped())
                        .await;
                }
                Ok((sink, end))
            }));
        }
        let (sink, end) = self.write.as_mut().unwrap().await?;
        self.write = None;
        self.sink = Some(sink);
        Ok(end)
    }
}

fn queued_size(message: &ControlMsg) -> usize {
    256 + match message {
        ControlMsg::ClipboardSync { data, .. } => data.len(),
        _ => 0,
    }
}

fn same_motion(a: &ControlMsg, b: &ControlMsg) -> bool {
    use removent_proto::MouseKind;
    match (a, b) {
        (
            ControlMsg::MouseEvent {
                display_id: a,
                buttons: ab,
                kind: ak,
                ..
            },
            ControlMsg::MouseEvent {
                display_id: b,
                buttons: bb,
                kind: bk,
                ..
            },
        ) => {
            a == b
                && ab == bb
                && ak == bk
                && matches!(
                    ak,
                    MouseKind::Moved
                        | MouseKind::LeftDragged
                        | MouseKind::RightDragged
                        | MouseKind::MiddleDragged
                )
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use removent_proto::{KeyKind, KeyModifiers, MouseKind};
    fn pointer(kind: MouseKind) -> ControlMsg {
        ControlMsg::MouseEvent {
            display_id: 0,
            x_px: 1.,
            y_px: 2.,
            buttons: 0,
            kind,
        }
    }
    #[test]
    fn replacement_never_crosses_input_or_geometry_barriers() {
        assert!(same_motion(
            &pointer(MouseKind::Moved),
            &pointer(MouseKind::Moved)
        ));
        for barrier in [
            pointer(MouseKind::LeftUp),
            pointer(MouseKind::LeftDragged),
            ControlMsg::KeyEvent {
                vk_code: 0,
                modifiers: KeyModifiers::empty(),
                kind: KeyKind::Up,
                unicode: None,
            },
            ControlMsg::FrameGeometry {
                width: 640,
                height: 480,
            },
        ] {
            assert!(!same_motion(&pointer(MouseKind::Moved), &barrier));
        }
    }
}
