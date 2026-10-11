use apple_cf::dispatch_queue::{
    dispatch_after, dispatch_apply, dispatch_async, dispatch_async_and_wait, DispatchGroup,
    DispatchQoS, DispatchQueue, DispatchSemaphore, DispatchSource,
};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    mpsc, Arc,
};
use std::thread;
use std::time::{Duration, Instant};

#[test]
fn dispatch_sync_primitives_work() {
    let group = DispatchGroup::new();
    group.enter();
    group.leave();
    assert!(group.wait(Some(Duration::from_millis(10))));

    let semaphore = DispatchSemaphore::new(0).expect("semaphore");
    assert_eq!(semaphore.signal(), 0);
    assert!(semaphore.wait(Some(Duration::from_millis(10))));

    let source = DispatchSource::timer(Duration::from_millis(5), Duration::from_millis(1));
    source.resume();
    thread::sleep(Duration::from_millis(30));
    source.cancel();
    assert!(source.fire_count() > 0);

    let queue = DispatchQueue::new(
        "com.doomfish.apple-cf.dispatch-primitives-tests",
        DispatchQoS::UserInitiated,
    );
    let counter = Arc::new(AtomicUsize::new(0));
    let async_group = DispatchGroup::new();
    async_group.enter();
    let async_group_done = async_group.clone();
    let async_counter = Arc::clone(&counter);
    dispatch_async(&queue, move || {
        async_counter.fetch_add(1, Ordering::SeqCst);
        async_group_done.leave();
    });
    assert!(async_group.wait(Some(Duration::from_secs(1))));

    let waited_counter = Arc::clone(&counter);
    dispatch_async_and_wait(&queue, move || {
        waited_counter.fetch_add(1, Ordering::SeqCst);
    });
    assert_eq!(counter.load(Ordering::SeqCst), 2);

    let total = Arc::new(AtomicUsize::new(0));
    let total_clone = Arc::clone(&total);
    dispatch_apply(4, &queue, move |index| {
        total_clone.fetch_add(index + 1, Ordering::SeqCst);
    });
    assert_eq!(total.load(Ordering::SeqCst), 10);
}

#[test]
fn dispatch_source_lifecycle_is_balanced_and_idempotent() {
    drop(DispatchSource::timer(
        Duration::from_millis(5),
        Duration::from_millis(1),
    ));

    let cancelled_before_resume =
        DispatchSource::timer(Duration::from_millis(5), Duration::from_millis(1));
    cancelled_before_resume.cancel();
    cancelled_before_resume.cancel();
    cancelled_before_resume.resume();
    drop(cancelled_before_resume);

    let source = DispatchSource::timer(Duration::from_millis(1), Duration::from_millis(1));
    let resume_threads: Vec<_> = (0..8)
        .map(|_| {
            let source = source.clone();
            thread::spawn(move || {
                for _ in 0..100 {
                    source.resume();
                }
            })
        })
        .collect();
    for thread in resume_threads {
        thread.join().expect("resume thread");
    }

    thread::sleep(Duration::from_millis(20));
    let retained = source.clone();
    drop(source);
    assert!(retained.fire_count() > 0);

    let cancel_threads: Vec<_> = (0..8)
        .map(|_| {
            let source = retained.clone();
            thread::spawn(move || {
                for _ in 0..100 {
                    source.cancel();
                }
            })
        })
        .collect();
    for thread in cancel_threads {
        thread.join().expect("cancel thread");
    }

    retained.cancel();
    retained.resume();
}

#[test]
fn dispatch_source_fire_count_is_monotonic_across_threads() {
    let source = DispatchSource::timer(Duration::from_millis(1), Duration::from_millis(1));
    source.resume();

    let readers: Vec<_> = (0..4)
        .map(|_| {
            let source = source.clone();
            thread::spawn(move || {
                let mut previous = 0;
                for _ in 0..1_000 {
                    let current = source.fire_count();
                    assert!(current >= previous);
                    previous = current;
                }
            })
        })
        .collect();

    for reader in readers {
        reader.join().expect("fire-count reader");
    }
    source.cancel();
}

#[test]
fn semaphore_rejects_negative_initial_counts() {
    assert!(DispatchSemaphore::new(-1).is_none());
    assert!(DispatchSemaphore::new(i64::MIN).is_none());
    let semaphore = DispatchSemaphore::new(1).expect("semaphore");
    assert!(semaphore.wait(Some(Duration::ZERO)));
    assert!(!semaphore.wait(Some(Duration::ZERO)));
}

#[test]
fn semaphore_released_below_its_initial_count_does_not_abort() {
    let semaphore = DispatchSemaphore::new(2).expect("semaphore");
    let clone = semaphore.clone();
    assert!(semaphore.wait(Some(Duration::ZERO)));
    assert!(clone.wait(Some(Duration::ZERO)));
    drop(semaphore);
    drop(clone);
}

#[test]
fn group_tolerates_unbalanced_leave_and_release_while_entered() {
    let group = DispatchGroup::new();
    group.leave();
    assert!(group.wait(Some(Duration::ZERO)));
    group.enter();
    assert!(!group.wait(Some(Duration::ZERO)));
    group.enter();
    group.leave();
    assert!(!group.wait(Some(Duration::ZERO)));
    drop(group);
}

#[test]
fn timer_source_accepts_extreme_and_sub_millisecond_durations() {
    let never = DispatchSource::timer(Duration::MAX, Duration::MAX);
    never.resume();
    thread::sleep(Duration::from_millis(5));
    never.cancel();
    assert_eq!(never.fire_count(), 0);

    let fast = DispatchSource::timer(Duration::from_micros(500), Duration::ZERO);
    fast.resume();
    thread::sleep(Duration::from_millis(30));
    fast.cancel();
    assert!(fast.fire_count() > 0);
}

#[test]
fn queue_labels_may_contain_nul() {
    let queue = DispatchQueue::new("com.doomfish.apple-cf\0ignored", DispatchQoS::Default);
    let (sender, receiver) = mpsc::channel();
    dispatch_async(&queue, move || sender.send(()).expect("send"));
    receiver
        .recv_timeout(Duration::from_secs(5))
        .expect("queue ran the work item");
}

#[test]
fn dispatch_after_runs_work_on_a_global_queue_after_the_delay() {
    let (sender, receiver) = mpsc::channel();
    let started = Instant::now();
    dispatch_after(
        Duration::from_millis(20),
        &DispatchQueue::global(DispatchQoS::Utility),
        move || sender.send(started.elapsed()).expect("send"),
    );
    let elapsed = receiver
        .recv_timeout(Duration::from_secs(5))
        .expect("delayed work ran");
    assert!(elapsed >= Duration::from_millis(20));
}

#[test]
fn concurrent_queue_runs_every_apply_iteration() {
    let queue = DispatchQueue::concurrent(
        "com.doomfish.apple-cf.concurrent-tests",
        DispatchQoS::UserInitiated,
    );
    let counter = Arc::new(AtomicUsize::new(0));
    let iterations = Arc::clone(&counter);
    dispatch_apply(16, &queue, move |_| {
        iterations.fetch_add(1, Ordering::SeqCst);
    });
    assert_eq!(counter.load(Ordering::SeqCst), 16);
}

#[test]
fn main_queue_handles_share_the_main_queue() {
    let main = DispatchQueue::main();
    let clone = main.clone();
    assert_eq!(main.as_ptr(), clone.as_ptr());
    assert_eq!(DispatchQueue::main().as_ptr(), main.as_ptr());
    assert_ne!(
        DispatchQueue::global(DispatchQoS::Background).as_ptr(),
        main.as_ptr()
    );
}
