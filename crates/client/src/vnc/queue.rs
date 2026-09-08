//! Ordered input with enough headroom for transient stalls. Only adjacent
//! pointer motion with identical button state may be replaced; transitions,
//! scrolls, geometry, and the final pointer position retain their FIFO order.

use removent_proto::{ControlMsg, MouseKind};
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::{Notify, mpsc::error::TrySendError};

const CAPACITY: usize = 4096;

#[derive(Clone, Copy, Debug, Default)]
pub struct InputSnapshot {
    pub pending: usize,
    pub coalesced: u64,
    pub sent: u64,
    /// Local enqueue-to-socket-write time, not remote acknowledgement or RTT.
    pub dispatch_delay: Option<Duration>,
    pub oldest_pending: Option<Duration>,
}

pub(super) struct PendingInput {
    pub message: ControlMsg,
    pub queued_at: Instant,
}

struct State {
    queue: VecDeque<PendingInput>,
    senders: usize,
    receiver_alive: bool,
    coalesced: u64,
    sent: u64,
    dispatch_delay: Option<Duration>,
    in_flight: Option<Instant>,
}

struct Shared {
    state: Mutex<State>,
    ready: Notify,
    space: Notify,
}

pub struct InputSender(Arc<Shared>);
pub(super) struct InputReceiver(Arc<Shared>);

pub(super) fn channel() -> (InputSender, InputReceiver) {
    let shared = Arc::new(Shared {
        state: Mutex::new(State {
            queue: VecDeque::new(),
            senders: 1,
            receiver_alive: true,
            coalesced: 0,
            sent: 0,
            dispatch_delay: None,
            in_flight: None,
        }),
        ready: Notify::new(),
        space: Notify::new(),
    });
    (InputSender(shared.clone()), InputReceiver(shared))
}

impl InputSender {
    pub fn try_send(&self, message: ControlMsg) -> Result<(), TrySendError<ControlMsg>> {
        let mut state = self.0.state.lock().unwrap();
        if !state.receiver_alive {
            return Err(TrySendError::Closed(message));
        }
        if state
            .queue
            .back()
            .is_some_and(|last| can_replace(&last.message, &message))
        {
            *state.queue.back_mut().unwrap() = PendingInput {
                message,
                queued_at: Instant::now(),
            };
            state.coalesced += 1;
        } else {
            if state.queue.len() == CAPACITY {
                return Err(TrySendError::Full(message));
            }
            state.queue.push_back(PendingInput {
                message,
                queued_at: Instant::now(),
            });
        }
        drop(state);
        self.0.ready.notify_one();
        Ok(())
    }

    pub async fn send(
        &self,
        mut message: ControlMsg,
    ) -> Result<(), tokio::sync::mpsc::error::SendError<ControlMsg>> {
        loop {
            let space = self.0.space.notified();
            tokio::pin!(space);
            space.as_mut().enable();
            match self.try_send(message) {
                Ok(()) => return Ok(()),
                Err(TrySendError::Closed(message)) => {
                    return Err(tokio::sync::mpsc::error::SendError(message));
                }
                Err(TrySendError::Full(value)) => message = value,
            }
            space.await;
        }
    }

    pub fn snapshot(&self) -> InputSnapshot {
        let state = self.0.state.lock().unwrap();
        InputSnapshot {
            pending: state.queue.len() + usize::from(state.in_flight.is_some()),
            coalesced: state.coalesced,
            sent: state.sent,
            dispatch_delay: state.dispatch_delay,
            oldest_pending: state
                .in_flight
                .or_else(|| state.queue.front().map(|event| event.queued_at))
                .map(|queued| queued.elapsed()),
        }
    }
}

impl Clone for InputSender {
    fn clone(&self) -> Self {
        self.0.state.lock().unwrap().senders += 1;
        Self(self.0.clone())
    }
}

impl Drop for InputSender {
    fn drop(&mut self) {
        let mut state = self.0.state.lock().unwrap();
        state.senders -= 1;
        let closed = state.senders == 0;
        drop(state);
        if closed {
            self.0.ready.notify_one();
        }
    }
}

impl InputReceiver {
    pub async fn recv(&mut self) -> Option<PendingInput> {
        loop {
            let ready = self.0.ready.notified();
            {
                let mut state = self.0.state.lock().unwrap();
                if let Some(event) = state.queue.pop_front() {
                    state.in_flight = Some(event.queued_at);
                    drop(state);
                    self.0.space.notify_waiters();
                    return Some(event);
                }
                if state.senders == 0 {
                    return None;
                }
            }
            ready.await;
        }
    }

