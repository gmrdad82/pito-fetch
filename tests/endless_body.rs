mod support;

use std::io::Write;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use fetch::{CancelToken, FetchSpec, Fetcher, Timings};
use support::{parse_request, payload, spawn, write_head};

#[test]
fn endless_body_without_content_length_ends_in_typed_error_in_bounded_time() {
    let (base, _connections) = spawn(move |_n, mut stream| {
        let _req = parse_request(&stream);
        write_head(
            &mut stream,
            "HTTP/1.1 200 OK",
            &["Connection: close".to_string()],
        );
        let chunk = payload(65_536);
        loop {
            if stream.write_all(&chunk).is_err() {
                return;
            }
            let _ = stream.flush();
            thread::sleep(Duration::from_millis(5));
        }
    });

    let dir = support::scratch_dir("endless-body");
    let fetcher = Fetcher::with_timings(
        dir.path(),
        Timings {
            connect_timeout: Duration::from_secs(1),
            stall_timeout: Duration::from_millis(300),
            poll_interval: Duration::from_millis(30),
            backoff: [
                Duration::from_millis(10),
                Duration::from_millis(10),
                Duration::from_millis(10),
            ],
        },
    );
    let spec = FetchSpec {
        url: Box::leak(format!("{base}/endless").into_boxed_str()),
        dest_rel: "endless.bin",
        sha256: "0000000000000000000000000000000000000000000000000000000000000000",
        size_hint: Some(10_000),
    };

    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let result = fetcher.fetch(&spec, &CancelToken::new(), |_| {});
        let _ = tx.send(result.map(|_| ()).map_err(|e| format!("{e:?}")));
    });

    let outcome = rx.recv_timeout(Duration::from_secs(5));
    let result = outcome
        .expect("fetch must return in bounded time when the body never ends and dwarfs size_hint");
    assert!(result.is_err());
    assert!(!dir.path().join("endless.bin").exists());
}
