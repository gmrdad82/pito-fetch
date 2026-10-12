use std::fs::File;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::thread;
use std::time::Duration;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (dir, port, rate) = match args.as_slice() {
        [dir, port] => (dir, port, None),
        [dir, port, rate] => (dir, port, Some(rate)),
        _ => {
            eprintln!("usage: serve <DIR> <PORT> [BYTES_PER_SECOND]");
            return ExitCode::from(2);
        }
    };
    let rate = match rate.map(|r| r.parse::<u64>()) {
        None => None,
        Some(Ok(r)) if r > 0 => Some(r),
        Some(_) => {
            eprintln!("serve: the rate is a whole number of bytes per second");
            return ExitCode::from(2);
        }
    };
    let listener = match TcpListener::bind(("127.0.0.1", port.parse().unwrap_or(0))) {
        Ok(listener) => listener,
        Err(err) => {
            eprintln!("serve: {err}");
            return ExitCode::FAILURE;
        }
    };
    let dir = PathBuf::from(dir);
    for stream in listener.incoming().flatten() {
        let dir = dir.clone();
        thread::spawn(move || {
            let _ = handle(stream, &dir, rate);
        });
    }
    ExitCode::SUCCESS
}

fn handle(mut stream: TcpStream, dir: &Path, rate: Option<u64>) -> std::io::Result<()> {
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut request_line = String::new();
    reader.read_line(&mut request_line)?;
    let name = request_line
        .split_whitespace()
        .nth(1)
        .unwrap_or("")
        .trim_start_matches('/');
    let mut start = 0;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line)? == 0 || line.trim().is_empty() {
            break;
        }
        if let Some(value) = line.to_ascii_lowercase().strip_prefix("range: bytes=") {
            start = value.trim().trim_end_matches('-').parse().unwrap_or(0);
        }
    }
    let file = (!name.is_empty() && !name.contains(['/', '\\']) && name != "..")
        .then(|| File::open(dir.join(name)).ok())
        .flatten();
    let Some(mut file) = file else {
        return stream.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n");
    };
    let len = file.metadata()?.len();
    if start >= len && start > 0 {
        let head = format!(
            "HTTP/1.1 416 Range Not Satisfiable\r\nContent-Range: bytes */{len}\r\nContent-Length: 0\r\n\r\n"
        );
        return stream.write_all(head.as_bytes());
    }
    let head = if start > 0 {
        format!(
            "HTTP/1.1 206 Partial Content\r\nContent-Range: bytes {start}-{}/{len}\r\nContent-Length: {}\r\n\r\n",
            len - 1,
            len - start
        )
    } else {
        format!("HTTP/1.1 200 OK\r\nAccept-Ranges: bytes\r\nContent-Length: {len}\r\n\r\n")
    };
    stream.write_all(head.as_bytes())?;
    file.seek(SeekFrom::Start(start))?;
    let chunk = rate.map_or(64 * 1024, |r| (r / 20).clamp(1, 64 * 1024) as usize);
    let mut buf = vec![0; chunk];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            return Ok(());
        }
        stream.write_all(&buf[..n])?;
        if let Some(r) = rate {
            thread::sleep(Duration::from_secs_f64(n as f64 / r as f64));
        }
    }
}
