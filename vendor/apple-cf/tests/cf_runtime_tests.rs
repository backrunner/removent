use apple_cf::cf::{
    CFFileDescriptor, CFMessagePort, CFNotificationCenter, CFRunLoop, CFRunLoopRunResult, CFSocket,
    CFStreamPair, CFString, CFTimer,
};
use apple_cf::cf::CFDictionary;
use std::io::{Read, Write};
use std::os::fd::{AsFd, AsRawFd, OwnedFd};
use std::os::unix::net::UnixStream;
use std::process;
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[test]
fn cf_runtime_wrappers_work() {
    let center = CFNotificationCenter::local();
    center.post(
        &CFString::new("com.doomfish.apple-cf.runtime-tests"),
        None,
        false,
    );

    let timer = CFTimer::new(Duration::from_millis(10), false);
    let run_loop = CFRunLoop::current();
    run_loop.add_timer(&timer);
    assert!(timer.is_valid());
    let result = CFRunLoop::run_in_default_mode(Duration::from_millis(20), true);
    assert!(matches!(
        result,
        CFRunLoopRunResult::Finished
            | CFRunLoopRunResult::Stopped
            | CFRunLoopRunResult::TimedOut
            | CFRunLoopRunResult::HandledSource
    ));

    let name = format!("com.doomfish.apple-cf.echo.test.{}", process::id());
    let _local = CFMessagePort::create_echo_local(&name).expect("local port");
    let remote = CFMessagePort::connect_remote(&name).expect("remote port");
    let reply = remote
        .send_request(b"ping", Duration::from_millis(100))
        .expect("reply");
    assert_eq!(reply, b"ping");

    let pair = CFStreamPair::new(1024);
    assert!(pair.read.open());
    assert!(pair.write.open());
    assert_eq!(pair.write.write(b"ok").expect("write"), 2);
    let mut buffer = [0_u8; 2];
    assert_eq!(pair.read.read(&mut buffer).expect("read"), 2);
    assert_eq!(&buffer, b"ok");
    pair.read.close();
    pair.write.close();

    let socket = CFSocket::udp_ipv4().expect("socket");
    assert!(socket.is_valid());
    assert!(socket.native() >= 0);

    let stdin = std::io::stdin();
    let fd = CFFileDescriptor::from_borrowed_fd(std::os::fd::AsFd::as_fd(&stdin))
        .expect("file descriptor");
    assert_eq!(fd.native_descriptor(), 0);
}

#[test]
fn message_port_names_with_nul_are_rejected() {
    assert!(CFMessagePort::create_echo_local("com.doomfish.apple-cf\0echo").is_none());
    assert!(CFMessagePort::connect_remote("com.doomfish.apple-cf\0echo").is_none());
}

#[test]
fn owned_file_descriptor_is_closed_when_the_last_reference_drops() {
    let (local, mut peer) = UnixStream::pair().expect("socket pair");
    peer.set_read_timeout(Some(Duration::from_secs(5)))
        .expect("read timeout");
    let descriptor = CFFileDescriptor::from_owned_fd(OwnedFd::from(local)).expect("descriptor");
    let clone = descriptor.clone();
    drop(descriptor);
    drop(clone);
    let mut buffer = [0_u8; 1];
    assert_eq!(peer.read(&mut buffer).expect("peer read"), 0);
}

#[test]
fn owned_file_descriptor_is_closed_on_invalidate() {
    let (local, mut peer) = UnixStream::pair().expect("socket pair");
    peer.set_read_timeout(Some(Duration::from_secs(5)))
        .expect("read timeout");
    let descriptor = CFFileDescriptor::from_owned_fd(OwnedFd::from(local)).expect("descriptor");
    descriptor.invalidate();
    let mut buffer = [0_u8; 1];
    assert_eq!(peer.read(&mut buffer).expect("peer read"), 0);
}

#[test]
fn borrowed_file_descriptor_is_never_closed() {
    let (mut local, mut peer) = UnixStream::pair().expect("socket pair");
    let descriptor = CFFileDescriptor::from_borrowed_fd(local.as_fd()).expect("descriptor");
    assert_eq!(descriptor.native_descriptor(), local.as_raw_fd());
    descriptor.invalidate();
    drop(descriptor);
    local.write_all(b"x").expect("write");
    let mut buffer = [0_u8; 1];
    peer.read_exact(&mut buffer).expect("read");
    assert_eq!(&buffer, b"x");
}

