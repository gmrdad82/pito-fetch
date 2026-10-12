mod support;

use std::io::Write;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use fetch::{CancelToken, FetchError, FetchSpec, Fetcher, Timings, sha256_hex};
use support::{parse_request, payload, spawn, write_head};

fn fast_timings() -> Timings {
    Timings {
        connect_timeout: Duration::from_secs(3),
        stall_timeout: Duration::from_millis(300),
        poll_interval: Duration::from_millis(30),
        backoff: [
            Duration::from_millis(10),
            Duration::from_millis(10),
            Duration::from_millis(10),
        ],
    }
}

#[test]
fn wrong_content_length_never_leaves_an_unverified_final_file() {
    let body = payload(20_000);
    let sha = sha256_hex(&body);
    let body_for_server = body.clone();

    let (base, connections) = spawn(move |_n, mut stream| {
        let _req = parse_request(&stream);
        write_head(
            &mut stream,
            "HTTP/1.1 200 OK",
            &[
                format!("Content-Length: {}", body_for_server.len() + 5_000),
                "Connection: close".to_string(),
            ],
        );
        let _ = stream.write_all(&body_for_server);
        let _ = stream.flush();
    });

    let dir = support::scratch_dir("hostile-corrupt");
    let fetcher = Fetcher::with_timings(dir.path(), fast_timings());
    let spec = FetchSpec {
        url: Box::leak(format!("{base}/wrong-length").into_boxed_str()),
        dest_rel: "wrong-length.bin",
        sha256: Box::leak(sha.into_boxed_str()),
        size_hint: None,
    };

    let started = Instant::now();
    let result = fetcher.fetch(&spec, &CancelToken::new(), |_| {});
    assert!(result.is_err());
    assert!(matches!(result.unwrap_err(), FetchError::Io { .. }));
    assert!(started.elapsed() < Duration::from_secs(5));
    assert!(!dir.path().join("wrong-length.bin").exists());
    assert!(connections.load(Ordering::SeqCst) >= 4);
}

#[test]
fn corrupted_bytes_are_caught_by_checksum_and_part_is_deleted() {
    let real_body = payload(20_000);
    let corrupt_body = payload(20_001);
    let expected_sha = sha256_hex(&real_body);
    let corrupt_for_server = corrupt_body.clone();

    let (base, _connections) = spawn(move |_n, mut stream| {
        let _req = parse_request(&stream);
        write_head(
            &mut stream,
            "HTTP/1.1 200 OK",
            &[
                format!("Content-Length: {}", corrupt_for_server.len()),
                "Connection: close".to_string(),
            ],
        );
        let _ = stream.write_all(&corrupt_for_server);
        let _ = stream.flush();
    });

    let dir = support::scratch_dir("hostile-length");
    let fetcher = Fetcher::with_timings(dir.path(), fast_timings());
    let spec = FetchSpec {
        url: Box::leak(format!("{base}/corrupt").into_boxed_str()),
        dest_rel: "corrupt.bin",
        sha256: Box::leak(expected_sha.clone().into_boxed_str()),
        size_hint: None,
    };

    let result = fetcher.fetch(&spec, &CancelToken::new(), |_| {});
    match result.unwrap_err() {
        FetchError::ChecksumMismatch { expected, got } => {
            assert_eq!(expected, expected_sha);
            assert_ne!(got, expected);
        }
        other => panic!("expected ChecksumMismatch, got {other:?}"),
    }
    assert!(!dir.path().join("corrupt.bin").exists());
    assert!(!dir.path().join("corrupt.bin.part").exists());
}

#[test]
fn redirect_loop_is_bounded_and_returns_quickly() {
    let (base, _connections) = spawn(move |_n, mut stream| {
        let req = parse_request(&stream);
        let location = req.path.clone();
        write_head(
            &mut stream,
            "HTTP/1.1 301 Moved Permanently",
            &[
                format!("Location: {location}"),
                "Content-Length: 0".to_string(),
                "Connection: close".to_string(),
            ],
        );
    });

    let dir = support::scratch_dir("hostile-silent");
    let fetcher = Fetcher::with_timings(dir.path(), fast_timings());
    let spec = FetchSpec {
        url: Box::leak(format!("{base}/loop").into_boxed_str()),
        dest_rel: "loop.bin",
        sha256: "0000000000000000000000000000000000000000000000000000000000000000",
        size_hint: None,
    };

    let started = Instant::now();
    let result = fetcher.fetch(&spec, &CancelToken::new(), |_| {});
    assert!(result.is_err());
    assert!(started.elapsed() < Duration::from_secs(10));
    assert!(!dir.path().join("loop.bin").exists());
}
