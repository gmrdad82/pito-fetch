---
title: Cancel a download
description: Stop a running download from another thread with a CancelToken, and resume it later.
sidebar:
  order: 12
---

A `CancelToken` is a shared flag. Clone it, hand the clone to whoever may stop the download (a UI button, a shutdown handler, a timer), and call `cancel()` there. The download notices within about 200 ms and returns `FetchError::Cancelled`.

```rust
use std::thread;
use std::time::Duration;

use fetch::{CancelToken, FetchError, Fetcher, Request};

let fetcher = Fetcher::new("models");
let cancel = CancelToken::new();

let stopper = cancel.clone();
thread::spawn(move || {
    thread::sleep(Duration::from_secs(5));
    stopper.cancel();
});

match fetcher.fetch_request(&request, &cancel, |_| {}) {
    Ok(path) => println!("{}", path.display()),
    Err(FetchError::Cancelled) => println!("stopped; run it again to resume"),
    Err(err) => eprintln!("{err}"),
}
```

## What a cancel keeps

The part file stays, so the next call with the same request [resumes](resume-and-verify.md) where this one stopped. To throw the bytes away instead, delete `<destination>.part` yourself after the call returns.

A cancel is noticed wherever the call waits on the server or the disk: while reading the body, between retries, and while it waits for [another download of the same file](two-downloads-of-one-file.md).

## A token is one-way

`cancel()` can't be undone, and every clone sees it, including `is_cancelled()`. A call made with a token that is already cancelled returns `Cancelled` at once without touching the network or the disk. Make a fresh `CancelToken::new()` for the next download.

One token can stop many downloads at once: pass clones of it to each call, as in [Download a file](download-a-file.md#many-downloads).
