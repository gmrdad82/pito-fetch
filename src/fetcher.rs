use std::fs::{self, File, OpenOptions, TryLockError};
use std::io::{self, Read, Write};
use std::path::{Component, Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};

use sha2::{Digest, Sha256};
use ureq::unversioned::resolver::DefaultResolver;
use ureq::unversioned::transport::{Connector, DefaultConnector};

use crate::cancel::CancelToken;
use crate::digest_hex;
use crate::error::FetchError;
use crate::poll::BodyPoll;
use crate::spec::{FetchProgress, FetchSpec, Request};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const STALL_TIMEOUT: Duration = Duration::from_secs(30);
const POLL_INTERVAL: Duration = Duration::from_millis(200);
const BODY_BUDGET: Duration = Duration::from_secs(24 * 60 * 60);
const BACKOFF: [Duration; 3] = [
    Duration::from_secs(1),
    Duration::from_secs(4),
    Duration::from_secs(9),
];
const CHUNK_SIZE: usize = 64 * 1024;
const PROGRESS_INTERVAL: Duration = Duration::from_millis(100);
const SLEEP_STEP: Duration = Duration::from_millis(50);
const ABSOLUTE_BYTE_CAP: u64 = 8 * 1024 * 1024 * 1024;
const MIN_HINT_CAP: u64 = 1024 * 1024;

#[derive(Debug, Clone, Copy)]
pub struct Timings {
    pub connect_timeout: Duration,
    pub stall_timeout: Duration,
    pub poll_interval: Duration,
    pub backoff: [Duration; 3],
}

impl Default for Timings {
    fn default() -> Self {
        Timings {
            connect_timeout: CONNECT_TIMEOUT,
            stall_timeout: STALL_TIMEOUT,
            poll_interval: POLL_INTERVAL,
            backoff: BACKOFF,
        }
    }
}

pub struct Fetcher {
    dest_root: PathBuf,
    agent: ureq::Agent,
    timings: Timings,
}

enum AttemptError {
    Cancelled,
    Offline,
    Stalled,
    Http(u16),
    Io(io::Error),
    Superseded,
}

enum Download {
    Done,
    Superseded,
}

enum LockError {
    Cancelled,
    Io(io::Error),
}

impl From<LockError> for AttemptError {
    fn from(err: LockError) -> Self {
        match err {
            LockError::Cancelled => AttemptError::Cancelled,
            LockError::Io(source) => AttemptError::Io(source),
        }
    }
}

struct PartWriter<'a> {
    file: File,
    path: &'a Path,
    len: u64,
}

impl<'a> PartWriter<'a> {
    fn open(
        path: &'a Path,
        resume_offset: u64,
        cancel: &CancelToken,
    ) -> Result<Self, AttemptError> {
        let file = if resume_offset > 0 {
            OpenOptions::new().append(true).open(path)
        } else {
            OpenOptions::new().create(true).append(true).open(path)
        }
        .map_err(AttemptError::Io)?;
        let writer = PartWriter {
            file,
            path,
            len: resume_offset,
        };
        if resume_offset == 0 {
            with_part_lock(&writer.file, writer.path, None, cancel, |file| {
                file.set_len(0)
            })?;
        }
        Ok(writer)
    }

    fn write(&mut self, buf: &[u8], cancel: &CancelToken) -> Result<(), AttemptError> {
        with_part_lock(&self.file, self.path, Some(self.len), cancel, |mut file| {
            file.write_all(buf)
        })?;
        self.len += buf.len() as u64;
        Ok(())
    }

    fn finish(self) -> Result<(), AttemptError> {
        self.file.sync_all().map_err(AttemptError::Io)
    }
}

struct StreamLimits {
    stall_timeout: Duration,
    byte_cap: u64,
}

#[derive(Clone, Copy)]
struct RequestRef<'a> {
    url: &'a str,
    dest_rel: &'a str,
    sha256: &'a str,
    size_hint: Option<u64>,
}

impl Fetcher {
    pub fn new(dest_root: impl Into<PathBuf>) -> Self {
        Self::with_timings(dest_root, Timings::default())
    }

    pub fn with_timings(dest_root: impl Into<PathBuf>, timings: Timings) -> Self {
        let config = ureq::Agent::config_builder()
            .timeout_connect(Some(timings.connect_timeout))
            .timeout_send_request(Some(timings.stall_timeout))
            .timeout_recv_response(Some(timings.stall_timeout))
            .timeout_recv_body(Some(BODY_BUDGET))
            .http_status_as_error(false)
            .max_redirects(10)
            .build();
        Fetcher {
            dest_root: dest_root.into(),
            agent: ureq::Agent::with_parts(
                config,
                DefaultConnector::new().chain(BodyPoll(timings.poll_interval)),
                DefaultResolver::default(),
            ),
            timings,
        }
    }

