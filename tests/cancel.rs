mod support;

use std::io::Write;
use std::thread;
use std::time::{Duration, Instant};

use fetch::{CancelToken, FetchError, FetchSpec, Fetcher, Timings, sha256_hex};
use support::{parse_request, payload, spawn, write_chunks_paced, write_head};

#[test]
fn cancel_mid_stream_tears_down_fast_and_keeps_the_part_file() {
    let body = payload(500_000);
    let expected_sha = sha256_hex(&body);
    let body_for_server = body.clone();

    let (base, _connections) = spawn(move |_n, mut stream| {
        let _req = parse_request(&stream);
        write_head(
            &mut stream,
            "HTTP/1.1 200 OK",
            &[
                format!("Content-Length: {}", body_for_server.len()),
                "Connection: close".to_string(),
            ],
        );
        write_chunks_paced(
            &mut stream,
            &body_for_server,
            2_048,
            Duration::from_millis(15),
        );
        let _ = stream.flush();
    });

    let dir = support::scratch_dir("cancel");
    let fetcher = Fetcher::with_timings(
        dir.path(),
        Timings {
            connect_timeout: Duration::from_secs(3),
            stall_timeout: Duration::from_secs(3),
            poll_interval: Duration::from_millis(30),
            backoff: [
                Duration::from_millis(10),
                Duration::from_millis(10),
                Duration::from_millis(10),
            ],
        },
    );
    let spec = FetchSpec {
        url: Box::leak(format!("{base}/cancel-me").into_boxed_str()),
        dest_rel: "cancel-me.bin",
        sha256: Box::leak(expected_sha.into_boxed_str()),
        size_hint: Some(body.len() as u64),
    };

    let cancel = CancelToken::new();
    let cancel_for_thread = cancel.clone();
    thread::spawn(move || {
        thread::sleep(Duration::from_millis(120));
        cancel_for_thread.cancel();
    });

    let started = Instant::now();
    let result = fetcher.fetch(&spec, &cancel, |_| {});
    let elapsed = started.elapsed();

    assert!(matches!(result.unwrap_err(), FetchError::Cancelled));
    assert!(
        elapsed < Duration::from_millis(500),
        "cancel took {elapsed:?}"
    );

    let part_path = dir.path().join("cancel-me.bin.part");
    let on_disk = std::fs::read(&part_path).expect(".part must survive a cancel for resume");
    assert!(!on_disk.is_empty());
    assert!(on_disk.len() < body.len());
    assert_eq!(on_disk, body[..on_disk.len()]);
}
