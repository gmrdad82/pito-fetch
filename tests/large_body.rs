mod support;

use std::fs;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use fetch::{CancelToken, FetchError, FetchProgress, Fetcher, sha256_hex};
use support::{
    fast_timings, parse_request, part_path, payload, respond_full, respond_range, scratch_dir,
    spawn, spec_for,
};

const MIB: usize = 1024 * 1024;

#[test]
fn a_body_of_three_and_a_half_mib_lands_byte_exact_with_monotonic_progress() {
    let body = payload(3 * MIB + MIB / 2);
    let len = body.len() as u64;
    let body_for_server = body.clone();
    let (base, count) = spawn(move |_n, mut stream| {
        let _ = parse_request(&stream);
        respond_full(&mut stream, &body_for_server);
    });
    let dir = scratch_dir("large-body");
    let spec = spec_for(&format!("{base}/large"), "large.bin", &body, Some(len));
    let fetcher = Fetcher::with_timings(dir.path(), fast_timings(Duration::from_secs(3)));

    let mut events: Vec<FetchProgress> = Vec::new();
    let path = fetcher
        .fetch(&spec, &CancelToken::new(), |p| events.push(p))
        .expect("the large body lands");
    let on_disk = fs::read(&path).unwrap();
    assert_eq!(on_disk.len(), body.len());
    assert_eq!(sha256_hex(&on_disk), spec.sha256);
    assert_eq!(on_disk, body);
    assert!(!part_path(&path).exists());
    assert_eq!(count.load(Ordering::SeqCst), 1);

    assert!(!events.is_empty());
    assert!(events.windows(2).all(|w| w[0].received <= w[1].received));
    assert!(events.iter().all(|p| p.total == Some(len)));
    assert_eq!(events.last().unwrap().received, len);
}

#[test]
fn resume_from_a_part_cut_mid_chunk_sends_the_exact_range_and_lands_verified() {
    let body = payload(3 * MIB + MIB / 2);
    let cut = MIB + MIB / 2;
    let body_for_server = body.clone();
    let ranges = Arc::new(Mutex::new(Vec::new()));
    let ranges_in_server = ranges.clone();
    let (base, _count) = spawn(move |_n, mut stream| {
        let req = parse_request(&stream);
        ranges_in_server.lock().unwrap().push(req.range.clone());
        match req.range_start {
            Some(start) => respond_range(&mut stream, &body_for_server, start),
            None => respond_full(&mut stream, &body_for_server),
        }
    });
    let dir = scratch_dir("large-resume");
    let final_path = dir.path().join("large.bin");
    fs::write(part_path(&final_path), &body[..cut]).unwrap();
    let spec = spec_for(
        &format!("{base}/large"),
        "large.bin",
        &body,
        Some(body.len() as u64),
    );
    let fetcher = Fetcher::with_timings(dir.path(), fast_timings(Duration::from_secs(3)));

    let mut events: Vec<FetchProgress> = Vec::new();
    let path = fetcher
        .fetch(&spec, &CancelToken::new(), |p| events.push(p))
        .expect("the resume lands");
    assert_eq!(path, final_path);
    assert_eq!(fs::read(&path).unwrap(), body);
    assert!(!part_path(&final_path).exists());
    assert_eq!(
        *ranges.lock().unwrap(),
        vec![Some("bytes=1572864-".to_string())]
    );
    assert!(!events.is_empty());
    assert!(events.iter().all(|p| p.received > cut as u64));
    assert!(events.iter().all(|p| p.total == Some(body.len() as u64)));
    assert_eq!(events.last().unwrap().received, body.len() as u64);
}

#[test]
fn a_size_hint_far_below_the_body_caps_the_read_and_ends_in_checksum_mismatch() {
    let body = payload(3 * MIB + MIB / 2);
    let body_for_server = body.clone();
    let (base, count) = spawn(move |_n, mut stream| {
        let _ = parse_request(&stream);
        respond_full(&mut stream, &body_for_server);
    });
    let dir = scratch_dir("large-capped");
    let spec = spec_for(&format!("{base}/capped"), "capped.bin", &body, Some(1_000));
    let fetcher = Fetcher::with_timings(dir.path(), fast_timings(Duration::from_secs(3)));

    let mut events = Vec::new();
    let result = fetcher.fetch(&spec, &CancelToken::new(), |p| events.push(p.received));
    match result {
        Err(FetchError::ChecksumMismatch { expected, got }) => {
            assert_eq!(expected, spec.sha256);
            assert_eq!(
                got,
                sha256_hex(&body[..MIB]),
                "the read stops at the 1 MiB floor cap"
            );
        }
        other => panic!("expected ChecksumMismatch, got {other:?}"),
    }
    assert_eq!(events.last().copied(), Some(MIB as u64));
    assert!(events.iter().all(|&r| r <= MIB as u64));
    assert_eq!(count.load(Ordering::SeqCst), 1);
    let final_path = dir.path().join("capped.bin");
    assert!(!final_path.exists());
    assert!(!part_path(&final_path).exists());
}
