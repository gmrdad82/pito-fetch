---
title: Download a file
description: Describe a download with a Request, run it with a Fetcher, and follow its progress.
sidebar:
  order: 10
---

A download is three things: a `Request` that says what to get, a `Fetcher` that says where files go, and a call that blocks until the file has landed verified or failed.

## The request

```rust
use fetch::Request;

let request = Request {
    url: "https://example.com/models/model.bin".into(),
    dest_rel: "embed/model.bin".into(),
    sha256: "9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08".into(),
    size_hint: Some(91_000_000),
};
```

- `url`: HTTP or HTTPS. Up to 10 redirects are followed.
- `dest_rel`: where the file goes, relative to the fetcher's folder. Subfolders are created as needed. An absolute path, a `..` or an empty path is refused before anything is written.
- `sha256`: the file's sha256 in lowercase hex, 64 characters. It is compared as a string, so uppercase never matches; lowercase it first if your source writes it in capitals.
- `size_hint`: the size you expect, if you know it. It sets the byte cap (twice the hint, at least 1 MiB) so a server can't send an endless body; with `None` the cap is 8 GiB.

## The fetcher

```rust
use fetch::Fetcher;

let fetcher = Fetcher::new("/var/lib/myapp/models");
```

A `Fetcher` holds the destination folder and an HTTP agent. Make one and reuse it for every download into that folder; it is safe to share between threads by reference. `Fetcher::with_timings` changes the timeouts, see [Timeouts and limits](timeouts-and-limits.md).

## The call

```rust
use fetch::CancelToken;

let cancel = CancelToken::new();
let path = fetcher.fetch_request(&request, &cancel, |p| {
    match p.total {
        Some(total) => eprintln!("{} / {} bytes at {} B/s", p.received, total, p.speed_bps),
        None => eprintln!("{} bytes at {} B/s", p.received, p.speed_bps),
    }
})?;
assert_eq!(path, std::path::Path::new("/var/lib/myapp/models/embed/model.bin"));
```

`fetch_request` returns the final path once the file's sha256 has matched and it has been renamed into place. Every failure is a `FetchError`; [Handle errors](handle-errors.md) lists them.

The progress callback runs on the calling thread, on the first bytes and then about every 100 ms while bytes arrive:

- `received`: bytes in the part file so far, a resumed download's earlier bytes included.
- `total`: the whole file's size when the server sends a length, else your `size_hint`, else `None`.
- `speed_bps`: the average speed in bytes a second since this request started sending.

Pass `|_| {}` when you don't need it.

## Many downloads

The call is blocking, one thread per download, with no async runtime. To download several files at once, run each call on its own thread with the same fetcher:

```rust
std::thread::scope(|s| {
    for request in &requests {
        let fetcher = &fetcher;
        let cancel = cancel.clone();
        s.spawn(move || fetcher.fetch_request(request, &cancel, |_| {}));
    }
});
```

A file that is already in place is downloaded again: pito-fetch doesn't skip a destination that exists. Check for it yourself first if your files don't change.