    pub fn fetch(
        &self,
        spec: &FetchSpec,
        cancel: &CancelToken,
        mut progress: impl FnMut(FetchProgress),
    ) -> Result<PathBuf, FetchError> {
        self.fetch_inner(
            RequestRef {
                url: spec.url,
                dest_rel: spec.dest_rel,
                sha256: spec.sha256,
                size_hint: spec.size_hint,
            },
            cancel,
            &mut progress,
        )
    }

    pub fn fetch_request(
        &self,
        request: &Request,
        cancel: &CancelToken,
        mut progress: impl FnMut(FetchProgress),
    ) -> Result<PathBuf, FetchError> {
        self.fetch_inner(
            RequestRef {
                url: &request.url,
                dest_rel: &request.dest_rel,
                sha256: &request.sha256,
                size_hint: request.size_hint,
            },
            cancel,
            &mut progress,
        )
    }

    fn fetch_inner(
        &self,
        request: RequestRef<'_>,
        cancel: &CancelToken,
        progress: &mut dyn FnMut(FetchProgress),
    ) -> Result<PathBuf, FetchError> {
        if cancel.is_cancelled() {
            return Err(FetchError::Cancelled);
        }
        let final_path = self.contained_dest(request.dest_rel)?;
        let part_path = part_path_of(&final_path);
        if let Some(parent) = final_path.parent() {
            fs::create_dir_all(parent).map_err(|source| FetchError::Io {
                path: parent.to_path_buf(),
                source,
            })?;
        }

        loop {
            match self.download_with_retries(request, &part_path, cancel, progress)? {
                Download::Done => {
                    return self.verify_and_rename(request, &part_path, &final_path, cancel);
                }
                Download::Superseded => {
                    if let Some(path) = self.follow(request, &part_path, &final_path, cancel)? {
                        return Ok(path);
                    }
                }
            }
        }
    }

    fn follow(
        &self,
        request: RequestRef<'_>,
        part_path: &Path,
        final_path: &Path,
        cancel: &CancelToken,
    ) -> Result<Option<PathBuf>, FetchError> {
        let mut last_len = None;
        let mut quiet_since = Instant::now();
        loop {
            sleep_interruptible(self.timings.poll_interval, cancel)?;
            match fs::metadata(part_path) {
                Ok(meta) if last_len == Some(meta.len()) => {
                    if quiet_since.elapsed() >= self.timings.stall_timeout {
                        return Ok(None);
                    }
                }
                Ok(meta) => {
                    last_len = Some(meta.len());
                    quiet_since = Instant::now();
                }
                Err(source) if source.kind() == io::ErrorKind::NotFound => {
                    return Ok(verify_completed(request, final_path).ok());
                }
                Err(source) => {
                    return Err(FetchError::Io {
                        path: part_path.to_path_buf(),
                        source,
                    });
                }
            }
        }
    }

