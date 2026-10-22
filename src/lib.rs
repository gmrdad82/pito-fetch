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

    #[test]
    fn stream_hasher_default_matches_new() {
        let data = payload(10_000);
        let mut from_default = StreamHasher::default();
        let mut from_new = StreamHasher::new();
        from_default.update(&data);
        from_new.update(&data);
        assert_eq!(from_default.finalize_hex(), from_new.finalize_hex());
    }

    #[test]
    fn sha256_hex_of_empty_input_is_the_known_digest() {
        let empty = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
        assert_eq!(sha256_hex(b""), empty);
        assert_eq!(StreamHasher::new().finalize_hex(), empty);
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn digest_hex_is_lowercase_and_zero_padded() {
        assert_eq!(digest_hex(&[0x00, 0x0a, 0xff, 0xab]), "000affab");
        assert_eq!(digest_hex(&[]), "");
    }

    fn payload(len: usize) -> Vec<u8> {
        (0..len)
            .map(|i| ((i as u32).wrapping_mul(2_654_435_761) >> 16) as u8)
            .collect()
    }
}
