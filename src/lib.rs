mod cancel;
mod error;
mod fetcher;
mod poll;
mod spec;

use std::fmt::Write as _;

use sha2::{Digest, Sha256};

pub use cancel::CancelToken;
pub use error::FetchError;
pub use fetcher::{Fetcher, Timings};
pub use spec::{FetchProgress, FetchSpec, Request};

pub fn sha256_hex(data: &[u8]) -> String {
    digest_hex(&Sha256::digest(data))
}

pub struct StreamHasher(Sha256);

impl StreamHasher {
    pub fn new() -> Self {
        StreamHasher(Sha256::new())
    }

    pub fn update(&mut self, data: &[u8]) {
        self.0.update(data);
    }

    pub fn finalize_hex(self) -> String {
        digest_hex(&self.0.finalize())
    }
}

pub(crate) fn digest_hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        write!(out, "{byte:02x}").expect("writing to a String is infallible");
    }
    out
}

impl Default for StreamHasher {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stream_hasher_matches_whole_input_digest() {
        let data = payload(500_000);
        let mut streamed = StreamHasher::new();
        for chunk in data.chunks(4096) {
            streamed.update(chunk);
        }
        assert_eq!(streamed.finalize_hex(), sha256_hex(&data));
    }

    fn payload(len: usize) -> Vec<u8> {
        (0..len)
            .map(|i| ((i as u32).wrapping_mul(2_654_435_761) >> 16) as u8)
            .collect()
    }
}