    fn contained_dest(&self, dest_rel: &str) -> Result<PathBuf, FetchError> {
        let reject = || FetchError::Io {
            path: PathBuf::from(dest_rel),
            source: io::Error::new(
                io::ErrorKind::InvalidInput,
                "dest_rel must be a plain relative path inside dest_root",
            ),
        };
        let mut contained = PathBuf::new();
        for component in Path::new(dest_rel).components() {
            match component {
                Component::Normal(part) => contained.push(part),
                Component::CurDir => {}
                Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                    return Err(reject());
                }
            }
        }
        if contained.as_os_str().is_empty() {
            return Err(reject());
        }
        Ok(self.dest_root.join(contained))
    }

    fn download_with_retries(
        &self,
        request: RequestRef<'_>,
        part_path: &Path,
        cancel: &CancelToken,
        progress: &mut dyn FnMut(FetchProgress),
    ) -> Result<Download, FetchError> {
        for attempt in 0..=self.timings.backoff.len() {
            if attempt > 0 {
                sleep_interruptible(self.timings.backoff[attempt - 1], cancel)?;
            }
            match self.attempt(request, part_path, cancel, progress) {
                Ok(()) => return Ok(Download::Done),
                Err(AttemptError::Superseded) => return Ok(Download::Superseded),
                Err(AttemptError::Cancelled) => return Err(FetchError::Cancelled),
                Err(AttemptError::Offline) => {
                    return Err(FetchError::Offline {
                        url: request.url.to_string(),
                    });
                }
                Err(AttemptError::Stalled) => {
                    return Err(FetchError::Stalled {
                        url: request.url.to_string(),
                    });
                }
                Err(AttemptError::Http(status)) => {
                    return Err(FetchError::Http {
                        status,
                        url: request.url.to_string(),
                    });
                }
                Err(AttemptError::Io(_)) if attempt < self.timings.backoff.len() => {}
                Err(AttemptError::Io(source)) => {
                    return Err(FetchError::Io {
                        path: part_path.to_path_buf(),
                        source,
                    });
                }
            }
        }
        unreachable!("loop always returns before exhausting its range")
    }

    fn attempt(
        &self,
        request: RequestRef<'_>,
        part_path: &Path,
        cancel: &CancelToken,
        progress: &mut dyn FnMut(FetchProgress),
    ) -> Result<(), AttemptError> {
        if cancel.is_cancelled() {
            return Err(AttemptError::Cancelled);
        }
        let existing_len = fs::metadata(part_path).map(|m| m.len()).unwrap_or(0);
        let (resume_offset, status, headers, body) =
            self.open_response(request, part_path, existing_len)?;
        let total = total_size(&headers, status, resume_offset, request.size_hint);
        let mut part = PartWriter::open(part_path, resume_offset, cancel)?;
        let reader = body.into_reader();
        let limits = StreamLimits {
            stall_timeout: self.timings.stall_timeout,
            byte_cap: byte_cap(request.size_hint),
        };
        stream_body(
            reader,
            &mut part,
            resume_offset,
            total,
            limits,
            cancel,
            progress,
        )?;
        part.finish()
    }

    fn open_response(
        &self,
        request: RequestRef<'_>,
        part_path: &Path,
        existing_len: u64,
    ) -> Result<(u64, u16, ureq::http::HeaderMap, ureq::Body), AttemptError> {
        let mut want_range = existing_len > 0;
        let mut offset = existing_len;
        loop {
            let mut request_builder = self.agent.get(request.url);
            if want_range {
                request_builder = request_builder.header("Range", format!("bytes={offset}-"));
            }
            let response = request_builder.call().map_err(classify_call_error)?;
            let status = response.status().as_u16();
            let (parts, body) = response.into_parts();
            match status {
                200 if want_range => {
                    let _ = fs::remove_file(part_path);
                    want_range = false;
                    offset = 0;
                    continue;
                }
                200 | 206 => return Ok((offset, status, parts.headers, body)),
                416 if want_range => {
                    let _ = fs::remove_file(part_path);
                    want_range = false;
                    offset = 0;
                    continue;
                }
                other => return Err(AttemptError::Http(other)),
            }
        }
    }

    fn verify_and_rename(
        &self,
        request: RequestRef<'_>,
        part_path: &Path,
        final_path: &Path,
        cancel: &CancelToken,
    ) -> Result<PathBuf, FetchError> {
        let file = match File::open(part_path) {
            Ok(file) => file,
            Err(source) if source.kind() == io::ErrorKind::NotFound => {
                return verify_completed(request, final_path);
            }
            Err(source) => {
                return Err(FetchError::Io {
                    path: part_path.to_path_buf(),
                    source,
                });
            }
        };
        match acquire_part_lock(&file, cancel) {
            Ok(()) => {}
            Err(LockError::Cancelled) => return Err(FetchError::Cancelled),
            Err(LockError::Io(source)) => {
                return Err(FetchError::Io {
                    path: part_path.to_path_buf(),
                    source,
                });
            }
        }
        if !part_still_ours(part_path, &file) {
            return verify_completed(request, final_path);
        }
        let got = hash_reader(&file).map_err(|source| FetchError::Io {
            path: part_path.to_path_buf(),
            source,
        })?;
        let got_hex = digest_hex(&got);
        if got_hex != request.sha256 {
            let _ = fs::remove_file(part_path);
            return Err(FetchError::ChecksumMismatch {
                expected: request.sha256.to_string(),
                got: got_hex,
            });
        }
        fs::rename(part_path, final_path).map_err(|source| FetchError::Io {
            path: final_path.to_path_buf(),
            source,
        })?;
        Ok(final_path.to_path_buf())
    }
}

fn verify_completed(request: RequestRef<'_>, final_path: &Path) -> Result<PathBuf, FetchError> {
    let got = hash_file(final_path).map_err(|source| FetchError::Io {
        path: final_path.to_path_buf(),
        source,
    })?;
    let got_hex = digest_hex(&got);
    if got_hex == request.sha256 {
        Ok(final_path.to_path_buf())
    } else {
        Err(FetchError::ChecksumMismatch {
            expected: request.sha256.to_string(),
            got: got_hex,
        })
    }
}

