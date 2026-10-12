use std::io::{IsTerminal, Write};
use std::path::Path;
use std::process::ExitCode;

use fetch::{CancelToken, FetchProgress, Fetcher, Request};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [url, sha256, dest] = args.as_slice() else {
        eprintln!("usage: get <URL> <SHA256> <DEST>");
        return ExitCode::from(2);
    };
    let dest = Path::new(dest);
    let Some(name) = dest.file_name() else {
        eprintln!("get: {} names no file", dest.display());
        return ExitCode::from(2);
    };
    let root = match dest.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent,
        _ => Path::new("."),
    };
    let request = Request {
        url: url.clone(),
        dest_rel: name.to_string_lossy().into_owned(),
        sha256: sha256.to_ascii_lowercase(),
        size_hint: None,
    };
    let terminal = std::io::stderr().is_terminal();
    let fetcher = Fetcher::new(root);
    let result = fetcher.fetch_request(&request, &CancelToken::new(), |p| {
        if terminal {
            eprint!("\r\x1b[2K{}", line(p));
            let _ = std::io::stderr().flush();
        }
    });
    if terminal {
        eprint!("\r\x1b[2K");
    }
    match result {
        Ok(path) => {
            println!("{}", path.display());
            ExitCode::SUCCESS
        }
        Err(err) => {
            eprintln!("get: {err}");
            ExitCode::FAILURE
        }
    }
}

fn line(p: FetchProgress) -> String {
    let speed = format!("{}/s", mb(p.speed_bps));
    match p.total {
        Some(total) if total > 0 => {
            let filled = (p.received.min(total) * 30 / total) as usize;
            format!(
                "[{}{}] {:>3}%  {} of {}  {speed}",
                "#".repeat(filled),
                "-".repeat(30 - filled),
                p.received.min(total) * 100 / total,
                mb(p.received),
                mb(total)
            )
        }
        _ => format!("{}  {speed}", mb(p.received)),
    }
}

fn mb(bytes: u64) -> String {
    format!("{:.1} MB", bytes as f64 / 1_000_000.0)
}