    pub fn record_sent(&self, queued_at: Instant, is_input: bool) {
        let mut state = self.0.state.lock().unwrap();
        state.in_flight = None;
        if is_input {
            state.sent += 1;
            state.dispatch_delay = Some(queued_at.elapsed());
        }
    }
}

impl Drop for InputReceiver {
    fn drop(&mut self) {
        let mut state = self.0.state.lock().unwrap();
        state.receiver_alive = false;
        state.queue.clear();
        state.in_flight = None;
        drop(state);
        self.0.space.notify_waiters();
    }
}

fn motion(message: &ControlMsg) -> Option<(u64, u8)> {
    match message {
        ControlMsg::MouseEvent {
            display_id,
            buttons,
            kind:
                MouseKind::Moved
                | MouseKind::LeftDragged
                | MouseKind::RightDragged
                | MouseKind::MiddleDragged,
            ..
        } => Some((*display_id, *buttons)),
        _ => None,
    }
}

fn can_replace(previous: &ControlMsg, next: &ControlMsg) -> bool {
    motion(previous).is_some_and(|state| Some(state) == motion(next))
}

#[cfg(test)]
mod tests {
    use super::*;
    use removent_proto::{KeyKind, KeyModifiers, ScrollPhase};

    fn pointer(x: f32, buttons: u8, kind: MouseKind) -> ControlMsg {
        ControlMsg::MouseEvent {
            display_id: 0,
            x_px: x,
            y_px: 10.,
            buttons,
            kind,
        }
    }

    fn key(kind: KeyKind) -> ControlMsg {
        ControlMsg::KeyEvent {
            vk_code: 0,
            modifiers: KeyModifiers::empty(),
            kind,
            unicode: Some('a'),
        }
    }

    #[tokio::test]
    async fn slow_writer_keeps_bursts_and_final_motion_in_order() {
        let (tx, mut rx) = channel();
        for _ in 0..500 {
            tx.try_send(key(KeyKind::Down)).unwrap();
            for x in 0..100 {
                tx.try_send(pointer(x as f32, 0, MouseKind::Moved)).unwrap();
            }
            tx.try_send(key(KeyKind::Up)).unwrap();
        }
        assert_eq!(tx.snapshot().pending, 1500);
        assert_eq!(tx.snapshot().coalesced, 49_500);
        drop(tx);
        for _ in 0..500 {
            assert!(matches!(
                rx.recv().await.unwrap().message,
                ControlMsg::KeyEvent {
                    kind: KeyKind::Down,
                    ..
                }
            ));
            assert!(matches!(
                rx.recv().await.unwrap().message,
                ControlMsg::MouseEvent { x_px: 99., .. }
            ));
            assert!(matches!(
                rx.recv().await.unwrap().message,
                ControlMsg::KeyEvent {
                    kind: KeyKind::Up,
                    ..
                }
            ));
        }
        assert!(rx.recv().await.is_none());
    }

    #[tokio::test]
    async fn clicks_scrolls_and_button_state_changes_are_barriers() {
        let (tx, mut rx) = channel();
        let events = [
            pointer(1., 0, MouseKind::Moved),
            pointer(2., 1, MouseKind::LeftDown),
            pointer(3., 1, MouseKind::Moved),
            ControlMsg::ScrollEvent {
                display_id: 0,
                dx_mm: 0.,
                dy_mm: 1.,
                phase: ScrollPhase::Changed,
            },
            pointer(4., 1, MouseKind::Moved),
            pointer(5., 0, MouseKind::LeftUp),
            pointer(6., 0, MouseKind::Moved),
            pointer(7., 1, MouseKind::Moved),
        ];
        for event in &events {
            tx.try_send(event.clone()).unwrap();
        }
        assert_eq!(tx.snapshot().coalesced, 0);
        for event in events {
            assert_eq!(rx.recv().await.unwrap().message, event);
        }
    }

    #[tokio::test]
    async fn overflow_is_explicit_and_waiting_senders_wake_on_close() {
        let (tx, rx) = channel();
        for _ in 0..CAPACITY {
            tx.try_send(key(KeyKind::Up)).unwrap();
        }
        assert!(matches!(
            tx.try_send(key(KeyKind::Up)),
            Err(TrySendError::Full(_))
        ));
        let sender = tx.clone();
        let waiting = tokio::spawn(async move { sender.send(key(KeyKind::Up)).await });
        tokio::task::yield_now().await;
        drop(rx);
        assert!(
            tokio::time::timeout(Duration::from_secs(1), waiting)
                .await
                .unwrap()
                .unwrap()
                .is_err()
        );
        assert!(matches!(
            tx.try_send(key(KeyKind::Up)),
            Err(TrySendError::Closed(_))
        ));
    }
}
