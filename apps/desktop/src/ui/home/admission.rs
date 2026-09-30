use super::*;

impl HomeView {
    pub(super) fn answer_admission(&mut self, allow: bool, cx: &mut Context<Self>) {
        if let Some(a) = self.admission.take() {
            self.engine.answer_admission(a.request_id, allow);
            self.set_status(
                if allow {
                    t!("status.admission_allowed", peer = a.peer_name)
                } else {
                    t!("status.admission_denied", peer = a.peer_name)
                },
                if allow {
                    StatusTone::Ok
                } else {
                    StatusTone::Warn
                },
            );
        }
        cx.notify();
    }

    /// Clear the PIN input: every path that closes the Entry dialog must leave the field
    /// empty so the next Entry dialog does not show the previous 6 digits.
    pub(super) fn clear_pin_input(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.pin_input
            .update(cx, |s, cx| s.set_value("", window, cx));
    }

    pub(super) fn submit_pin(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let pin = self.pin_input.read(cx).value().trim().to_string();
        if pin.len() != 6 || !pin.bytes().all(|b| b.is_ascii_digit()) {
            return;
        }
        if !matches!(self.pin_dialog, Some(PinDialog::Entry(_))) {
            return;
        }
        if let Some(PinDialog::Entry(tx)) = self.pin_dialog.take() {
            let _ = tx.send(pin);
            self.set_connection_stage(ConnectionStage::Authenticating, cx);
            self.clear_pin_input(window, cx);
            self.set_status(t!("status.pin_submitted").to_string(), StatusTone::Info);
        }
        cx.notify();
    }

    pub(super) fn cancel_pin(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(PinDialog::Entry(tx)) = self.pin_dialog.take() {
            drop(tx);
            self.clear_pin_input(window, cx);
            self.cancel_connection(cx);
        }
        cx.notify();
    }
}
