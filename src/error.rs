use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum FetchError {
    #[error("offline: could not reach {url}")]
    Offline { url: String },
    #[error("stalled: no bytes received for 30s from {url}")]
    Stalled { url: String },
    #[error("checksum mismatch: expected {expected}, got {got}")]
    ChecksumMismatch { expected: String, got: String },
    #[error("http {status} from {url}")]
    Http { status: u16, url: String },
    #[error("fetch cancelled")]
    Cancelled,
    #[error("io error at {path}: {source}")]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
}
