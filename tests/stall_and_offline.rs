mod support;

use std::fs;
use std::io::Write;
use std::net::TcpListener;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use fetch::{CancelToken, FetchError, Fetcher, Timings};
use support::{
    fast_timings, file_len, parse_request, part_path, payload, respond_full, scratch_dir, spawn,
    spec_for, wait_until, write_head,
};

#[test]
fn a_server_gone_silent_mid_body_stalls_in_bounded_time_and_keeps_the_part() {
    let body = payload(400_000);
    let sent = 50_000usize;
    let body_for_server = body.clone();
    let release = Arc::new(AtomicBool::new(false));
    let release_in_server = release.clone();
    let (base, count) = spawn(move |_n, mut stream| {
        let _ = parse_request(&stream);
        write_head(
            &mut stream,
            "HTTP/1.1 200 OK",
            &[
                format!("Content-Length: {}", body_for_server.len()),
                "Connection: close".to_string(),
            ],
        );
        let _ = stream.write_all(&body_for_server[..sent]);
        let _ = stream.flush();
        wait_until(Duration::from_secs(10), || {
            release_in_server.load(Ordering::SeqCst)
        });
    });
    let dir = scratch_dir("stall");
    let url = format!("{base}/stall");
    let spec = spec_for(&url, "stall.bin", &body, Some(body.len() as u64));
    let fetcher = Fetcher::with_timings(dir.path(), fast_timings(Duration::from_millis(300)));

    let started = Instant::now();
    let result = fetcher.fetch(&spec, &CancelToken::new(), |_| {});
    let elapsed = started.elapsed();
    release.store(true, Ordering::SeqCst);

    match result {
        Err(FetchError::Stalled { url: got }) => assert_eq!(got, url),
        other => panic!("expected Stalled, got {other:?}"),
    }
    assert!(
        elapsed >= Duration::from_millis(300),
        "stalled after {elapsed:?}"
    );
    assert!(elapsed < Duration::from_secs(3), "stall took {elapsed:?}");
    assert_eq!(count.load(Ordering::SeqCst), 1);
    let final_path = dir.path().join("stall.bin");
    assert!(!final_path.exists());
    assert_eq!(fs::read(part_path(&final_path)).unwrap(), body[..sent]);
}

#[test]
fn a_port_with_nothing_listening_is_offline() {
    let port = {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.local_addr().unwrap().port()
    };
    let url = format!("http://127.0.0.1:{port}/asset/offline");
    let dir = scratch_dir("offline");
    let spec = spec_for(&url, "offline.bin", &payload(100), None);
    let fetcher = Fetcher::with_timings(dir.path(), fast_timings(Duration::from_millis(300)));

    let started = Instant::now();
    match fetcher.fetch(&spec, &CancelToken::new(), |_| {}) {
        Err(FetchError::Offline { url: got }) => assert_eq!(got, url),
        other => panic!("expected Offline, got {other:?}"),
    }
    assert!(started.elapsed() < Duration::from_secs(3));
    let final_path = dir.path().join("offline.bin");
    assert!(!final_path.exists());
    assert!(!part_path(&final_path).exists());
}

#[test]
fn a_token_cancelled_before_fetch_touches_neither_the_server_nor_the_disk() {
    let body = payload(10_000);
    let body_for_server = body.clone();
    let (base, count) = spawn(move |_n, mut stream| {
        let _ = parse_request(&stream);
        respond_full(&mut stream, &body_for_server);
    });
    let dir = scratch_dir("cancel-early");
    let spec = spec_for(&format!("{base}/early"), "nested/early.bin", &body, None);
    let fetcher = Fetcher::with_timings(dir.path(), fast_timings(Duration::from_millis(300)));
    let cancel = CancelToken::new();
    cancel.cancel();

    assert!(matches!(
        fetcher.fetch(&spec, &cancel, |_| {}),
        Err(FetchError::Cancelled)
    ));
    assert_eq!(count.load(Ordering::SeqCst), 0);
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 0);
}

#[test]
fn a_cancel_during_the_retry_backoff_returns_promptly() {
    let body = payload(200_000);
    let cut = 30_000usize;
    let body_for_server = body.clone();
    let (base, count) = spawn(move |_n, mut stream| {
        let _ = parse_request(&stream);
        write_head(
            &mut stream,
            "HTTP/1.1 200 OK",
            &[
                format!("Content-Length: {}", body_for_server.len()),
                "Connection: close".to_string(),
            ],
        );
        let _ = stream.write_all(&body_for_server[..cut]);
        let _ = stream.flush();
    });
    let dir = scratch_dir("cancel-backoff");
    let final_path = dir.path().join("backoff.bin");
    let spec = spec_for(&format!("{base}/backoff"), "backoff.bin", &body, None);
    let fetcher = Fetcher::with_timings(
        dir.path(),
        Timings {
            backoff: [Duration::from_secs(5); 3],
            ..fast_timings(Duration::from_secs(3))
        },
    );
    let cancel = CancelToken::new();
    let cancel_in_thread = cancel.clone();
    let part = part_path(&final_path);
    let canceller = thread::spawn(move || {
        let reached = wait_until(Duration::from_secs(5), || file_len(&part) == cut as u64);
        thread::sleep(Duration::from_millis(200));
        cancel_in_thread.cancel();
        (reached, Instant::now())
    });

    let started = Instant::now();
    let result = fetcher.fetch(&spec, &cancel, |_| {});
    let returned = Instant::now();
    let (reached, cancelled_at) = canceller.join().unwrap();

    assert!(reached, "the cut body never reached the .part");
    assert!(matches!(result, Err(FetchError::Cancelled)), "{result:?}");
    assert!(
        returned.duration_since(cancelled_at) < Duration::from_millis(500),
        "cancel took {:?}",
        returned.duration_since(cancelled_at)
    );
    assert!(returned.duration_since(started) < Duration::from_secs(5));
    assert_eq!(
        count.load(Ordering::SeqCst),
        1,
        "no retry ran after the cancel"
    );
    assert!(!final_path.exists());
    assert_eq!(fs::read(part_path(&final_path)).unwrap(), body[..cut]);
}
