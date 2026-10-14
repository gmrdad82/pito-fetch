---
title: Handle errors
description: Every FetchError, when it happens, what is left on disk, and what to do next.
sidebar:
  order: 13
---

Every failure is one `FetchError`. It implements `std::error::Error` and `Display`, so `?` and `eprintln!("{err}")` work as they are.

| Error | When | Left on disk | What to do |
|---|---|---|---|
| `Offline { url }` | the host can't be resolved or reached, or the connection failed before an answer | the part file, if any | try again later; it resumes |
| `Stalled { url }` | no byte arrived for the stall timeout (30 s by default) | the part file | try again; it resumes |
| `Http { status, url }` | the server answered with a status other than 200 or 206 | the part file, if any | a 404 or 403 is the URL's fault; a 5xx may pass |
| `ChecksumMismatch { expected, got }` | the downloaded file's sha256 isn't the one asked for | nothing | the file or the sum is wrong; retrying gets the same bytes |
| `Cancelled` | the `CancelToken` was cancelled | the part file | run the request again to resume |
| `Io { path, source }` | a file couldn't be written, renamed or read, or the destination isn't a plain relative path | depends on `path` | look at `source`; `InvalidInput` means a bad `dest_rel` |

The destination itself is never left half-written by any of them: it is either the previous file, if there was one, or the new verified file.

## Matching on them

```rust
use fetch::FetchError;

match fetcher.fetch_request(&request, &cancel, |_| {}) {
    Ok(path) => println!("ready: {}", path.display()),
    Err(FetchError::Offline { .. } | FetchError::Stalled { .. }) => {
        println!("no connection; the download resumes next time");
    }
    Err(FetchError::Http { status: 500..=599, .. }) => println!("the server is down; try later"),
    Err(FetchError::ChecksumMismatch { expected, got }) => {
        eprintln!("wrong file: wanted {expected}, got {got}");
    }
    Err(FetchError::Cancelled) => {}
    Err(err) => eprintln!("{err}"),
}
```

## What each one prints

The `Display` text of each (the `get` example prints it after `get: `):

```
offline: could not reach http://127.0.0.1:8479/demo.bin
http 404 from http://127.0.0.1:8471/nope.bin
checksum mismatch: expected 0000…0000, got adfffd73f4828f23fcc2dd03f8a91eca4f332e554b0dd8706afbd5cfb77a6295
fetch cancelled
io error at ../escape.bin: dest_rel must be a plain relative path inside dest_root
```

`Stalled` prints `stalled: no bytes received for 30s from <url>`; the 30 s in that text is fixed, even when [`Timings`](timeouts-and-limits.md) sets another stall timeout.

## Retried for you

Only I/O errors while the body streams (a reset connection, a broken pipe) are retried inside one call: three times, after 1, 4 and 9 s, each resuming from the part file. Everything in the table is final for that call. Retrying later is yours to decide; the part file makes it cheap.
