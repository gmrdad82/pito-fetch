mod support;

use std::fs;
use std::io::Write;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use fetch::{CancelToken, FetchError, FetchProgress, Fetcher};
use support::{
    fast_timings, parse_request, part_path, payload, respond_full, respond_status, scratch_dir,
    spawn, spec_for, write_head,
};

fn http_error_after_exactly_one_request(status_line: &'static str, status: u16) {
    let (base, count) = spawn(move |_n, mut stream| {
        let _ = parse_request(&stream);
        respond_status(&mut stream, status_line);
    });
    let dir = scratch_dir("http-status");
    let url = format!("{base}/missing");
    let spec = spec_for(&url, "missing.bin", &payload(1_000), None);
    let fetcher = Fetcher::with_timings(dir.path(), fast_timings(Duration::from_secs(2)));

    match fetcher.fetch(&spec, &CancelToken::new(), |_| {}) {
        Err(FetchError::Http {
            status: got,
            url: got_url,
        }) => {
            assert_eq!(got, status);
            assert_eq!(got_url, url);
        }
        other => panic!("expected Http {status}, got {other:?}"),
    }
    assert_eq!(
        count.load(Ordering::SeqCst),
        1,
        "HTTP errors are not retried"
    );
    let final_path = dir.path().join("missing.bin");
    assert!(!final_path.exists());
    assert!(!part_path(&final_path).exists());
}

#[test]
fn not_found_returns_http_404_after_one_request_and_leaves_nothing() {
    http_error_after_exactly_one_request("HTTP/1.1 404 Not Found", 404);
}

#[test]
fn server_error_returns_http_500_after_one_request_and_leaves_nothing() {
    http_error_after_exactly_one_request("HTTP/1.1 500 Internal Server Error", 500);
}

fn restart_story(label: &str, range_reply: fn(&mut std::net::TcpStream, &[u8])) {
    let body = payload(200_000);
    let body_for_server = body.clone();
    let ranges = Arc::new(Mutex::new(Vec::new()));
    let ranges_in_server = ranges.clone();
    let (base, _count) = spawn(move |_n, mut stream| {
        let req = parse_request(&stream);
        ranges_in_server.lock().unwrap().push(req.range_start);
        if req.range_start.is_some() {
            range_reply(&mut stream, &body_for_server);
        } else {
            respond_full(&mut stream, &body_for_server);
        }
    });
    let dir = scratch_dir(label);
    let final_path = dir.path().join("restart.bin");
    fs::write(part_path(&final_path), vec![0xEEu8; 5_000]).unwrap();
    let spec = spec_for(&format!("{base}/restart"), "restart.bin", &body, None);
    let fetcher = Fetcher::with_timings(dir.path(), fast_timings(Duration::from_secs(2)));

    let path = fetcher
        .fetch(&spec, &CancelToken::new(), |_| {})
        .expect("a refused resume restarts from zero and lands");
    assert_eq!(path, final_path);
    assert_eq!(fs::read(&path).unwrap(), body);
    assert!(!part_path(&final_path).exists());
    assert_eq!(*ranges.lock().unwrap(), vec![Some(5_000), None]);
}

#[test]
fn range_answered_with_416_restarts_from_zero_and_lands_verified() {
    restart_story("http-416", |stream, _body| {
        respond_status(stream, "HTTP/1.1 416 Range Not Satisfiable");
    });
}

#[test]
fn range_answered_with_200_whole_body_restarts_and_lands_verified() {
    restart_story("http-200-for-range", respond_full);
}

#[test]
fn resume_206_reports_progress_totals_from_content_range() {
    let body = payload(300_000);
    let len = body.len();
    let prefix = 80_000usize;
    let body_for_server = body.clone();
    let (base, _count) = spawn(move |_n, mut stream| {
        let req = parse_request(&stream);
        let start = req.range_start.expect("the resume asks for a range") as usize;
        write_head(
            &mut stream,
            "HTTP/1.1 206 Partial Content",
            &[
                format!("Content-Range: bytes {}-{}/{}", start, len - 1, len),
                "Connection: close".to_string(),
            ],
        );
        let _ = stream.write_all(&body_for_server[start..]);
        let _ = stream.flush();
    });
    let dir = scratch_dir("http-206-progress");
    let final_path = dir.path().join("resumed.bin");
    fs::write(part_path(&final_path), &body[..prefix]).unwrap();
    let spec = spec_for(&format!("{base}/resumed"), "resumed.bin", &body, None);
    let fetcher = Fetcher::with_timings(dir.path(), fast_timings(Duration::from_secs(2)));

    let mut events: Vec<FetchProgress> = Vec::new();
    let path = fetcher
        .fetch(&spec, &CancelToken::new(), |p| events.push(p))
        .expect("the 206 resume lands");
    assert_eq!(fs::read(&path).unwrap(), body);
    assert!(!events.is_empty());
    assert!(events.iter().all(|p| p.total == Some(len as u64)));
    assert!(events.iter().all(|p| p.received > prefix as u64));
    assert_eq!(events.last().unwrap().received, len as u64);
}
