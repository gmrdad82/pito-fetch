mod support;

use std::io::Write;
use std::thread;
use std::time::Duration;

use fetch::{CancelToken, FetchSpec, Fetcher, Timings, sha256_hex};
use support::{parse_request, payload, spawn, write_chunks_paced, write_head};

#[test]
fn final_path_is_never_observed_partially_written() {
    let body = payload(200_000);
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
            4_096,
            Duration::from_millis(10),
        );
        let _ = stream.flush();
    });

    let dir = support::scratch_dir("atomicity");
    let final_path = dir.path().join("atomic.bin");
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
        url: Box::leak(format!("{base}/atomic").into_boxed_str()),
        dest_rel: "atomic.bin",
        sha256: Box::leak(expected_sha.into_boxed_str()),
        size_hint: Some(body.len() as u64),
    };

    let watch_path = final_path.clone();
    let watch_body = body.clone();
    let watcher = thread::spawn(move || {
        let mut observations = 0usize;
        loop {
            if let Ok(bytes) = std::fs::read(&watch_path) {
                observations += 1;
                assert_eq!(bytes, watch_body, "final path observed with wrong bytes");
            }
            thread::sleep(Duration::from_millis(2));
            if observations > 3 {
                break;
            }
        }
    });

    let result = fetcher.fetch(&spec, &CancelToken::new(), |_| {});
    let path = result.expect("plain download should succeed");
    assert_eq!(path, final_path);
    assert_eq!(std::fs::read(&path).unwrap(), body);

    let mut settle_wait = 0;
    while !final_path.exists() && settle_wait < 50 {
        thread::sleep(Duration::from_millis(5));
        settle_wait += 1;
    }
    let _ = watcher.join();
}
