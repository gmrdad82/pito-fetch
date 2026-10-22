use std::fs::{self, File, OpenOptions, TryLockError};
use std::io::{self, Read, Seek, SeekFrom, Write};
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
const CHUNK_SIZE: usize = 1024 * 1024;
const INPUT_BUFFER_SIZE: usize = 256 * 1024;
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

enum Download<'a> {
    Done(PartWriter<'a>),
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
    hasher: Sha256,
}

impl<'a> PartWriter<'a> {
    fn resume(path: &'a Path, cancel: &CancelToken) -> Result<Option<Self>, AttemptError> {
        let file = match OpenOptions::new().read(true).append(true).open(path) {
            Ok(file) => file,
            Err(source) if source.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(source) => return Err(AttemptError::Io(source)),
        };
        let hashed = with_part_lock(&file, path, None, cancel, |mut file| {
            let len = file.metadata()?.len();
            file.seek(SeekFrom::Start(0))?;
            let mut hasher = Sha256::new();
            if hash_into(file.take(len), &mut hasher)? != len {
                return Err(io::ErrorKind::UnexpectedEof.into());
            }
            Ok((len, hasher))
        });
        match hashed {
            Ok((0, _)) | Err(AttemptError::Superseded) => Ok(None),
            Ok((len, hasher)) => Ok(Some(PartWriter {
                file,
                path,
                len,
                hasher,
            })),
            Err(err) => Err(err),
        }
    }

    fn create(path: &'a Path, cancel: &CancelToken) -> Result<Self, AttemptError> {
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .map_err(AttemptError::Io)?;
        with_part_lock(&file, path, None, cancel, |file| file.set_len(0))?;
        Ok(PartWriter {
            file,
            path,
            len: 0,
            hasher: Sha256::new(),
        })
    }