fn acquire_part_lock(file: &File, cancel: &CancelToken) -> Result<(), LockError> {
    loop {
        match file.try_lock() {
            Ok(()) => return Ok(()),
            Err(TryLockError::WouldBlock) => {
                if cancel.is_cancelled() {
                    return Err(LockError::Cancelled);
                }
                thread::sleep(SLEEP_STEP);
            }
            Err(TryLockError::Error(source)) => return Err(LockError::Io(source)),
        }
    }
}

fn with_part_lock<T>(
    file: &File,
    part_path: &Path,
    expected_len: Option<u64>,
    cancel: &CancelToken,
    op: impl FnOnce(&File) -> io::Result<T>,
) -> Result<T, AttemptError> {
    acquire_part_lock(file, cancel).map_err(AttemptError::from)?;
    let ours = part_len_if_ours(part_path, file)
        .is_some_and(|len| expected_len.is_none_or(|expected| expected == len));
    let outcome = if ours {
        op(file).map_err(AttemptError::Io)
    } else {
        Err(AttemptError::Superseded)
    };
    let _ = file.unlock();
    outcome
}

fn part_path_of(final_path: &Path) -> PathBuf {
    let mut os = final_path.as_os_str().to_os_string();
    os.push(".part");
    PathBuf::from(os)
}

fn total_size(
    headers: &ureq::http::HeaderMap,
    status: u16,
    resume_offset: u64,
    size_hint: Option<u64>,
) -> Option<u64> {
    if status == 206
        && let Some(total) = headers
            .get("content-range")
            .and_then(|v| v.to_str().ok())
            .and_then(parse_content_range_total)
    {
        return Some(total);
    }
    if let Some(len) = headers
        .get("content-length")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse::<u64>().ok())
    {
        return Some(resume_offset + len);
    }
    size_hint
}

fn parse_content_range_total(value: &str) -> Option<u64> {
    value.rsplit('/').next()?.trim().parse().ok()
}

fn sleep_interruptible(total: Duration, cancel: &CancelToken) -> Result<(), FetchError> {
    let mut waited = Duration::ZERO;
    while waited < total {
        if cancel.is_cancelled() {
            return Err(FetchError::Cancelled);
        }
        let step = SLEEP_STEP.min(total - waited);
        thread::sleep(step);
        waited += step;
    }
    if cancel.is_cancelled() {
        return Err(FetchError::Cancelled);
    }
    Ok(())
}

fn classify_call_error(err: ureq::Error) -> AttemptError {
    match err {
        ureq::Error::Timeout(ureq::Timeout::Connect) => AttemptError::Offline,
        ureq::Error::Timeout(ureq::Timeout::Resolve) => AttemptError::Offline,
        ureq::Error::Timeout(_) => AttemptError::Stalled,
        _ => AttemptError::Offline,
    }
}

fn byte_cap(size_hint: Option<u64>) -> u64 {
    size_hint
        .map(|hint| hint.saturating_mul(2).max(MIN_HINT_CAP))
        .unwrap_or(ABSOLUTE_BYTE_CAP)
}

#[cfg(unix)]
fn identity_of(meta: &fs::Metadata) -> (u64, u64) {
    use std::os::unix::fs::MetadataExt;
    (meta.dev(), meta.ino())
}

#[cfg(not(unix))]
fn identity_of(_meta: &fs::Metadata) -> (u64, u64) {
    (0, 0)
}

fn part_still_ours(part_path: &Path, file: &File) -> bool {
    part_len_if_ours(part_path, file).is_some()
}

fn part_len_if_ours(part_path: &Path, file: &File) -> Option<u64> {
    match (fs::metadata(part_path), file.metadata()) {
        (Ok(on_disk), Ok(ours)) if identity_of(&on_disk) == identity_of(&ours) => Some(ours.len()),
        _ => None,
    }
}

