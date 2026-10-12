mod support;

use std::io::Write;
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use fetch::{CancelToken, FetchSpec, Fetcher, Timings, sha256_hex};
use support::{parse_request, payload, write_head};

fn spawn_threaded<F>(handler: F) -> String
where
    F: Fn(usize, TcpStream) + Send + Sync + 'static,
{
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let count = Arc::new(AtomicUsize::new(0));
    let handler = Arc::new(handler);
    thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(stream) = stream else { break };
            let n = count.fetch_add(1, Ordering::SeqCst);
            let h = handler.clone();
            thread::spawn(move || h(n, stream));
        }
    });
    format!("http://127.0.0.1:{port}/asset")
}

fn timings(stall: Duration) -> Timings {
    Timings {
        connect_timeout: Duration::from_secs(3),
        stall_timeout: stall,
        poll_interval: Duration::from_millis(30),
        backoff: [
            Duration::from_millis(10),
            Duration::from_millis(10),
            Duration::from_millis(10),
        ],
    }
}

#[test]
fn overlapping_fetch_of_same_spec_never_leaves_unverified_bytes_at_final_path() {
    let body = payload(400_000);
    let cut = 100_000usize;
    let len = body.len();
    let sha = sha256_hex(&body);
    let body_for_server = body.clone();
    let release = Arc::new(AtomicBool::new(false));
    let release_for_server = release.clone();
    let base = spawn_threaded(move |_n, mut stream| {
        let req = parse_request(&stream);
        match req.range_start {
            None => {
                write_head(
                    &mut stream,
                    "HTTP/1.1 200 OK",
                    &[
                        format!("Content-Length: {len}"),
                        "Connection: close".to_string(),
                    ],
                );
                let _ = stream.write_all(&body_for_server[..cut]);
                let _ = stream.flush();
                let waited = Instant::now();
                while !release_for_server.load(Ordering::SeqCst)
                    && waited.elapsed() < Duration::from_secs(8)
                {
                    thread::sleep(Duration::from_millis(10));
                }
                let divergent = vec![0x5Au8; len - cut];
                let _ = stream.write_all(&divergent);
            }
            Some(start) => {
                let start = start as usize;
                let remaining = &body_for_server[start..];
                write_head(
                    &mut stream,
                    "HTTP/1.1 206 Partial Content",
                    &[
                        format!("Content-Range: bytes {}-{}/{}", start, len - 1, len),
                        format!("Content-Length: {}", remaining.len()),
                        "Connection: close".to_string(),
                    ],
                );
                let _ = stream.write_all(remaining);
            }
        }
    });

    let dir = support::scratch_dir("overlap");
    let dest_root = dir.path().to_path_buf();
    let final_path = dest_root.join("artifact.bin");
    let part_path = dest_root.join("artifact.bin.part");
    let spec = FetchSpec {
        url: Box::leak(format!("{base}/artifact").into_boxed_str()),
        dest_rel: "artifact.bin",
        sha256: Box::leak(sha.into_boxed_str()),
        size_hint: Some(len as u64),
    };

    let stalled_root = dest_root.clone();
    let stalled_spec = spec;
    let stalled = thread::spawn(move || {
        let fetcher = Fetcher::with_timings(stalled_root, timings(Duration::from_secs(10)));
        fetcher.fetch(&stalled_spec, &CancelToken::new(), |_| {})
    });

    let waited = Instant::now();
    loop {
        let part_len = std::fs::metadata(&part_path).map(|m| m.len()).unwrap_or(0);
        if part_len == cut as u64 {
            break;
        }
        assert!(
            waited.elapsed() < Duration::from_secs(5),
            "stalled fetch never reached the staged offset, part at {part_len}"
        );
        thread::sleep(Duration::from_millis(5));
    }
    thread::sleep(Duration::from_millis(50));

    let retry_fetcher = Fetcher::with_timings(dest_root, timings(Duration::from_secs(3)));
    let retry_path = retry_fetcher
        .fetch(&spec, &CancelToken::new(), |_| {})
        .expect("retry fetch must succeed via resume");
    assert_eq!(std::fs::read(&retry_path).unwrap(), body);

    release.store(true, Ordering::SeqCst);
    let _ = stalled.join().unwrap();

    let on_disk = std::fs::read(&final_path).unwrap();
    let first_divergence = on_disk
        .iter()
        .zip(body.iter())
        .position(|(a, b)| a != b)
        .or_else(|| (on_disk.len() != body.len()).then_some(body.len().min(on_disk.len())));
    assert!(
        first_divergence.is_none(),
        "final path holds unverified bytes: {} on disk vs {} expected, first divergence at byte {}",
        on_disk.len(),
        body.len(),
        first_divergence.unwrap()
    );
}
