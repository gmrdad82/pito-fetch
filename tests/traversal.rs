mod support;

use std::io::Write;
use std::time::Duration;

use fetch::{CancelToken, FetchSpec, Fetcher, Timings, sha256_hex};
use support::{parse_request, payload, spawn, write_head};

fn fast_timings() -> Timings {
    Timings {
        connect_timeout: Duration::from_secs(3),
        stall_timeout: Duration::from_secs(3),
        poll_interval: Duration::from_millis(30),
        backoff: [
            Duration::from_millis(10),
            Duration::from_millis(10),
            Duration::from_millis(10),
        ],
    }
}

fn serve_fixed_body(body: Vec<u8>) -> String {
    let (base, _connections) = spawn(move |_n, mut stream| {
        let _req = parse_request(&stream);
        write_head(
            &mut stream,
            "HTTP/1.1 200 OK",
            &[
                format!("Content-Length: {}", body.len()),
                "Connection: close".to_string(),
            ],
        );
        let _ = stream.write_all(&body);
        let _ = stream.flush();
    });
    base
}

#[test]
fn dest_rel_traversal_cannot_escape_dest_root() {
    let body = payload(1_000);
    let sha = sha256_hex(&body);
    let base = serve_fixed_body(body);

    let outer = support::scratch_dir("traversal-relative");
    let root = outer.path().join("root");
    std::fs::create_dir_all(&root).unwrap();
    let fetcher = Fetcher::with_timings(&root, fast_timings());
    let spec = FetchSpec {
        url: Box::leak(format!("{base}/escape").into_boxed_str()),
        dest_rel: "../escaped.bin",
        sha256: Box::leak(sha.into_boxed_str()),
        size_hint: None,
    };

    let result = fetcher.fetch(&spec, &CancelToken::new(), |_| {});
    assert!(
        !outer.path().join("escaped.bin").exists(),
        "dest_rel with ../ wrote outside dest_root"
    );
    if let Ok(path) = result {
        assert!(
            path.starts_with(&root),
            "returned path {} lies outside dest_root",
            path.display()
        );
    }
}

#[test]
fn absolute_dest_rel_cannot_replace_dest_root() {
    let body = payload(1_000);
    let sha = sha256_hex(&body);
    let base = serve_fixed_body(body);

    let outer = support::scratch_dir("traversal-absolute");
    let root = outer.path().join("root");
    let elsewhere = outer.path().join("elsewhere");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::create_dir_all(&elsewhere).unwrap();
    let fetcher = Fetcher::with_timings(&root, fast_timings());
    let abs_target = elsewhere.join("hijack.bin");
    let spec = FetchSpec {
        url: Box::leak(format!("{base}/abs").into_boxed_str()),
        dest_rel: Box::leak(abs_target.to_str().unwrap().to_string().into_boxed_str()),
        sha256: Box::leak(sha.into_boxed_str()),
        size_hint: None,
    };

    let result = fetcher.fetch(&spec, &CancelToken::new(), |_| {});
    assert!(
        !abs_target.exists(),
        "absolute dest_rel wrote outside dest_root"
    );
    if let Ok(path) = result {
        assert!(path.starts_with(&root));
    }
}
