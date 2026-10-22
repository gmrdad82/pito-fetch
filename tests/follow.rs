mod support;

use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use fetch::{CancelToken, Fetcher};
use support::{
    fast_timings, file_len, parse_request, part_path, payload, respond_range, scratch_dir,
    spawn_threaded, spec_for, wait_until, write_head,
};

const WAIT: Duration = Duration::from_secs(8);

struct Shared {
    root: PathBuf,
    final_path: PathBuf,
    part: PathBuf,
}

fn shared(label: &str) -> (tempfile::TempDir, Shared) {
    let dir = scratch_dir(label);
    let root = dir.path().to_path_buf();
    let final_path = root.join("shared.bin");
    let part = part_path(&final_path);
    (
        dir,
        Shared {
            root,
            final_path,
            part,
        },
    )
}

fn write_full_head(stream: &mut std::net::TcpStream, len: usize) {
    write_head(
        stream,
        "HTTP/1.1 200 OK",
        &[
            format!("Content-Length: {len}"),
            "Connection: close".to_string(),
        ],
    );
}

#[test]
fn a_part_that_grows_while_followed_is_taken_over_only_after_it_goes_quiet() {
    let body = payload(400_000);
    let first = 100_000usize;
    let steps = [160_000usize, 200_000, 240_000];
    let stall = Duration::from_millis(1_500);
    let spacing = Duration::from_millis(300);
    let (_dir, at) = shared("follow-takeover");
    let step = Arc::new(AtomicUsize::new(0));
    let release = Arc::new(AtomicBool::new(false));
    let last_growth = Arc::new(Mutex::new(None::<Instant>));
    let ranges = Arc::new(Mutex::new(Vec::new()));
    let (base, _count) = spawn_threaded({
        let body = body.clone();
        let part = at.part.clone();
        let step = step.clone();
        let release = release.clone();
        let last_growth = last_growth.clone();
        let ranges = ranges.clone();
        move |_n, mut stream| {
            let req = parse_request(&stream);
            ranges
                .lock()
                .unwrap()
                .push((req.range_start, Instant::now()));
            match req.range_start {
                None => {
                    write_full_head(&mut stream, body.len());
                    let _ = stream.write_all(&body[..first]);
                    let _ = stream.flush();
                    let mut written = first;
                    for (k, &end) in steps.iter().enumerate() {
                        wait_until(WAIT, || step.load(Ordering::SeqCst) > k);
                        if k + 1 == steps.len() {
                            *last_growth.lock().unwrap() = Some(Instant::now());
                        }
                        let _ = stream.write_all(&body[written..end]);
                        let _ = stream.flush();
                        written = end;
                    }
                    wait_until(WAIT, || release.load(Ordering::SeqCst));
                    let _ = stream.write_all(&body[written..]);
                    let _ = stream.flush();
                }
                Some(start) if start == first as u64 => {
                    let rest = &body[first..];
                    write_head(
                        &mut stream,
                        "HTTP/1.1 206 Partial Content",
                        &[
                            format!(
                                "Content-Range: bytes {first}-{}/{}",
                                body.len() - 1,
                                body.len()
                            ),
                            format!("Content-Length: {}", rest.len()),
                            "Connection: close".to_string(),
                        ],
                    );
                    let _ = stream.flush();
                    step.store(1, Ordering::SeqCst);
                    wait_until(WAIT, || file_len(&part) == steps[0] as u64);
                    let _ = stream.write_all(rest);
                    let _ = stream.flush();
                    drop(stream);
                    for (k, &len) in steps.iter().enumerate().skip(1) {
                        thread::sleep(spacing);
                        step.store(k + 1, Ordering::SeqCst);
                        wait_until(WAIT, || file_len(&part) == len as u64);
                    }
                }
                Some(start) => respond_range(&mut stream, &body, start),
            }
        }
    });
    let spec = spec_for(
        &format!("{base}/shared"),
        "shared.bin",
        &body,
        Some(body.len() as u64),
    );

    let holder_root = at.root.clone();
    let holder = thread::spawn(move || {
        Fetcher::with_timings(holder_root, fast_timings(Duration::from_secs(10))).fetch(
            &spec,
            &CancelToken::new(),
            |_| {},
        )
    });
    assert!(
        wait_until(WAIT, || file_len(&at.part) == first as u64),
        "the holder never wrote its first bytes"
    );

    let taker = Fetcher::with_timings(&at.root, fast_timings(stall));
    let taken = taker.fetch(&spec, &CancelToken::new(), |_| {});
    release.store(true, Ordering::SeqCst);
    let held = holder.join().unwrap();

    let path = taken.expect("the taker lands the file once the part goes quiet");
    assert_eq!(path, at.final_path);
    assert_eq!(fs::read(&path).unwrap(), body);
    assert!(!at.part.exists());
    let ranges = ranges.lock().unwrap().clone();
    let starts: Vec<Option<u64>> = ranges.iter().map(|(start, _)| *start).collect();
    let grown = *steps.last().unwrap() as u64;
    assert_eq!(starts, vec![None, Some(first as u64), Some(grown)]);
    let last_growth = last_growth
        .lock()
        .unwrap()
        .expect("the part grew to its last step");
    assert!(
        ranges[2].1 >= last_growth + stall,
        "the takeover came {:?} after the last growth, before the {stall:?} quiet",
        ranges[2].1.saturating_duration_since(last_growth)
    );
    assert_eq!(
        held.expect("the holder returns the landed file"),
        at.final_path
    );
    assert_eq!(fs::read(&at.final_path).unwrap(), body);
}

