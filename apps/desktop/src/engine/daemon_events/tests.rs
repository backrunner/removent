use super::*;

#[test]
fn ended_sessions_never_underflow() {
    let sessions = AtomicUsize::new(1);
    decrement_session_count(&sessions);
    assert_eq!(sessions.load(Ordering::SeqCst), 0);
    decrement_session_count(&sessions);
    assert_eq!(sessions.load(Ordering::SeqCst), 0);
}

#[test]
fn concurrent_session_updates_do_not_lose_counts() {
    let sessions = AtomicUsize::new(4096);
    let barrier = std::sync::Barrier::new(8);
    std::thread::scope(|scope| {
        for _ in 0..8 {
            scope.spawn(|| {
                barrier.wait();
                for _ in 0..512 {
                    sessions.fetch_add(1, Ordering::SeqCst);
                    decrement_session_count(&sessions);
                    decrement_session_count(&sessions);
                }
            });
        }
    });
    assert_eq!(sessions.load(Ordering::SeqCst), 0);
}
