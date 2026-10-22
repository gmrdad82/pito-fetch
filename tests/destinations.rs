mod support;

use std::fs;
use std::io::ErrorKind;
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::time::Duration;

use fetch::{CancelToken, FetchError, Fetcher, Request, sha256_hex};
use support::{
    fast_timings, parse_request, part_path, payload, respond_full, scratch_dir, spawn, spec_for,
};

#[test]
fn a_dest_rel_with_a_current_dir_segment_lands_at_the_normalised_path() {
    let body = payload(40_000);
    let body_for_server = body.clone();
    let (base, count) = spawn(move |_n, mut stream| {
        let _ = parse_request(&stream);
        respond_full(&mut stream, &body_for_server);
    });
    let dir = scratch_dir("dest-curdir");
    let request = Request {
        url: format!("{base}/curdir"),
        dest_rel: "a/./b/c.bin".to_string(),
        sha256: sha256_hex(&body),
        size_hint: Some(body.len() as u64),
    };
    let fetcher = Fetcher::with_timings(dir.path(), fast_timings(Duration::from_secs(2)));

    let path = fetcher
        .fetch_request(&request, &CancelToken::new(), |_| {})
        .expect("a ./ segment is a plain relative path");
    let expected = dir.path().join("a").join("b").join("c.bin");
    assert_eq!(path, expected);
    assert_eq!(fs::read(&expected).unwrap(), body);
    assert!(!part_path(&expected).exists());
    assert_eq!(count.load(Ordering::SeqCst), 1);
}

fn refused_before_any_request(dest_rel: &'static str) {
    let (base, count) = spawn(|_n, mut stream| {
        let _ = parse_request(&stream);
        respond_full(&mut stream, b"never");
    });
    let dir = scratch_dir("dest-empty");
    let spec = spec_for(&format!("{base}/empty"), dest_rel, b"never", None);
    let fetcher = Fetcher::with_timings(dir.path(), fast_timings(Duration::from_secs(2)));

    match fetcher.fetch(&spec, &CancelToken::new(), |_| {}) {
        Err(FetchError::Io { path, source }) => {
            assert_eq!(source.kind(), ErrorKind::InvalidInput);
            assert_eq!(path, PathBuf::from(dest_rel));
        }
        other => panic!("expected Io InvalidInput for {dest_rel:?}, got {other:?}"),
    }
    assert_eq!(count.load(Ordering::SeqCst), 0);
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 0);
}

#[test]
fn an_empty_dest_rel_is_refused_before_any_request() {
    refused_before_any_request("");
}

#[test]
fn a_dest_rel_of_only_the_current_dir_is_refused_before_any_request() {
    refused_before_any_request(".");
}

#[test]
fn a_dest_rel_whose_parent_is_a_regular_file_returns_io_naming_that_parent() {
    let (base, count) = spawn(|_n, mut stream| {
        let _ = parse_request(&stream);
        respond_full(&mut stream, b"never");
    });
    let dir = scratch_dir("dest-blocked");
    let blocker = dir.path().join("blocker");
    fs::write(&blocker, b"a regular file").unwrap();
    let spec = spec_for(&format!("{base}/blocked"), "blocker/c.bin", b"never", None);
    let fetcher = Fetcher::with_timings(dir.path(), fast_timings(Duration::from_secs(2)));

    match fetcher.fetch(&spec, &CancelToken::new(), |_| {}) {
        Err(FetchError::Io { path, .. }) => assert_eq!(path, blocker),
        other => panic!("expected Io naming the parent, got {other:?}"),
    }
    assert_eq!(count.load(Ordering::SeqCst), 0);
    assert_eq!(fs::read(&blocker).unwrap(), b"a regular file");
}
