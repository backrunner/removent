use super::*;

#[test]
fn dropped_pairing_display_clears_only_an_unfinished_prompt() {
    let clears = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let count = clears.clone();
    let cbs = Arc::new(HostCallbacks {
        show_pairing_pin: Box::new(|_| {}),
        admission_prompt: Box::new(|_, _| Box::pin(async { false })),
        on_event: Box::new(move |event| {
            if matches!(event, HostEvent::PairingCleared) {
                count.fetch_add(1, Ordering::SeqCst);
            }
        }),
    });
    for (shown, completed) in [(false, false), (true, true), (true, false)] {
        let flag = Arc::new(AtomicBool::new(shown));
        let guard = PairingDisplayGuard {
            shown: flag.clone(),
            cbs: cbs.clone(),
        };
        if completed {
            flag.store(false, Ordering::SeqCst);
        }
        drop(guard);
    }
    assert_eq!(clears.load(Ordering::SeqCst), 1);
}
