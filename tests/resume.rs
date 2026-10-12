mod support;

use std::io::Write;
use std::sync::atomic::Ordering;
use std::time::Duration;

use fetch::{CancelToken, FetchSpec, Fetcher, Timings, sha256_hex};
use support::{parse_request, payload, spawn, write_head};

#[test]
fn kill_mid_download_then_resume_completes_with_verified_checksum() {
    let body = payload(300_000);
    let cut_at = 120_000usize;
    let expected_sha256 = sha256_hex(&body);
    let body_for_server = body.clone();

    let (base, connections) = spawn(move |n, mut stream| {
        let req = parse_request(&stream);
        assert_eq!(req.method, "GET");
        if n == 0 {
            assert!(req.range_start.is_none());
            write_head(
                &mut stream,
                "HTTP/1.1 200 OK",
                &[
                    format!("Content-Length: {}", body_for_server.len()),
                    "Connection: close".to_string(),
                ],
            );
            let _ = stream.write_all(&body_for_server[..cut_at]);
            let _ = stream.flush();
        } else {
            let start = req.range_start.unwrap_or(0) as usize;
            let remaining = &body_for_server[start..];
            write_head(
                &mut stream,
                "HTTP/1.1 206 Partial Content",
                &[
                    format!(
                        "Content-Range: bytes {}-{}/{}",
                        start,
                        body_for_server.len() - 1,
                        body_for_server.len()
                    ),
                    format!("Content-Length: {}", remaining.len()),
                    "Connection: close".to_string(),
                ],
            );
            let _ = stream.write_all(remaining);
            let _ = stream.flush();
        }
    });

    let dir = support::scratch_dir("resume");
    let fetcher = Fetcher::with_timings(
        dir.path(),
        Timings {
            connect_timeout: Duration::from_secs(5),
            stall_timeout: Duration::from_secs(5),
            poll_interval: Duration::from_millis(50),
            backoff: [
                Duration::from_millis(20),
                Duration::from_millis(20),
                Duration::from_millis(20),
            ],
        },
    );
    let spec = FetchSpec {
        url: Box::leak(format!("{base}/kill-resume").into_boxed_str()),
        dest_rel: "kill-resume.bin",
        sha256: Box::leak(expected_sha256.into_boxed_str()),
        size_hint: Some(body.len() as u64),
    };

    let result = fetcher.fetch(&spec, &CancelToken::new(), |_| {});
    let path = result.expect("fetch should recover via resume");
    let on_disk = std::fs::read(&path).unwrap();
    assert_eq!(on_disk, body);
    assert_eq!(connections.load(Ordering::SeqCst), 2);
    let part_path = std::path::PathBuf::from(format!("{}.part", path.display()));
    assert!(!part_path.exists());
}
