---
title: Timeouts and limits
description: The folder a fetcher can write to, its timeouts and retries, redirects, and the byte cap that stops an endless body.
sidebar:
  order: 15
---

pito-fetch is built for files named by a manifest or a server you don't fully trust. Every download is contained and bounded.

## Contained to its folder

`dest_rel` is a plain relative path inside the folder the `Fetcher` was made for. An absolute path, a `..` anywhere, a Windows drive prefix or an empty path is refused with `FetchError::Io` (kind `InvalidInput`) before anything is written or sent:

```rust
let request = Request { dest_rel: "../escape.bin".into(), ..request };
let err = fetcher.fetch_request(&request, &cancel, |_| {}).unwrap_err();
assert_eq!(err.to_string(), "io error at ../escape.bin: dest_rel must be a plain relative path inside dest_root");
```

Subfolders like `embed/v2/model.bin` are fine and created as needed.

## Timings

| Setting | Default | What it bounds |
|---|---|---|
| `connect_timeout` | 10 s | reaching the host |
| `stall_timeout` | 30 s | sending the request, waiting for the answer, and any gap between body bytes; also how long a [follower](two-downloads-of-one-file.md) waits on a quiet leader |
| `poll_interval` | 200 ms | how often a quiet body read wakes up to check for a cancel or a stall |
| `backoff` | 1, 4, 9 s | the waits before the three retries of an I/O error mid-body |

`Fetcher::with_timings` sets them all:

```rust
use std::time::Duration;

use fetch::{Fetcher, Timings};

let fetcher = Fetcher::with_timings(
    "models",
    Timings {
        stall_timeout: Duration::from_secs(10),
        ..Timings::default()
    },
);
```

A slow download that keeps receiving bytes never stalls: the stall timeout counts silence, not total time. One response body may take up to 24 hours.

## Redirects

Up to 10 redirects are followed. A longer chain, or a loop, fails without writing anything.

## The byte cap

A server that sends more than it should can't fill the disk. The download stops at:

- **twice `size_hint`**, but at least 1 MiB, when the request gives a hint;
- **8 GiB** without one.

The bytes up to the cap are then verified like any other download, so a capped body fails with `ChecksumMismatch` and its part file is deleted. Give a `size_hint` whenever you know the size, so the cap sits close to it.

## Blocking, one thread a download

There is no async runtime: each call blocks its thread until it returns. Memory stays flat whatever the file's size, a 64 KiB buffer for the body and the same for hashing; see [Speed and memory](speed-and-memory.md).
