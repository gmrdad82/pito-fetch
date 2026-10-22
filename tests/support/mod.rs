#![allow(dead_code)]

use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use fetch::{FetchSpec, Timings, sha256_hex};

pub fn scratch_dir(label: &str) -> tempfile::TempDir {
    let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tmp/testscratch");
    std::fs::create_dir_all(&root).expect("test scratch root is creatable");
    tempfile::Builder::new()
        .prefix(label)
        .tempdir_in(root)
        .expect("test scratch dir is creatable")
}

#[derive(Debug, Clone)]
pub struct ParsedRequest {
    pub method: String,
    pub path: String,
    pub range_start: Option<u64>,
    pub range: Option<String>,
}

pub fn parse_request(stream: &TcpStream) -> ParsedRequest {
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    let mut request_line = String::new();
    reader.read_line(&mut request_line).unwrap();
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or("").to_string();
    let path = parts.next().unwrap_or("").to_string();
    let mut range_start = None;
    let mut range = None;
    loop {
        let mut line = String::new();
        let n = reader.read_line(&mut line).unwrap();
        if n == 0 || line == "\r\n" || line == "\n" {
            break;
        }
        let lower = line.to_ascii_lowercase();
        if let Some(v) = lower.strip_prefix("range:") {
            range = Some(line["range:".len()..].trim().to_string());
            let Some(rest) = v.trim().strip_prefix("bytes=") else {
                continue;
            };
            let start = rest.split('-').next().unwrap_or("").trim();
            range_start = start.parse::<u64>().ok();
        }
    }
    ParsedRequest {
        method,
        path,
        range_start,
        range,
    }
}

pub fn payload(len: usize) -> Vec<u8> {
    (0..len)
        .map(|i| ((i as u32).wrapping_mul(2_654_435_761) >> 16) as u8)
        .collect()
}

pub fn spawn<F>(handler: F) -> (String, Arc<AtomicUsize>)
where
    F: Fn(usize, TcpStream) + Send + 'static,
{
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let count = Arc::new(AtomicUsize::new(0));
    let count_in_thread = count.clone();
    thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(stream) = stream else { break };
            let n = count_in_thread.fetch_add(1, Ordering::SeqCst);
            handler(n, stream);
        }
    });
    (format!("http://127.0.0.1:{port}/asset"), count)
}

pub fn write_head(stream: &mut TcpStream, status_line: &str, headers: &[String]) {
    let _ = write!(stream, "{status_line}\r\n");
    for header in headers {
        let _ = write!(stream, "{header}\r\n");
    }
    let _ = write!(stream, "\r\n");
}

pub fn write_chunks_paced(
    stream: &mut TcpStream,
    data: &[u8],
    chunk: usize,
    pace: std::time::Duration,
) {
    for slice in data.chunks(chunk) {
        if stream.write_all(slice).is_err() {
            return;
        }
        let _ = stream.flush();
        if !pace.is_zero() {
            thread::sleep(pace);
        }
    }
}

pub fn spawn_threaded<F>(handler: F) -> (String, Arc<AtomicUsize>)
where
    F: Fn(usize, TcpStream) + Send + Sync + 'static,
{
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let count = Arc::new(AtomicUsize::new(0));
    let count_in_thread = count.clone();
    let handler = Arc::new(handler);
    thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(stream) = stream else { break };
            let n = count_in_thread.fetch_add(1, Ordering::SeqCst);
            let handler = handler.clone();
            thread::spawn(move || handler(n, stream));
        }
    });
    (format!("http://127.0.0.1:{port}/asset"), count)
}

pub fn leak(value: impl Into<String>) -> &'static str {
    Box::leak(value.into().into_boxed_str())
}

pub fn spec_for(url: &str, dest_rel: &str, body: &[u8], size_hint: Option<u64>) -> FetchSpec {
    FetchSpec {
        url: leak(url),
        dest_rel: leak(dest_rel),
        sha256: leak(sha256_hex(body)),
        size_hint,
    }
}

pub fn fast_timings(stall: Duration) -> Timings {
    Timings {
        connect_timeout: Duration::from_secs(3),
        stall_timeout: stall,
        poll_interval: Duration::from_millis(30),
        backoff: [Duration::from_millis(10); 3],
    }
}

pub fn part_path(final_path: &Path) -> PathBuf {
    PathBuf::from(format!("{}.part", final_path.display()))
}

pub fn file_len(path: &Path) -> u64 {
    std::fs::metadata(path).map(|m| m.len()).unwrap_or(0)
}

pub fn wait_until(limit: Duration, mut ready: impl FnMut() -> bool) -> bool {
    let started = Instant::now();
    while !ready() {
        if started.elapsed() >= limit {
            return false;
        }
        thread::sleep(Duration::from_millis(5));
    }
    true
}

pub fn respond_status(stream: &mut TcpStream, status_line: &str) {
    write_head(
        stream,
        status_line,
        &[
            "Content-Length: 0".to_string(),
            "Connection: close".to_string(),
        ],
    );
    let _ = stream.flush();
}

pub fn respond_full(stream: &mut TcpStream, body: &[u8]) {
    write_head(
        stream,
        "HTTP/1.1 200 OK",
        &[
            format!("Content-Length: {}", body.len()),
            "Connection: close".to_string(),
        ],
    );
    let _ = stream.write_all(body);
    let _ = stream.flush();
}

pub fn respond_range(stream: &mut TcpStream, body: &[u8], start: u64) {
    let start = start as usize;
    if start >= body.len() {
        respond_status(stream, "HTTP/1.1 416 Range Not Satisfiable");
        return;
    }
    let rest = &body[start..];
    write_head(
        stream,
        "HTTP/1.1 206 Partial Content",
        &[
            format!(
                "Content-Range: bytes {}-{}/{}",
                start,
                body.len() - 1,
                body.len()
            ),
            format!("Content-Length: {}", rest.len()),
            "Connection: close".to_string(),
        ],
    );
    let _ = stream.write_all(rest);
    let _ = stream.flush();
}
