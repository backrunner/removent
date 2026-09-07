//! Single-consumer, latest-value channel for live media. Sending replaces an
//! unread value instead of queuing latency; receiving moves it without a copy.

use std::sync::{Arc, Mutex};
use tokio::sync::Notify;

struct State<T> {
    value: Option<T>,
    senders: usize,
    receiver_alive: bool,
}

struct Shared<T> {
    state: Mutex<State<T>>,
    ready: Notify,
}

pub struct Sender<T>(Arc<Shared<T>>);
pub struct Receiver<T>(Arc<Shared<T>>);

pub fn channel<T>() -> (Sender<T>, Receiver<T>) {
    let shared = Arc::new(Shared {
        state: Mutex::new(State {
            value: None,
            senders: 1,
            receiver_alive: true,
        }),
        ready: Notify::new(),
    });
    (Sender(shared.clone()), Receiver(shared))
}

impl<T> Sender<T> {
    pub fn send(&self, value: T) -> Result<(), T> {
        let mut state = self.0.state.lock().unwrap();
        if !state.receiver_alive {
            return Err(value);
        }
        let old = state.value.replace(value);
        drop(state);
        self.0.ready.notify_one();
        drop(old);
        Ok(())
    }

    pub fn is_closed(&self) -> bool {
        !self.0.state.lock().unwrap().receiver_alive
    }
}

impl<T> Clone for Sender<T> {
    fn clone(&self) -> Self {
        self.0.state.lock().unwrap().senders += 1;
        Self(self.0.clone())
    }
}

impl<T> Drop for Sender<T> {
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

impl<T> Receiver<T> {
    /// Cancellation safe; an unread final value is delivered before EOF.
    pub async fn recv(&mut self) -> Option<T> {
        loop {
            let ready = self.0.ready.notified();
            {
                let mut state = self.0.state.lock().unwrap();
                if let Some(value) = state.value.take() {
                    return Some(value);
                }
                if state.senders == 0 {
                    return None;
                }
            }
            ready.await;
        }
    }
}

impl<T> Drop for Receiver<T> {
    fn drop(&mut self) {
        let mut state = self.0.state.lock().unwrap();
        state.receiver_alive = false;
        let old = state.value.take();
        drop(state);
        drop(old);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[tokio::test]
    async fn slow_consumer_gets_latest_and_final_value_before_eof() {
        let (tx, mut rx) = channel();
        for i in 0..10_000 {
            tx.send(i).unwrap();
        }
        drop(tx);
        assert_eq!(rx.recv().await, Some(9_999));
        assert_eq!(rx.recv().await, None);
    }

    #[tokio::test]
    async fn wakes_on_send_and_last_sender_drop() {
        let (tx, mut rx) = channel();
        let producer = tx.clone();
        drop(tx);
        let task = tokio::spawn(async move { (rx.recv().await, rx.recv().await) });
        tokio::task::yield_now().await;
        producer.send(42).unwrap();
        drop(producer);
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(1), task)
                .await
                .unwrap()
                .unwrap(),
            (Some(42), None)
        );
    }

    #[test]
    fn releases_replaced_frames_and_detects_receiver_drop() {
        let (tx, rx) = channel();
        let frame = Arc::new(vec![0u8; 1024]);
        tx.send(frame.clone()).unwrap();
        tx.send(Arc::new(vec![])).unwrap();
        assert_eq!(Arc::strong_count(&frame), 1);
        drop(rx);
        assert!(tx.is_closed());
        assert!(tx.send(frame).is_err());
    }
}
