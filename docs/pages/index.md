---
title: pito-fetch
description: Atomic, resumable HTTP downloads pinned to a sha256, for Rust. A file lands whole and verified, or not at all.
sidebar:
  order: 1
---

![A terminal: get downloads model.bin from a local server with a progress bar and is stopped with Ctrl-C at 54%, leaving model.bin.part; the same command again resumes from 54% and prints models/model.bin; with a wrong sha256 it prints a checksum mismatch, and ls shows only model.bin.](images/resume.gif)

**Downloads you can trust, in one call.** pito-fetch is a small Rust library, part of [PITO MD](https://pitomd.com): give it a URL, where the file goes and the file's sha256. It downloads into a `.part` file, picks that file up again after a crash or a dropped connection, checks the sha256 and only then renames it into place. A file is there whole and verified, or not at all.

| | |
|---|---|
| 1 GiB downloaded and verified | **0.59 s** on loopback, in **7.9 MiB** of memory |
| Library code | **739 lines** of Rust |
| Dependencies | **3** direct (ureq, sha2, thiserror), no OpenSSL or other system library |
| Clean release build | **11.7 s** |
| A downloader built on it | **2.5 MB** stripped, TLS and root certificates included |

[Speed and memory](guides/speed-and-memory.md) says how each number was measured. Works on Linux and macOS (Windows through WSL).

## Start here

- [Install](install.md): add the crate and download your first file.
- [Download a file](guides/download-a-file.md): a `Request`, a `Fetcher` and a progress callback.
- [Resume and verify](guides/resume-and-verify.md): what happens after a crash, a Ctrl-C or a wrong sha256.
- [Cancel a download](guides/cancel-a-download.md): stop it from another thread and resume it later.
- [Handle errors](guides/handle-errors.md): every `FetchError`, and what to do with each.
- [Two downloads of one file](guides/two-downloads-of-one-file.md): threads or processes racing for the same file.
- [Timeouts and limits](guides/timeouts-and-limits.md): containment, timings, retries, redirects and byte caps.
- [Write the sha256 sums](guides/write-the-sums.md): make the sums a manifest pins, and a static manifest with `FetchSpec`.
- [Try it locally](guides/try-it-locally.md): the `get` and `serve` examples.
- [Formats](reference/formats.md): the part file, the sha256 strings and the progress values it promises.
- [API reference](api/fetch/index.html): every public type and function, from `cargo doc`.
- [Changelog](changelog.md).

## Licence

The code is MIT licensed, © Catalin Ilinca. The pito-fetch and PITO MD names and their logos are © Catalin Ilinca, all rights reserved, and are not covered by the MIT licence. Source and issues: [github.com/gmrdad82/pito-fetch](https://github.com/gmrdad82/pito-fetch).