#[test]
fn local_notification_observer_receives_posts_until_dropped() {
    let center = CFNotificationCenter::local();
    let name = CFString::new(&format!(
        "com.doomfish.apple-cf.observer.{}",
        process::id()
    ));
    let received = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&received);
    let observer = center.add_observer(&name, move |name, user_info| {
        sink.lock()
            .expect("sink")
            .push((name.to_string(), user_info.map(CFDictionary::len)));
    });
    let key = CFString::new("key");
    let value = CFString::new("value");
    let user_info = CFDictionary::from_pairs(&[(&key, &value)]);
    center.post(&name, Some(&user_info), true);
    center.post(&name, None, true);
    drop(observer);
    center.post(&name, None, true);
    assert_eq!(
        *received.lock().expect("received"),
        vec![(name.to_string(), Some(1)), (name.to_string(), None)]
    );
}

#[test]
fn panicking_notification_observer_does_not_unwind_into_core_foundation() {
    let center = CFNotificationCenter::local();
    let name = CFString::new(&format!(
        "com.doomfish.apple-cf.panicking-observer.{}",
        process::id()
    ));
    let calls = Arc::new(Mutex::new(0_u32));
    let counter = Arc::clone(&calls);
    let _observer = center.add_observer(&name, move |_, _| {
        *counter.lock().expect("counter") += 1;
        panic!("observer panic");
    });
    center.post(&name, None, true);
    center.post(&name, None, true);
    assert_eq!(*calls.lock().expect("calls"), 2);
}

#[test]
fn run_loops_can_be_stopped_from_another_thread() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<CFRunLoop>();

    let (run_loop_tx, run_loop_rx) = std::sync::mpsc::channel();
    let (result_tx, result_rx) = std::sync::mpsc::channel();
    let worker = std::thread::spawn(move || {
        let timer = CFTimer::new(Duration::from_secs(60), true);
        let run_loop = CFRunLoop::current();
        run_loop.add_timer(&timer);
        run_loop_tx.send(run_loop).expect("send run loop");
        let result = CFRunLoop::run_in_default_mode(Duration::from_secs(20), false);
        timer.invalidate();
        result_tx.send(result).expect("send result");
    });

    let worker_run_loop = run_loop_rx.recv().expect("worker run loop");
    assert_ne!(worker_run_loop, CFRunLoop::current());
    let result = loop {
        worker_run_loop.stop();
        worker_run_loop.wake_up();
        if let Ok(result) = result_rx.recv_timeout(Duration::from_millis(20)) {
            break result;
        }
    };
    assert_eq!(result, CFRunLoopRunResult::Stopped);
    worker.join().expect("join worker");
}

#[test]
fn run_loop_descriptions_do_not_read_the_loop_state() {
    let run_loop = CFRunLoop::current();
    let expected = format!("<CFRunLoop {:p}>", run_loop.as_ptr());
    let debug = format!("{run_loop:?}");
    assert!(debug.contains(&expected), "{debug}");
    assert!(!debug.contains("modes"), "{debug}");
    let erased = run_loop.into_cf_type();
    assert_eq!(erased.description(), expected);
    assert_eq!(erased.to_string(), expected);
    assert!(!format!("{erased:?}").contains("modes"));
}

#[test]
fn run_loops_can_be_described_while_another_thread_mutates_them() {
    let (run_loop_tx, run_loop_rx) = std::sync::mpsc::channel();
    let (done_tx, done_rx) = std::sync::mpsc::channel::<()>();
    let worker = std::thread::spawn(move || {
        let keep_alive = CFTimer::new(Duration::from_secs(60), true);
        CFRunLoop::current().add_timer(&keep_alive);
        run_loop_tx.send(CFRunLoop::current()).expect("send run loop");
        while done_rx.try_recv().is_err() {
            let _ = CFRunLoop::run_in_default_mode(Duration::from_millis(5), false);
        }
        keep_alive.invalidate();
    });
    let worker_run_loop = run_loop_rx.recv().expect("worker run loop");
    let describer = {
        let run_loop = worker_run_loop.clone();
        std::thread::spawn(move || {
            for _ in 0..2_000 {
                assert!(format!("{run_loop:?}").starts_with("CFRunLoop"));
            }
        })
    };
    for _ in 0..2_000 {
        let timer = CFTimer::new(Duration::from_secs(60), false);
        worker_run_loop.add_timer(&timer);
        assert!(timer.is_valid());
        timer.invalidate();
    }
    describer.join().expect("join describer");
    done_tx.send(()).expect("stop worker");
    worker.join().expect("join worker");
}