    fn write(&mut self, buf: &[u8], cancel: &CancelToken) -> Result<(), AttemptError> {
        if buf.is_empty() {
            return Ok(());
        }
        with_part_lock(&self.file, self.path, Some(self.len), cancel, |mut file| {
            file.write_all(buf)
        })?;
        self.hasher.update(buf);
        self.len += buf.len() as u64;
        Ok(())
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
            .input_buffer_size(INPUT_BUFFER_SIZE)
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
                Download::Done(part) => {
                    if let Some(path) =
                        self.verify_and_rename(request, part, &final_path, cancel)?
                    {
                        return Ok(path);
                    }
                }
                Download::Superseded => {}
            }
            if let Some(path) = self.follow(request, &part_path, &final_path, cancel)? {
                return Ok(path);
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

    fn download_with_retries<'a>(
        &self,
        request: RequestRef<'_>,
        part_path: &'a Path,
        cancel: &CancelToken,
        progress: &mut dyn FnMut(FetchProgress),
    ) -> Result<Download<'a>, FetchError> {
        for attempt in 0..=self.timings.backoff.len() {
            if attempt > 0 {
                sleep_interruptible(self.timings.backoff[attempt - 1], cancel)?;
            }
            match self.attempt(request, part_path, cancel, progress) {
                Ok(part) => return Ok(Download::Done(part)),
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

    fn attempt<'a>(
        &self,
        request: RequestRef<'_>,
        part_path: &'a Path,
        cancel: &CancelToken,
        progress: &mut dyn FnMut(FetchProgress),
    ) -> Result<PartWriter<'a>, AttemptError> {
        if cancel.is_cancelled() {
            return Err(AttemptError::Cancelled);
        }
        let resumed = PartWriter::resume(part_path, cancel)?;
        let existing_len = resumed.as_ref().map_or(0, |part| part.len);
        let (resume_offset, status, headers, body) =
            self.open_response(request, part_path, existing_len)?;
        let total = total_size(&headers, status, resume_offset, request.size_hint);
        let mut part = match resumed {
            Some(part) if resume_offset > 0 => part,
            _ => PartWriter::create(part_path, cancel)?,
        };
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
        Ok(part)
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
        part: PartWriter<'_>,
        final_path: &Path,
        cancel: &CancelToken,
    ) -> Result<Option<PathBuf>, FetchError> {
        let PartWriter {
            file,
            path,
            len,
            hasher,
        } = part;
        let got_hex = digest_hex(&hasher.finalize());
        let matches = got_hex == request.sha256;
        let outcome = with_part_lock(&file, path, Some(len), cancel, |file| {
            if matches {
                file.sync_all()?;
                fs::rename(path, final_path)
            } else {
                fs::remove_file(path)
            }
        });
        match outcome {
            Ok(()) if matches => Ok(Some(final_path.to_path_buf())),
            Ok(()) => Err(FetchError::ChecksumMismatch {
                expected: request.sha256.to_string(),
                got: got_hex,
            }),
            Err(AttemptError::Cancelled) => Err(FetchError::Cancelled),
            Err(AttemptError::Io(source)) if source.kind() != io::ErrorKind::NotFound => {
                Err(FetchError::Io {
                    path: path.to_path_buf(),
                    source,
                })
            }
            Err(_) => Ok(None),
        }
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
        return Some(resume_offset.saturating_add(len));
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
    let mut buf = vec![0u8; CHUNK_SIZE];
    let mut filled = 0;
    let start = Instant::now();
    let mut last_data = Instant::now();
    let mut last_emit = start.checked_sub(PROGRESS_INTERVAL).unwrap_or(start);
    let mut received = resume_offset;
    loop {
        if cancel.is_cancelled() {
            let _ = part.write(&buf[..filled], cancel);
            return Err(AttemptError::Cancelled);
        }
        match reader.read(&mut buf[filled..]) {
            Ok(0) => {
                part.write(&buf[..filled], cancel)?;
                emit_progress(progress, received, resume_offset, total, start);
                return Ok(());
            }
            Ok(n) => {
                filled += n;
                received += n as u64;
                if received >= limits.byte_cap {
                    part.write(&buf[..filled], cancel)?;
                    emit_progress(progress, received, resume_offset, total, start);
                    return Ok(());
                }
                if filled == buf.len() {
                    part.write(&buf, cancel)?;
                    filled = 0;
                }
                last_data = Instant::now();
                if last_emit.elapsed() >= PROGRESS_INTERVAL {
                    emit_progress(progress, received, resume_offset, total, start);
                    last_emit = Instant::now();
                }
            }
            Err(err) => {
                let flushed = part.write(&buf[..filled], cancel);
                filled = 0;
                if cancel.is_cancelled() {
                    return Err(AttemptError::Cancelled);
                }
                flushed?;
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

fn emit_progress(
    progress: &mut dyn FnMut(FetchProgress),
    received: u64,
    resume_offset: u64,
    total: Option<u64>,
    start: Instant,
) {
    let elapsed = start.elapsed().as_secs_f64().max(0.001);
    let sent = received.saturating_sub(resume_offset);
    progress(FetchProgress {
        received,
        total,
        speed_bps: (sent as f64 / elapsed) as u64,
    });
}

fn is_recv_body_poll_timeout(err: &io::Error) -> bool {
    err.get_ref()
        .and_then(|inner| inner.downcast_ref::<ureq::Error>())
        .map(|ureq_err| matches!(ureq_err, ureq::Error::Timeout(ureq::Timeout::RecvBody)))
        .unwrap_or(false)
}

fn hash_file(path: &Path) -> io::Result<[u8; 32]> {
    let mut hasher = Sha256::new();
    hash_into(File::open(path)?, &mut hasher)?;
    Ok(hasher.finalize().into())
}

fn hash_into(mut reader: impl Read, hasher: &mut Sha256) -> io::Result<u64> {
    let mut buf = vec![0u8; CHUNK_SIZE];
    let mut total = 0;
    loop {
        let n = match reader.read(&mut buf) {
            Ok(0) => return Ok(total),
            Ok(n) => n,
            Err(err) if err.kind() == io::ErrorKind::Interrupted => continue,
            Err(err) => return Err(err),
        };
        hasher.update(&buf[..n]);
        total += n as u64;
    }
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
                let Ok(mut stream) = stream else { continue };
                let mut request = [0; 1024];
                let _ = stream.read(&mut request);
                let _ = write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = stream.write_all(&body);
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

    #[test]
    fn fetch_into_a_folder_removed_mid_download_ends_in_an_io_error() {
        let scratch = Path::new(env!("CARGO_MANIFEST_DIR")).join("tmp/testscratch");
        fs::create_dir_all(&scratch).unwrap();
        let root = tempfile::Builder::new()
            .prefix("fetch-owned-")
            .tempdir_in(scratch)
            .unwrap();
        let body = b"an asset whose folder goes away".to_vec();
        let url = serve(body.clone(), 5);
        let digest = sha256_hex(&body);
        let request = Request {
            url,
            dest_rel: "gone/asset.bin".into(),
            sha256: digest,
            size_hint: Some(body.len() as u64),
        };
        let fetcher = Fetcher::with_timings(
            root.path(),
            Timings {
                connect_timeout: Duration::from_secs(5),
                stall_timeout: Duration::from_secs(5),
                poll_interval: Duration::from_millis(10),
                backoff: [Duration::ZERO; 3],
            },
        );
        let folder = root.path().join("gone");
        let mut removed = 0;
        let outcome = fetcher.fetch_request(&request, &CancelToken::new(), |_| {
            if folder.exists() {
                fs::remove_dir_all(&folder).unwrap();
                removed += 1;
            }
        });
        match outcome {
            Err(FetchError::Io { path, source }) => {
                assert_eq!(path, folder.join("asset.bin.part"));
                assert_eq!(source.kind(), io::ErrorKind::NotFound);
            }
            other => panic!("a removed folder gave {other:?}"),
        }
        assert_eq!(removed, 1);
        assert!(!folder.exists());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sha256_hex;

    fn scratch() -> tempfile::TempDir {
        let base = Path::new(env!("CARGO_MANIFEST_DIR")).join("tmp/testscratch");
        fs::create_dir_all(&base).unwrap();
        tempfile::Builder::new()
            .prefix("fetch-unit-")
            .tempdir_in(base)
            .unwrap()
    }

    fn kind(err: &AttemptError) -> &'static str {
        match err {
            AttemptError::Cancelled => "cancelled",
            AttemptError::Offline => "offline",
            AttemptError::Stalled => "stalled",
            AttemptError::Http(_) => "http",
            AttemptError::Io(_) => "io",
            AttemptError::Superseded => "superseded",
        }
    }

    fn ok<T>(result: Result<T, AttemptError>) -> T {
        match result {
            Ok(value) => value,
            Err(err) => panic!("unexpected attempt error: {}", kind(&err)),
        }
    }

    fn request<'a>(sha256: &'a str) -> RequestRef<'a> {
        RequestRef {
            url: "http://127.0.0.1:9/asset",
            dest_rel: "asset.bin",
            sha256,
            size_hint: None,
        }
    }

    fn headers(pairs: &[(&'static str, &'static str)]) -> ureq::http::HeaderMap {
        let mut map = ureq::http::HeaderMap::new();
        for (name, value) in pairs {
            map.insert(*name, ureq::http::HeaderValue::from_static(value));
        }
        map
    }

    fn assert_refused(fetcher: &Fetcher, dest_rel: &str) {
        match fetcher.contained_dest(dest_rel) {
            Err(FetchError::Io { path, source }) => {
                assert_eq!(path, PathBuf::from(dest_rel));
                assert_eq!(source.kind(), io::ErrorKind::InvalidInput);
            }
            other => panic!("{dest_rel:?} was not refused: {other:?}"),
        }
    }

    #[test]
    fn contained_dest_skips_cur_dir_and_keeps_nested_paths_inside_root() {
        let root = PathBuf::from("/srv/root");
        let fetcher = Fetcher::new(&root);
        assert_eq!(
            fetcher.contained_dest("./a/./b.bin").unwrap(),
            root.join("a").join("b.bin")
        );
        let nested = fetcher.contained_dest("models/en/base.bin").unwrap();
        assert_eq!(nested, root.join("models/en/base.bin"));
        assert!(nested.starts_with(&root));
        assert_eq!(fetcher.contained_dest("a.bin").unwrap(), root.join("a.bin"));
    }

    #[test]
    fn contained_dest_refuses_empty_parent_and_absolute_paths() {
        let fetcher = Fetcher::new("/srv/root");
        for dest_rel in [
            "",
            ".",
            "./.",
            "../x",
            "a/../b",
            "a/b/..",
            "/etc/passwd",
            "/",
        ] {
            assert_refused(&fetcher, dest_rel);
        }
    }

    #[test]
    fn parse_content_range_total_reads_the_total_after_the_slash() {
        assert_eq!(parse_content_range_total("bytes 0-9/10"), Some(10));
        assert_eq!(parse_content_range_total("bytes */4096"), Some(4096));
        assert_eq!(parse_content_range_total("bytes 0-9/ 77 "), Some(77));
        assert_eq!(parse_content_range_total("bytes 0-9/*"), None);
        assert_eq!(parse_content_range_total("bytes 0-9/abc"), None);
        assert_eq!(parse_content_range_total("bytes 0-9/-5"), None);
        assert_eq!(parse_content_range_total(""), None);
    }

    #[test]
    fn total_size_prefers_a_valid_content_range_on_206() {
        let map = headers(&[
            ("content-range", "bytes 100-199/1000"),
            ("content-length", "100"),
        ]);
        assert_eq!(total_size(&map, 206, 100, Some(5)), Some(1000));
    }

    #[test]
    fn total_size_falls_back_to_content_length_when_the_range_is_malformed() {
        let map = headers(&[
            ("content-range", "bytes 100-199/abc"),
            ("content-length", "100"),
        ]);
        assert_eq!(total_size(&map, 206, 100, Some(5)), Some(200));
        let star = headers(&[
            ("content-range", "bytes 100-199/*"),
            ("content-length", "100"),
        ]);
        assert_eq!(total_size(&star, 206, 100, None), Some(200));
    }

    #[test]
    fn total_size_with_unknown_range_and_no_length_uses_the_hint() {
        let map = headers(&[("content-range", "bytes 100-199/*")]);
        assert_eq!(total_size(&map, 206, 100, Some(5)), Some(5));
        assert_eq!(total_size(&map, 206, 100, None), None);
    }

    #[test]
    fn total_size_on_200_ignores_content_range_and_adds_the_offset() {
        let map = headers(&[("content-range", "bytes 0-9/77"), ("content-length", "50")]);
        assert_eq!(total_size(&map, 200, 30, Some(5)), Some(80));
        assert_eq!(total_size(&map, 200, 0, None), Some(50));
    }

    #[test]
    fn total_size_saturates_a_hostile_content_length_on_a_resume() {
        let map = headers(&[("content-length", "18446744073709551615")]);
        assert_eq!(total_size(&map, 206, 100, None), Some(u64::MAX));
        let range = headers(&[
            ("content-range", "bytes 100-199/*"),
            ("content-length", "18446744073709551600"),
        ]);
        assert_eq!(total_size(&range, 206, 100, Some(5)), Some(u64::MAX));
    }

    #[test]
    fn total_size_without_usable_headers_returns_the_hint() {
        assert_eq!(total_size(&headers(&[]), 200, 0, Some(42)), Some(42));
        assert_eq!(total_size(&headers(&[]), 200, 0, None), None);
        let bad = headers(&[("content-length", "lots")]);
        assert_eq!(total_size(&bad, 200, 0, Some(9)), Some(9));
    }

    #[test]
    fn byte_cap_doubles_hints_floors_them_and_saturates() {
        assert_eq!(byte_cap(None), ABSOLUTE_BYTE_CAP);
        assert_eq!(byte_cap(Some(0)), MIN_HINT_CAP);
        assert_eq!(byte_cap(Some(1)), MIN_HINT_CAP);
        assert_eq!(byte_cap(Some(MIN_HINT_CAP / 2)), MIN_HINT_CAP);
        assert_eq!(byte_cap(Some(MIN_HINT_CAP)), 2 * MIN_HINT_CAP);
        assert_eq!(byte_cap(Some(u64::MAX / 2 + 1)), u64::MAX);
        assert_eq!(byte_cap(Some(u64::MAX)), u64::MAX);
    }

    #[test]
    fn part_path_of_appends_part_without_touching_the_extension() {
        assert_eq!(
            part_path_of(Path::new("a/b.bin")),
            PathBuf::from("a/b.bin.part")
        );
        assert_eq!(
            part_path_of(Path::new("x.tar.gz")),
            PathBuf::from("x.tar.gz.part")
        );
        assert_eq!(
            part_path_of(Path::new("/r/noext")),
            PathBuf::from("/r/noext.part")
        );
    }

    #[test]
    fn classify_call_error_maps_connect_and_resolve_to_offline_and_other_timeouts_to_stalled() {
        use ureq::Timeout;
        for timeout in [Timeout::Connect, Timeout::Resolve] {
            assert_eq!(
                kind(&classify_call_error(ureq::Error::Timeout(timeout))),
                "offline"
            );
        }
        for timeout in [
            Timeout::Global,
            Timeout::PerCall,
            Timeout::SendRequest,
            Timeout::Await100,
            Timeout::SendBody,
            Timeout::RecvResponse,
            Timeout::RecvBody,
        ] {
            assert_eq!(
                kind(&classify_call_error(ureq::Error::Timeout(timeout))),
                "stalled"
            );
        }
        assert_eq!(
            kind(&classify_call_error(ureq::Error::HostNotFound)),
            "offline"
        );
        assert_eq!(
            kind(&classify_call_error(ureq::Error::ConnectionFailed)),
            "offline"
        );
    }

    #[test]
    fn sleep_interruptible_zero_returns_at_once_unless_cancelled() {
        let started = Instant::now();
        assert!(sleep_interruptible(Duration::ZERO, &CancelToken::new()).is_ok());
        assert!(started.elapsed() < Duration::from_millis(500));
        let cancel = CancelToken::new();
        cancel.cancel();
        assert!(matches!(
            sleep_interruptible(Duration::ZERO, &cancel),
            Err(FetchError::Cancelled)
        ));
    }

    #[test]
    fn sleep_interruptible_waits_the_full_duration() {
        let started = Instant::now();
        assert!(sleep_interruptible(Duration::from_millis(120), &CancelToken::new()).is_ok());
        assert!(started.elapsed() >= Duration::from_millis(120));
    }

    #[test]
    fn sleep_interruptible_cancelled_before_returns_without_sleeping() {
        let cancel = CancelToken::new();
        cancel.cancel();
        let started = Instant::now();
        assert!(matches!(
            sleep_interruptible(Duration::from_secs(30), &cancel),
            Err(FetchError::Cancelled)
        ));
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn sleep_interruptible_cancelled_during_stops_well_before_the_full_duration() {
        let cancel = CancelToken::new();
        let canceller = cancel.clone();
        let started = Instant::now();
        let handle = thread::spawn(move || {
            thread::sleep(Duration::from_millis(100));
            canceller.cancel();
        });
        let outcome = sleep_interruptible(Duration::from_secs(30), &cancel);
        handle.join().unwrap();
        assert!(matches!(outcome, Err(FetchError::Cancelled)));
        assert!(started.elapsed() >= Duration::from_millis(100));
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    struct Scripted(Vec<io::Result<Vec<u8>>>);

    impl Read for Scripted {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            if self.0.is_empty() {
                return Ok(0);
            }
            let bytes = self.0.remove(0)?;
            buf[..bytes.len()].copy_from_slice(&bytes);
            Ok(bytes.len())
        }
    }

    #[test]
    fn hash_into_retries_interrupted_reads_and_hashes_everything() {
        let reader = Scripted(vec![
            Err(io::ErrorKind::Interrupted.into()),
            Ok(b"hello ".to_vec()),
            Err(io::ErrorKind::Interrupted.into()),
            Ok(b"world".to_vec()),
        ]);
        let mut hasher = Sha256::new();
        assert_eq!(hash_into(reader, &mut hasher).unwrap(), 11);
        assert_eq!(digest_hex(&hasher.finalize()), sha256_hex(b"hello world"));
    }

    #[test]
    fn hash_into_returns_the_reader_error() {
        let reader = Scripted(vec![
            Ok(b"partial".to_vec()),
            Err(io::Error::other("disk gone")),
            Ok(b"never".to_vec()),
        ]);
        let err = hash_into(reader, &mut Sha256::new()).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::Other);
        assert_eq!(err.to_string(), "disk gone");
    }

    #[test]
    fn verify_completed_reports_missing_wrong_and_right_files() {
        let dir = scratch();
        let final_path = dir.path().join("done.bin");
        let right = sha256_hex(b"payload");

        match verify_completed(request(&right), &final_path) {
            Err(FetchError::Io { path, source }) => {
                assert_eq!(path, final_path);
                assert_eq!(source.kind(), io::ErrorKind::NotFound);
            }
            other => panic!("missing file gave {other:?}"),
        }

        fs::write(&final_path, b"tampered").unwrap();
        match verify_completed(request(&right), &final_path) {
            Err(FetchError::ChecksumMismatch { expected, got }) => {
                assert_eq!(expected, right);
                assert_eq!(got, sha256_hex(b"tampered"));
            }
            other => panic!("wrong bytes gave {other:?}"),
        }

        fs::write(&final_path, b"payload").unwrap();
        assert_eq!(
            verify_completed(request(&right), &final_path).unwrap(),
            final_path
        );
    }

    #[test]
    fn resume_on_a_missing_or_empty_part_starts_over() {
        let dir = scratch();
        let part = dir.path().join("a.bin.part");
        let cancel = CancelToken::new();
        assert!(ok(PartWriter::resume(&part, &cancel)).is_none());
        fs::write(&part, b"").unwrap();
        assert!(ok(PartWriter::resume(&part, &cancel)).is_none());
        assert!(part.exists());
    }

    #[test]
    fn resume_on_a_part_with_bytes_rehashes_them() {
        let dir = scratch();
        let part = dir.path().join("a.bin.part");
        let bytes: Vec<u8> = (0..CHUNK_SIZE as u32 + 4097)
            .map(|i| (i % 251) as u8)
            .collect();
        fs::write(&part, &bytes).unwrap();
        let writer = ok(PartWriter::resume(&part, &CancelToken::new())).expect("resumed");
        assert_eq!(writer.len, bytes.len() as u64);
        assert_eq!(writer.path, part.as_path());
        assert_eq!(digest_hex(&writer.hasher.finalize()), sha256_hex(&bytes));
    }

    #[test]
    fn resume_on_a_directory_is_an_io_error() {
        let dir = scratch();
        let part = dir.path().join("a.bin.part");
        fs::create_dir(&part).unwrap();
        match PartWriter::resume(&part, &CancelToken::new()) {
            Err(err) => assert_eq!(kind(&err), "io"),
            Ok(_) => panic!("a directory resumed as a part"),
        }
    }

    #[test]
    fn create_truncates_an_existing_part() {
        let dir = scratch();
        let part = dir.path().join("a.bin.part");
        fs::write(&part, b"stale bytes").unwrap();
        let writer = ok(PartWriter::create(&part, &CancelToken::new()));
        assert_eq!(writer.len, 0);
        assert_eq!(fs::metadata(&part).unwrap().len(), 0);
        assert_eq!(digest_hex(&writer.hasher.finalize()), sha256_hex(b""));
    }

    #[test]
    fn write_skips_empty_buffers_and_appends_the_rest() {
        let dir = scratch();
        let part = dir.path().join("a.bin.part");
        let cancel = CancelToken::new();
        let mut writer = ok(PartWriter::create(&part, &cancel));
        ok(writer.write(&[], &cancel));
        assert_eq!(writer.len, 0);
        ok(writer.write(b"abc", &cancel));
        ok(writer.write(&[], &cancel));
        ok(writer.write(b"de", &cancel));
        assert_eq!(writer.len, 5);
        assert_eq!(fs::read(&part).unwrap(), b"abcde");
        assert_eq!(digest_hex(&writer.hasher.finalize()), sha256_hex(b"abcde"));
    }

    #[test]
    fn write_after_the_part_was_replaced_is_superseded_and_leaves_it_alone() {
        let dir = scratch();
        let part = dir.path().join("a.bin.part");
        let cancel = CancelToken::new();
        let mut writer = ok(PartWriter::create(&part, &cancel));
        ok(writer.write(b"ours1", &cancel));
        let other = dir.path().join("other");
        fs::write(&other, b"their").unwrap();
        fs::rename(&other, &part).unwrap();
        match writer.write(b"ours2", &cancel) {
            Err(err) => assert_eq!(kind(&err), "superseded"),
            Ok(()) => panic!("wrote into a replaced part"),
        }
        assert_eq!(writer.len, 5);
        assert_eq!(fs::read(&part).unwrap(), b"their");
    }

    #[test]
    fn with_part_lock_runs_the_op_on_our_part() {
        let dir = scratch();
        let part = dir.path().join("a.bin.part");
        fs::write(&part, b"12345").unwrap();
        let file = File::open(&part).unwrap();
        let cancel = CancelToken::new();
        assert_eq!(
            ok(with_part_lock(&file, &part, None, &cancel, |_| Ok(7))),
            7
        );
        assert_eq!(
            ok(with_part_lock(&file, &part, Some(5), &cancel, |_| Ok(8))),
            8
        );
        let failed = with_part_lock(&file, &part, Some(5), &cancel, |_| {
            Err::<(), _>(io::Error::other("op failed"))
        });
        assert!(matches!(failed, Err(AttemptError::Io(ref e)) if e.to_string() == "op failed"));
        let second = File::open(&part).unwrap();
        assert!(second.try_lock().is_ok());
    }

    #[test]
    fn with_part_lock_is_superseded_when_the_part_was_replaced() {
        let dir = scratch();
        let part = dir.path().join("a.bin.part");
        fs::write(&part, b"12345").unwrap();
        let file = File::open(&part).unwrap();
        let second = File::open(&part).unwrap();
        let other = dir.path().join("other");
        fs::write(&other, b"12345").unwrap();
        fs::rename(&other, &part).unwrap();
        let mut ran = false;
        let outcome = with_part_lock(&file, &part, None, &CancelToken::new(), |_| {
            ran = true;
            Ok(())
        });
        assert!(matches!(outcome, Err(AttemptError::Superseded)));
        assert!(!ran);
        assert!(second.try_lock().is_ok());
    }

    #[test]
    fn with_part_lock_is_superseded_when_the_length_moved_or_the_part_is_gone() {
        let dir = scratch();
        let part = dir.path().join("a.bin.part");
        fs::write(&part, b"123").unwrap();
        let file = File::open(&part).unwrap();
        let cancel = CancelToken::new();
        let mut ran = false;
        let moved = with_part_lock(&file, &part, Some(5), &cancel, |_| {
            ran = true;
            Ok(())
        });
        assert!(matches!(moved, Err(AttemptError::Superseded)));
        fs::remove_file(&part).unwrap();
        let gone = with_part_lock(&file, &part, None, &cancel, |_| {
            ran = true;
            Ok(())
        });
        assert!(matches!(gone, Err(AttemptError::Superseded)));
        assert!(!ran);
    }

    #[test]
    fn acquire_part_lock_takes_a_free_lock() {
        let dir = scratch();
        let part = dir.path().join("a.bin.part");
        fs::write(&part, b"x").unwrap();
        let file = File::open(&part).unwrap();
        assert!(acquire_part_lock(&file, &CancelToken::new()).is_ok());
        let second = File::open(&part).unwrap();
        assert!(matches!(second.try_lock(), Err(TryLockError::WouldBlock)));
    }

    #[test]
    fn acquire_part_lock_held_elsewhere_then_cancelled_returns_cancelled_promptly() {
        let dir = scratch();
        let part = dir.path().join("a.bin.part");
        fs::write(&part, b"x").unwrap();
        let holder = File::open(&part).unwrap();
        holder.lock().unwrap();
        let file = File::open(&part).unwrap();
        let cancel = CancelToken::new();
        let canceller = cancel.clone();
        let handle = thread::spawn(move || {
            thread::sleep(Duration::from_millis(100));
            canceller.cancel();
        });
        let started = Instant::now();
        let outcome = acquire_part_lock(&file, &cancel);
        handle.join().unwrap();
        assert!(matches!(outcome, Err(LockError::Cancelled)));
        assert!(started.elapsed() < Duration::from_secs(2));

        let mut ran = false;
        let via_with = with_part_lock(&file, &part, None, &cancel, |_| {
            ran = true;
            Ok(())
        });
        assert!(matches!(via_with, Err(AttemptError::Cancelled)));
        assert!(!ran);
        assert!(matches!(
            AttemptError::from(LockError::Io(io::Error::other("lock"))),
            AttemptError::Io(_)
        ));
    }

    fn written_part<'a>(part: &'a Path, bytes: &[u8]) -> PartWriter<'a> {
        let cancel = CancelToken::new();
        let mut writer = ok(PartWriter::create(part, &cancel));
        ok(writer.write(bytes, &cancel));
        writer
    }

    #[test]
    fn verify_and_rename_on_a_match_lands_the_file_and_drops_the_part() {
        let dir = scratch();
        let fetcher = Fetcher::new(dir.path());
        let final_path = dir.path().join("asset.bin");
        let part = part_path_of(&final_path);
        let digest = sha256_hex(b"the payload");
        let writer = written_part(&part, b"the payload");
        let landed = fetcher
            .verify_and_rename(request(&digest), writer, &final_path, &CancelToken::new())
            .unwrap();
        assert_eq!(landed, Some(final_path.clone()));
        assert_eq!(fs::read(&final_path).unwrap(), b"the payload");
        assert!(!part.exists());
    }

    #[test]
    fn verify_and_rename_on_a_mismatch_removes_the_part() {
        let dir = scratch();
        let fetcher = Fetcher::new(dir.path());
        let final_path = dir.path().join("asset.bin");
        let part = part_path_of(&final_path);
        let expected = sha256_hex(b"what we wanted");
        let writer = written_part(&part, b"what we got");
        match fetcher.verify_and_rename(
            request(&expected),
            writer,
            &final_path,
            &CancelToken::new(),
        ) {
            Err(FetchError::ChecksumMismatch { expected: e, got }) => {
                assert_eq!(e, expected);
                assert_eq!(got, sha256_hex(b"what we got"));
            }
            other => panic!("mismatch gave {other:?}"),
        }
        assert!(!part.exists());
        assert!(!final_path.exists());
    }

    #[test]
    fn verify_and_rename_on_a_vanished_part_returns_none() {
        let dir = scratch();
        let fetcher = Fetcher::new(dir.path());
        let final_path = dir.path().join("asset.bin");
        let part = part_path_of(&final_path);
        let digest = sha256_hex(b"bytes");
        let writer = written_part(&part, b"bytes");
        fs::remove_file(&part).unwrap();
        let outcome = fetcher
            .verify_and_rename(request(&digest), writer, &final_path, &CancelToken::new())
            .unwrap();
        assert_eq!(outcome, None);
        assert!(!final_path.exists());
    }

    #[test]
    fn verify_and_rename_when_cancelled_on_a_held_lock_is_cancelled_and_keeps_the_part() {
        let dir = scratch();
        let fetcher = Fetcher::new(dir.path());
        let final_path = dir.path().join("asset.bin");
        let part = part_path_of(&final_path);
        let digest = sha256_hex(b"bytes");
        let writer = written_part(&part, b"bytes");
        let holder = File::open(&part).unwrap();
        holder.lock().unwrap();
        let cancel = CancelToken::new();
        cancel.cancel();
        assert!(matches!(
            fetcher.verify_and_rename(request(&digest), writer, &final_path, &cancel),
            Err(FetchError::Cancelled)
        ));
        assert_eq!(fs::read(&part).unwrap(), b"bytes");
        assert!(!final_path.exists());
    }

    #[test]
    fn verify_and_rename_onto_a_non_empty_directory_is_an_io_error_on_the_part() {
        let dir = scratch();
        let fetcher = Fetcher::new(dir.path());
        let final_path = dir.path().join("asset.bin");
        fs::create_dir(&final_path).unwrap();
        fs::write(final_path.join("occupant"), b"x").unwrap();
        let part = part_path_of(&final_path);
        let digest = sha256_hex(b"bytes");
        let writer = written_part(&part, b"bytes");
        match fetcher.verify_and_rename(request(&digest), writer, &final_path, &CancelToken::new())
        {
            Err(FetchError::Io { path, .. }) => assert_eq!(path, part),
            other => panic!("rename onto a directory gave {other:?}"),
        }
        assert_eq!(fs::read(&part).unwrap(), b"bytes");
        assert!(final_path.join("occupant").exists());
    }

    #[test]
    fn verify_and_rename_when_the_destination_folder_was_removed_returns_none() {
        let dir = scratch();
        let fetcher = Fetcher::new(dir.path());
        let folder = dir.path().join("gone");
        fs::create_dir(&folder).unwrap();
        let final_path = folder.join("asset.bin");
        let part = part_path_of(&final_path);
        let digest = sha256_hex(b"bytes");
        let writer = written_part(&part, b"bytes");
        fs::remove_dir_all(&folder).unwrap();
        let outcome = fetcher
            .verify_and_rename(request(&digest), writer, &final_path, &CancelToken::new())
            .unwrap();
        assert_eq!(outcome, None);
        assert!(!folder.exists());
    }
}