fn stream_body(
    mut reader: impl Read,
    part: &mut PartWriter<'_>,
    resume_offset: u64,
    total: Option<u64>,
    limits: StreamLimits,
    cancel: &CancelToken,
    progress: &mut dyn FnMut(FetchProgress),
) -> Result<(), AttemptError> {
    let mut buf = [0u8; CHUNK_SIZE];
    let start = Instant::now();
    let mut last_data = Instant::now();
    let mut last_emit = start.checked_sub(PROGRESS_INTERVAL).unwrap_or(start);
    let mut received = resume_offset;
    loop {
        if cancel.is_cancelled() {
            return Err(AttemptError::Cancelled);
        }
        match reader.read(&mut buf) {
            Ok(0) => return Ok(()),
            Ok(n) => {
                part.write(&buf[..n], cancel)?;
                received += n as u64;
                if received >= limits.byte_cap {
                    return Ok(());
                }
                last_data = Instant::now();
                if last_emit.elapsed() >= PROGRESS_INTERVAL {
                    let elapsed = start.elapsed().as_secs_f64().max(0.001);
                    let sent = received.saturating_sub(resume_offset);
                    let speed_bps = (sent as f64 / elapsed) as u64;
                    progress(FetchProgress {
                        received,
                        total,
                        speed_bps,
                    });
                    last_emit = Instant::now();
                }
            }
            Err(err) => {
                if cancel.is_cancelled() {
                    return Err(AttemptError::Cancelled);
                }
                if is_recv_body_poll_timeout(&err) {
                    if last_data.elapsed() >= limits.stall_timeout {
                        return Err(AttemptError::Stalled);
                    }
                    continue;
                }
                return Err(AttemptError::Io(err));
            }
        }
    }
}

fn is_recv_body_poll_timeout(err: &io::Error) -> bool {
    err.get_ref()
        .and_then(|inner| inner.downcast_ref::<ureq::Error>())
        .map(|ureq_err| matches!(ureq_err, ureq::Error::Timeout(ureq::Timeout::RecvBody)))
        .unwrap_or(false)
}

fn hash_file(path: &Path) -> io::Result<[u8; 32]> {
    let file = File::open(path)?;
    hash_reader(&file)
}

fn hash_reader(mut file: &File) -> io::Result<[u8; 32]> {
    let mut hasher = Sha256::new();
    let mut buf = [0u8; CHUNK_SIZE];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hasher.finalize().into())
}

#[cfg(test)]
mod owned_request_tests {
    use super::*;
    use crate::{Request, sha256_hex};
    use std::net::TcpListener;

    fn serve(body: Vec<u8>, requests: usize) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            for stream in listener.incoming().take(requests) {
                let mut stream = stream.unwrap();
                let mut request = [0; 1024];
                let _ = stream.read(&mut request);
                write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                )
                .unwrap();
                stream.write_all(&body).unwrap();
            }
        });
        format!("http://{address}/asset")
    }

    #[test]
    fn owned_request_matches_static_spec_and_checksum_refusal() {
        let scratch = Path::new(env!("CARGO_MANIFEST_DIR")).join("tmp/testscratch");
        fs::create_dir_all(&scratch).unwrap();
        let root = tempfile::Builder::new()
            .prefix("fetch-owned-")
            .tempdir_in(scratch)
            .unwrap();
        let body = b"runtime catalog asset".to_vec();
        let good_url = Box::leak(serve(body.clone(), 2).into_boxed_str());
        let digest = Box::leak(sha256_hex(&body).into_boxed_str());
        let static_spec = FetchSpec {
            url: good_url,
            dest_rel: "static.bin",
            sha256: digest,
            size_hint: Some(body.len() as u64),
        };
        let owned = Request {
            url: good_url.to_string(),
            dest_rel: "owned.bin".into(),
            sha256: digest.to_string(),
            size_hint: Some(body.len() as u64),
        };
        let fetcher = Fetcher::new(root.path());
        let static_path = fetcher
            .fetch(&static_spec, &CancelToken::new(), |_| {})
            .unwrap();
        let owned_path = fetcher
            .fetch_request(&owned, &CancelToken::new(), |_| {})
            .unwrap();
        assert_eq!(
            fs::read(static_path).unwrap(),
            fs::read(owned_path).unwrap()
        );

        let bad_url = Box::leak(serve(body, 2).into_boxed_str());
        let bad_spec = FetchSpec {
            url: bad_url,
            dest_rel: "bad-static.bin",
            sha256: "00",
            size_hint: None,
        };
        let bad_owned = Request {
            url: bad_url.to_string(),
            dest_rel: "bad-owned.bin".into(),
            sha256: "00".into(),
            size_hint: None,
        };
        assert!(matches!(
            fetcher.fetch(&bad_spec, &CancelToken::new(), |_| {}),
            Err(FetchError::ChecksumMismatch { .. })
        ));
        assert!(matches!(
            fetcher.fetch_request(&bad_owned, &CancelToken::new(), |_| {}),
            Err(FetchError::ChecksumMismatch { .. })
        ));
    }
}
