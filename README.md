<p align="center"><img src="docs/images/resume.gif" width="800" alt="A terminal: get downloads model.bin from a local server with a progress bar and is stopped with Ctrl-C at 54%, leaving model.bin.part; the same command again resumes from 54% and prints models/model.bin; with a wrong sha256 it prints a checksum mismatch, and ls shows only model.bin."></p>

# pito-fetch

[![CI](https://github.com/gmrdad82/pito-fetch/actions/workflows/ci.yml/badge.svg)](https://github.com/gmrdad82/pito-fetch/actions/workflows/ci.yml) [![Version](https://img.shields.io/github/v/tag/gmrdad82/pito-fetch)](https://github.com/gmrdad82/pito-fetch/tags)

pito-fetch is a small Rust library for downloads you can trust, part of [PITO](https://pitomd.com). You give it a URL, where the file goes and the file's sha256; it downloads into a `.part` file beside the destination, resumes that file after an interruption, checks the sha256 and only then renames it into place. A file is there whole and verified, or not at all.

Works on Linux and macOS (Windows through WSL).

## What it is

Apps that download models, assets or releases each end up writing the same loop, and the same bugs: a half-written file that looks finished, a restart from zero after a dropped connection, a server that ignores `Range`, two downloads of one file at once, a path from a manifest that walks out of its folder. pito-fetch is that loop, written once:

- **Atomic:** bytes go to `<dest>.part`. When the download ends, the whole part file is hashed, and only a sha256 match renames it to `<dest>`; a mismatch deletes the part and returns `ChecksumMismatch`.
- **Resumable:** a part file left by a crash, a Ctrl-C or a cancel is picked up with a `Range` request. A server that answers `200` or `416` instead gets a fresh download from zero.
- **Safe to race:** the part file is locked while it is written, and a writer that finds its part file replaced or removed stops, so two fetchers of one file never interleave their bytes.
- **Contained:** the destination is a plain relative path inside the folder the `Fetcher` was made for; an absolute path or `..` is refused before anything is written.
- **Bounded:** a 10 s connect timeout, a stall after 30 s without a byte, three retries of I/O errors after 1, 4 and 9 s, at most 10 redirects, and a byte cap (twice the size hint, at least 1 MiB, or 8 GiB without one), so an endless body can't fill the disk. The timings are yours to set with `Timings`.
- **Cancellable:** a `CancelToken` stops a download from another thread within about 200 ms, keeping the part file for a later resume.
- **Progress:** a callback about every 100 ms with the bytes received, the total when the server says it, and the speed.

It is blocking (one thread per download), speaks HTTP and HTTPS through [ureq](https://github.com/algesten/ureq) with rustls and the Mozilla root certificates built in, and needs no OpenSSL or other system library.

## Install

pito-fetch isn't on crates.io. Add it to your `Cargo.toml` from this repository, pinned to a [tag](https://github.com/gmrdad82/pito-fetch/tags):

```toml
[dependencies]
fetch = { git = "https://github.com/gmrdad82/pito-fetch", tag = "v0.1.2" }
```

The package is named `fetch`, so the code says `use fetch::…`. It needs Rust 1.97 or newer, on Linux, macOS, or Windows through WSL.

## Use

```rust
use fetch::{CancelToken, Fetcher, Request};

let fetcher = Fetcher::new("models");
let request = Request {
    url: "https://example.com/model.bin".into(),
    dest_rel: "model.bin".into(),
    sha256: "<the file's sha256, in lowercase hex>".into(),
    size_hint: None,
};
let path = fetcher.fetch_request(&request, &CancelToken::new(), |p| {
    eprintln!("{} of {:?} bytes, {} B/s", p.received, p.total, p.speed_bps);
})?;
```

`path` is `models/model.bin`, verified. For a list of downloads known at compile time, `FetchSpec` holds the same fields as `&'static str` and goes to `fetcher.fetch(…)`. To cancel, clone the `CancelToken`, hand the clone to another thread and call `cancel()` there. `Fetcher::with_timings` sets the connect and stall timeouts, how often a quiet body is checked for a stall or a cancel, and the retry delays.

Every failure is a `FetchError`:

| Error | When |
|---|---|
| `Offline` | the host can't be resolved or reached, or the connection failed before an answer |
| `Stalled` | no byte arrived for the stall timeout (30 s by default) |
| `Http` | the server answered with a status other than 200 or 206 |
| `ChecksumMismatch` | the downloaded file's sha256 isn't the one asked for; nothing is left behind |
| `Cancelled` | the `CancelToken` was cancelled; the part file stays for a resume |
| `Io` | a file couldn't be written, renamed or read, or the destination isn't a plain relative path |

`sha256_hex(&bytes)` and `StreamHasher` give the same lowercase hex digests, for writing the sums in the first place.

### The examples

[`examples/get.rs`](examples/get.rs) is a small downloader on top of the library, the one in the clip above:

```
cargo run --release --example get -- <URL> <SHA256> <DEST>
```

It draws a progress line on a terminal and prints the destination when the file lands. [`examples/serve.rs`](examples/serve.rs) serves a folder on `127.0.0.1` with `Range` support at an optional rate in bytes per second, to try a resume locally:

```
cargo run --release --example serve -- <DIR> <PORT> [BYTES_PER_SECOND]
```

## Contributing

Issues are welcome at [github.com/gmrdad82/pito-fetch/issues](https://github.com/gmrdad82/pito-fetch/issues): a download that went wrong, with the server's answer if you can share it, or something the library should handle. Pull requests aren't taken. Please read the [code of conduct](CODE_OF_CONDUCT.md) first, and report a security issue privately as [SECURITY.md](SECURITY.md) says.

## Build and test

```
cargo build --release
bin/gate
```

`bin/gate` runs what CI runs: ShellCheck where it is installed, rustfmt, clippy with warnings as errors, and the tests, which run real downloads against local servers that die mid-file, lie about the length, corrupt bytes, loop redirects or send endless bodies, with cancels, path traversal and two fetchers racing for one file. CI runs the gate on Linux and macOS on every push and nightly, and a tag publishes its GitHub Release with the [CHANGELOG](CHANGELOG.md)'s notes. The README's clip is a prec shot in `render/terminal.toml`, made a GIF by pconv ([NOTICE](NOTICE)).

## Licence

The code is MIT licensed ([LICENSE](LICENSE)), © Catalin Ilinca. The pito-fetch and PITO names and their logos are © Catalin Ilinca, all rights reserved, and are not covered by the MIT licence ([TRADEMARKS.md](TRADEMARKS.md)).
