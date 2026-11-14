//! Keep native dialogs responsive to daemon resolution, cancellation and disconnects.
use crate::dialogs::Dialog;
use removent_core::ipc::IpcRequest;
use std::{
    collections::VecDeque,
    time::{Duration, Instant},
};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Admission(u64),
    Pairing,
    Alert,
}
struct Pending {
    kind: Kind,
    title: String,
    text: String,
    ready: Instant,
    expires: Instant,
}
struct Active {
    pending: Pending,
    dialog: Dialog,
    cancelled: bool,
}
#[derive(Default)]
pub struct Prompts {
    queue: VecDeque<Pending>,
    active: Option<Active>,
    pairing: Option<String>,
}
impl Prompts {
    pub fn admission(
        &mut self,
        id: u64,
        name: &str,
        fingerprint: &str,
        title: &str,
        desktop_open: bool,
    ) {
        if self.queue.iter().any(|p| p.kind == Kind::Admission(id))
            || self
                .active
                .as_ref()
                .is_some_and(|p| p.pending.kind == Kind::Admission(id))
        {
            return;
        }
        let now = Instant::now();
        // Match the desktop-first policy of the macOS tray. The daemon remains
        // the authority for the 30-second timeout and for competing answers.
        self.enqueue(Pending {
            kind: Kind::Admission(id),
            title: title.into(),
            text: format!("{name}\n{fingerprint}"),
            ready: now + Duration::from_secs(if desktop_open { 8 } else { 0 }),
            expires: now + Duration::from_secs(30),
        });
    }
    pub fn resolved(&mut self, id: u64) {
        self.cancel(Kind::Admission(id));
    }
    pub fn pairing(&mut self, code: String, title: &str) {
        if self.pairing.as_ref() == Some(&code) {
            return;
        }
        self.cancel(Kind::Pairing);
        self.pairing = Some(code.clone());
        let now = Instant::now();
        self.enqueue(Pending {
            kind: Kind::Pairing,
            title: title.into(),
            text: code,
            ready: now,
            expires: now + Duration::from_secs(300),
        });
    }
    pub fn reset_pairing(&mut self) {
        self.pairing = None;
        self.cancel(Kind::Pairing);
    }
    pub fn alert(&mut self, text: String) {
        if self.queue.iter().any(|p| p.text == text)
            || self.active.as_ref().is_some_and(|p| p.pending.text == text)
        {
            return;
        }
        let now = Instant::now();
        self.enqueue(Pending {
            kind: Kind::Alert,
            title: "Removent".into(),
            text,
            ready: now,
            expires: now + Duration::from_secs(300),
        });
    }
    fn enqueue(&mut self, pending: Pending) {
        // A remote peer cannot create an unbounded native-dialog backlog.
        if self.queue.len() < 16 {
            self.queue.push_back(pending);
        }
    }
    fn cancel(&mut self, kind: Kind) {
        self.queue.retain(|p| p.kind != kind);
        if let Some(active) = self.active.as_mut().filter(|p| p.pending.kind == kind) {
            active.cancelled = true;
            active.dialog.close();
        }
    }
    pub fn disconnected(&mut self) {
        self.queue.clear();
        self.pairing = None;
        if let Some(active) = &mut self.active {
            active.cancelled = true;
            active.dialog.close();
        }
    }
    pub fn poll(&mut self, send: &tokio::sync::mpsc::UnboundedSender<IpcRequest>) {
        let now = Instant::now();
        self.queue.retain(|p| p.expires > now);
        if let Some(active) = &mut self.active {
            if active.pending.expires <= now {
                active.cancelled = true;
                active.dialog.close();
            }
            if let Some(allow) = active.dialog.poll() {
                if !active.cancelled
                    && let Kind::Admission(request_id) = active.pending.kind
                {
                    let _ = send.send(IpcRequest::AdmissionReply { request_id, allow });
                }
                self.active = None;
            }
        }
        if self.active.is_none()
            && let Some(index) = self.queue.iter().position(|p| p.ready <= now)
        {
            let pending = self.queue.remove(index).unwrap();
            let dialog = Dialog::new(
                &pending.title,
                &pending.text,
                matches!(pending.kind, Kind::Admission(_)),
            );
            self.active = Some(Active {
                pending,
                dialog,
                cancelled: false,
            });
        }
    }
}
