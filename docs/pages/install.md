---
title: Install
description: Add pito-fetch to a Rust project on Linux, macOS or WSL, and download your first verified file.
sidebar:
  order: 2
---

pito-fetch is a library, so there is nothing to install on its own: a Rust project depends on it. It isn't on crates.io; add it from its repository, pinned to a [tag](https://github.com/gmrdad82/pito-fetch/tags).

## Linux, macOS and Windows through WSL

The line is the same on every platform. In your `Cargo.toml`:

```toml
[dependencies]
fetch = { git = "https://github.com/gmrdad82/pito-fetch", tag = "{{version}}" }
```

The package is named `fetch`, so the code says `use fetch::…`. It needs:

- **Rust 1.97 or newer** (`rustup update stable`).
- **A C compiler** at build time: ring, the crypto under rustls, compiles a little C and assembly. Linux distributions ship one with their build tools (`build-essential` on Debian and Ubuntu); on macOS, `xcode-select --install`. Nothing links to a system library at run time: TLS and the Mozilla root certificates are built in.
- **Windows:** use it inside WSL, where it is the Linux build. It is built and tested on Linux and macOS only.

## Quick start

```rust
use fetch::{CancelToken, Fetcher, Request};

fn main() -> Result<(), fetch::FetchError> {
    let request = Request {
        url: "https://example.com/model.bin".into(),
        dest_rel: "model.bin".into(),
        sha256: "<the file's sha256, in lowercase hex>".into(),
        size_hint: None,
    };
    let path = Fetcher::new("models").fetch_request(&request, &CancelToken::new(), |p| {
        eprintln!("{} of {:?} bytes, {} B/s", p.received, p.total, p.speed_bps);
    })?;
    println!("{}", path.display());
    Ok(())
}
```

`path` is `models/model.bin`, and its sha256 is the one you asked for. The call blocks until the file has landed or failed; [Download a file](guides/download-a-file.md) goes through each part.

## See it work in 30 seconds

The repository's two examples serve a file locally and download it:

```sh
git clone https://github.com/gmrdad82/pito-fetch && cd pito-fetch
cargo build --release --examples
mkdir -p tmp/files && head -c 50000000 /dev/urandom > tmp/files/demo.bin
target/release/examples/serve tmp/files 8471 10000000 &
sum=$(sha256sum tmp/files/demo.bin | cut -d' ' -f1)   # macOS: shasum -a 256
target/release/examples/get http://127.0.0.1:8471/demo.bin $sum tmp/models/demo.bin
```

```
tmp/models/demo.bin
```

The server sends 10 MB a second, so the 50 MB file takes about five seconds. Press Ctrl-C halfway and run the last line again: it carries on from where it stopped. `kill %1` stops the server. [Try it locally](guides/try-it-locally.md) has more to try with them.
