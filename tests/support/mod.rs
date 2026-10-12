#![allow(dead_code)]

use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;

pub fn scratch_dir(label: &str) -> tempfile::TempDir {
    let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tmp/testscratch");
    std::fs::create_dir_all(&root).expect("test scratch root is creatable");
    tempfile::Builder::new()
        .prefix(label)
        .tempdir_in(root)
        .expect("test scratch dir is creatable")
}

pub struct ParsedRequest {
    pub method: String,
    pub path: String,
    pub range_start: Option<u64>,
}

pub fn parse_request(stream: &TcpStream) -> ParsedRequest {
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    let mut request_line = String::new();
    reader.read_line(&mut request_line).unwrap();
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or("").to_string();
    let path = parts.next().unwrap_or("").to_string();
    let mut range_start = None;
    loop {
        let mut line = String::new();
        let n = reader.read_line(&mut line).unwrap();
        if n == 0 || line == "\r\n" || line == "\n" {
            break;
        }
        let lower = line.to_ascii_lowercase();
        if let Some(v) = lower.strip_prefix("range:")
            && let Some(rest) = v.trim().strip_prefix("bytes=")
        {
            let start = rest.split('-').next().unwrap_or("").trim();
            range_start = start.parse::<u64>().ok();
        }
    }
    ParsedRequest {
        method,
        path,
        range_start,
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
