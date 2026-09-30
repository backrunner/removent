use super::*;

impl Engine {
    /// Publish geometry on the same FIFO as input, and resend it after resume.
    pub fn set_frame_geometry(&self, generation: usize, width: u32, height: u32) {
        let failed = {
            let mut channels = self.client_channels.lock().unwrap();
            if generation != channels.generation || channels.geometry == Some((width, height)) {
                return;
            }
            let Some(tx) = &channels.cmd else {
                return;
            };
            match tx.try_send(ControlMsg::FrameGeometry { width, height }) {
                Ok(()) => {
                    channels.geometry = Some((width, height));
                    false
                }
                Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => true,
                Err(_) => false,
            }
        };
        if failed {
            self.cancel_client(Some(generation), t!("session.input_overloaded").to_string());
        }
    }

    /// Buffer native/VNC transitions and coalesce motion; RDP reserves FIFO
    /// space for transitions. A hard overflow closes the session explicitly
    /// instead of silently losing a key/button release.
    pub(super) fn try_send_cmd(&self, msg: ControlMsg) {
        let failed_generation = {
            let channels = self.client_channels.lock().unwrap();
            channels.cmd.as_ref().and_then(|tx| match tx.try_send(msg) {
                // The frame bridge owns disconnect/retry handling. A closed
                // old channel must not abort a reconnect that is starting.
                Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => Some(channels.generation),
                _ => None,
            })
        };
        if let Some(generation) = failed_generation {
            self.cancel_client(Some(generation), t!("session.input_overloaded").to_string());
        }
    }

    pub fn send_input_mouse(
        &self,
        display_id: u64,
        x_px: f32,
        y_px: f32,
        buttons: u8,
        kind: removent_proto::MouseKind,
    ) {
        self.try_send_cmd(ControlMsg::MouseEvent {
            display_id,
            x_px,
            y_px,
            buttons,
            kind,
        });
    }

    pub fn send_input_key(
        &self,
        vk_code: u16,
        modifiers: removent_proto::KeyModifiers,
        kind: removent_proto::KeyKind,
        unicode: Option<char>,
    ) {
        self.try_send_cmd(ControlMsg::KeyEvent {
            vk_code,
            modifiers,
            kind,
            unicode,
        });
    }

    pub fn send_input_scroll(
        &self,
        display_id: u64,
        dx_mm: f32,
        dy_mm: f32,
        phase: removent_proto::ScrollPhase,
    ) {
        self.try_send_cmd(ControlMsg::ScrollEvent {
            display_id,
            dx_mm,
            dy_mm,
            phase,
        });
    }

    /// Clear the LOCAL clipboard (NSPasteboard). The viewer's focus-loss clearing
    /// wipes the local pasteboard — where the sensitive remote content landed —
    /// and leaves the remote clipboard alone (it may hold the host user's own
    /// content). The clipboard poller skips empty local content, so this never
    /// propagates to the peer.
    pub fn clear_local_clipboard(&self) {
        if let Err(e) = rinput::write_text("") {
            tracing::warn!(err=%e, "local clipboard clear failed");
        }
    }
}
