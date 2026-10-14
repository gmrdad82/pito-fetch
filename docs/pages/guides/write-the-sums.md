---
title: Write the sha256 sums
description: Make the sha256 sums your downloads are pinned to, and keep a fixed list of downloads in a static manifest.
sidebar:
  order: 16
---

Every download is pinned to a sha256 you know before it starts. You make that sum once, from the file you publish, and ship it with your app or your manifest.

## From a shell

```sh
$ sha256sum model.bin            # Linux and WSL
$ shasum -a 256 model.bin        # macOS
9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08  model.bin
```

Both print the lowercase hex pito-fetch compares against; take the first 64 characters.

## From Rust

The library writes the same lowercase hex. For bytes in memory:

```rust
let sum = fetch::sha256_hex(b"test");
assert_eq!(sum, "9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08");
```

For a file or a stream, feed it in pieces with `StreamHasher`, so memory stays flat:

```rust
use std::io::Read;

use fetch::StreamHasher;

fn sum_of(path: &std::path::Path) -> std::io::Result<String> {
    let mut file = std::fs::File::open(path)?;
    let mut hasher = StreamHasher::new();
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            return Ok(hasher.finalize_hex());
        }
        hasher.update(&buf[..n]);
    }
}
```

## A static manifest

When the files an app needs are known when it is built, keep them as `FetchSpec` constants. A `FetchSpec` holds the same four fields as a `Request`, as `&'static str`, and goes to `fetcher.fetch(…)`:

```rust
use fetch::{CancelToken, FetchSpec, Fetcher};

const MODELS: &[FetchSpec] = &[
    FetchSpec {
        url: "https://example.com/models/encoder-v2.bin",
        dest_rel: "encoder-v2.bin",
        sha256: "9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08",
        size_hint: Some(91_000_000),
    },
];

fn fetch_models(fetcher: &Fetcher, cancel: &CancelToken) -> Result<(), fetch::FetchError> {
    for spec in MODELS {
        fetcher.fetch(spec, cancel, |_| {})?;
    }
    Ok(())
}
```

Putting the version in the file name, as `encoder-v2.bin` does, lets a new version download beside the old one, so the old file stays usable until the new one has landed verified.