#[test]
fn a_follower_whose_resume_request_races_the_finish_returns_the_landed_file() {
    let body = payload(400_000);
    let first = 100_000usize;
    let (_dir, at) = shared("follow-finished");
    let finish = Arc::new(AtomicBool::new(false));
    let ranges = Arc::new(Mutex::new(Vec::new()));
    let (base, count) = spawn_threaded({
        let body = body.clone();
        let final_path = at.final_path.clone();
        let finish = finish.clone();
        let ranges = ranges.clone();
        move |_n, mut stream| {
            let req = parse_request(&stream);
            ranges.lock().unwrap().push(req.range_start);
            match req.range_start {
                None => {
                    write_full_head(&mut stream, body.len());
                    let _ = stream.write_all(&body[..first]);
                    let _ = stream.flush();
                    wait_until(WAIT, || finish.load(Ordering::SeqCst));
                    let _ = stream.write_all(&body[first..]);
                    let _ = stream.flush();
                }
                Some(start) => {
                    finish.store(true, Ordering::SeqCst);
                    wait_until(WAIT, || final_path.exists());
                    respond_range(&mut stream, &body, start);
                }
            }
        }
    });
    let spec = spec_for(
        &format!("{base}/shared"),
        "shared.bin",
        &body,
        Some(body.len() as u64),
    );

    let leader_root = at.root.clone();
    let leader = thread::spawn(move || {
        Fetcher::with_timings(leader_root, fast_timings(Duration::from_secs(10))).fetch(
            &spec,
            &CancelToken::new(),
            |_| {},
        )
    });
    assert!(
        wait_until(WAIT, || file_len(&at.part) == first as u64),
        "the leader never wrote its first bytes"
    );

    let follower = Fetcher::with_timings(&at.root, fast_timings(Duration::from_secs(3)));
    let followed = follower.fetch(&spec, &CancelToken::new(), |_| {});
    finish.store(true, Ordering::SeqCst);
    let led = leader.join().unwrap();

    let led = led.expect("the leader lands the file");
    let followed = followed.expect("the follower returns the landed file");
    assert_eq!(led, at.final_path);
    assert_eq!(followed, led);
    assert_eq!(fs::read(&at.final_path).unwrap(), body);
    assert!(!at.part.exists());
    assert_eq!(count.load(Ordering::SeqCst), 2);
    assert_eq!(*ranges.lock().unwrap(), vec![None, Some(first as u64)]);
}
