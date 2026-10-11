use super::*;
use std::sync::atomic::{AtomicBool, AtomicUsize};

#[tokio::test(start_paused = true)]
async fn silent_retries_keep_session_alive_and_have_a_finite_budget() {
    let recovery = Recovery::default();
    let attempts = AtomicUsize::new(0);
    let start = Instant::now();
    let result = recovery
        .run_with_network(
            || async {
                let attempt = attempts.fetch_add(1, Ordering::SeqCst);
                if attempt == 0 {
                    recovery.connected();
                }
                if attempt > 0 {
                    assert_eq!(
                        recovery.state(),
                        State::Reconnecting {
                            after_offline: false
                        }
                    );
                }
                anyhow::bail!("server unavailable")
            },
            || true,
        )
        .await;
    assert!(result.unwrap_err().to_string().contains("retry limit"));
    assert_eq!(attempts.load(Ordering::SeqCst), 6); // initial + five retries
    assert_eq!(Instant::now() - start, Duration::from_secs(15));
}

#[tokio::test(start_paused = true)]
async fn stalled_reconnects_obey_total_deadline() {
    let recovery = Recovery::default();
    let attempts = AtomicUsize::new(0);
    let start = Instant::now();
    let result = recovery
        .run_with_network(
            || async {
                if attempts.fetch_add(1, Ordering::SeqCst) == 0 {
                    recovery.connected();
                    anyhow::bail!("lost transport");
                }
                std::future::pending::<anyhow::Result<()>>().await
            },
            || true,
        )
        .await;
    assert!(result.is_err());
    assert_eq!(Instant::now() - start, Duration::from_secs(45));
}

#[tokio::test(start_paused = true)]
async fn offline_cancels_live_transport_waits_without_spending_budget_and_recovers() {
    let recovery = Recovery::default();
    let online = AtomicBool::new(true);
    let attempts = AtomicUsize::new(0);
    let dropped = AtomicUsize::new(0);
    struct Guard<'a>(&'a AtomicUsize);
    impl Drop for Guard<'_> {
        fn drop(&mut self) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }
    let result = recovery.run_with_network(
        || async {
            let _guard = Guard(&dropped);
            if attempts.fetch_add(1, Ordering::SeqCst) == 0 {
                recovery.connected();
                online.store(false, Ordering::SeqCst);
                std::future::pending::<()>().await;
            }
            assert_eq!(dropped.load(Ordering::SeqCst), 1);
            assert_eq!(
                recovery.state(),
                State::Reconnecting {
                    after_offline: true
                }
            );
            recovery.connected();
            // A healthy session must not be killed by the old retry deadline.
            tokio::time::sleep(Duration::from_secs(120)).await;
            Ok(())
        },
        || online.load(Ordering::SeqCst),
    );
    let network = async {
        tokio::time::sleep(Duration::from_secs(120)).await;
        assert_eq!(recovery.state(), State::Offline);
        assert_eq!(attempts.load(Ordering::SeqCst), 1);
        online.store(true, Ordering::SeqCst);
    };
    let (result, _) = tokio::join!(result, network);
    result.unwrap();
    assert_eq!(attempts.load(Ordering::SeqCst), 2);
}

#[tokio::test(start_paused = true)]
async fn second_interruption_gets_a_fresh_budget_and_explicit_close_never_retries() {
    let recovery = Recovery::default();
    let attempts = AtomicUsize::new(0);
    recovery
        .run_with_network(
            || async {
                let n = attempts.fetch_add(1, Ordering::SeqCst);
                if n == 1 || n == 3 {
                    anyhow::bail!("still unavailable");
                }
                recovery.connected();
                if n < 4 {
                    anyhow::bail!("transport interrupted");
                }
                Ok(())
            },
            || true,
        )
        .await
        .unwrap();
    assert_eq!(attempts.load(Ordering::SeqCst), 5);
}

#[tokio::test(start_paused = true)]
async fn initial_authentication_failure_is_not_retried() {
    let recovery = Recovery::default();
    let attempts = AtomicUsize::new(0);
    assert!(
        recovery
            .run_with_network(
                || async {
                    attempts.fetch_add(1, Ordering::SeqCst);
                    anyhow::bail!("wrong password")
                },
                || true
            )
            .await
            .is_err()
    );
    assert_eq!(attempts.load(Ordering::SeqCst), 1);
}

#[tokio::test(start_paused = true)]
async fn rapid_handshake_drop_loops_cannot_reset_the_retry_budget() {
    let recovery = Recovery::default();
    let attempts = AtomicUsize::new(0);
    let result = recovery
        .run_with_network(
            || async {
                attempts.fetch_add(1, Ordering::SeqCst);
                recovery.connected();
                anyhow::bail!("dropped immediately after handshake")
            },
            || true,
        )
        .await;
    assert!(result.is_err());
    assert_eq!(attempts.load(Ordering::SeqCst), 6);
}
