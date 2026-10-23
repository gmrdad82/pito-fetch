---
title: Speed and memory
description: How fast pito-fetch downloads and verifies, how much memory it takes, how it compares with curl and wget, and how to measure it yourself.
sidebar:
  order: 18
---

pito-fetch downloads and verifies in one pass of the network and one read of the file, in flat memory. These are the numbers, how they were measured, and how to measure them on your own machine.

## Download and verify

![Chart, download and verify in milliseconds, lower is better: 1 GiB takes pito-fetch 592 ms, curl plus sha256sum 814 ms, wget plus sha256sum 743 ms; 100 MiB takes 71, 87 and 80 ms.](../images/download-verify-paired.png)

Downloading a file and checking its sha256 takes pito-fetch less time than curl or wget followed by `sha256sum`: 592 ms against 814 and 743 ms on a 1 GiB file, 71 against 87 and 80 ms on 100 MiB. Each chunk is hashed as it is written, so the file is never read back. That is over loopback, where the network costs nothing and the client is all that's measured; over a real connection the network's speed dominates all three.

## Peak memory

![Chart, peak memory in MiB for a 1 GiB download and verify, lower is better: pito-fetch 7.9, curl plus sha256sum 9.9, wget plus sha256sum 8.3.](../images/peak-memory-paired.png)

Memory stays flat whatever the file's size: bytes stream through a 1 MiB buffer to disk and into the sha256 in the same pass.

## Size

| | |
|---|---|
| Library code | 739 lines of Rust |
| Dependencies | 3 direct (ureq, sha2, thiserror), 35 crates in all |
| Clean release build | 11.7 s |
| The `get` example | 3.4 MB as built, 2.5 MB stripped, TLS and root certificates included |

## How it was measured

Once on 2026-10-16, on Linux with an AMD Ryzen 9 9950X (16 cores, 32 threads) and an NVMe disk, with Rust 1.97.1. The machine was otherwise idle (load average about 2), and the clients ran interleaved run by run.

- **Download and verify:** the `serve` example serves a file of random bytes on 127.0.0.1; the `get` example (download and sha256 check in one process), `curl -o` (curl 8.22) and `wget -O` (wget 1.25), each of the last two followed by `sha256sum`, are timed seven times each, interleaved. The chart shows the medians: 1 GiB in 592, 814 and 743 ms; 100 MiB in 71, 87 and 80 ms. In the same session pito-fetch 1.0.1, which read the file back after the download to hash it and locked the part file for every 64 KiB, took 802 and 83 ms.
- **Peak memory:** the peak resident set (`ru_maxrss`) of each process for the 1 GiB file: `get` 7.9 MiB; curl 9.9 MiB, wget 8.3 MiB and `sha256sum` 8.3 MiB, so the larger of each pair is charted.
- **Lines:** the library's `src/`, without its unit tests, blank lines or comments.
- **Dependencies:** `cargo tree`, build-time macros included.
- **Build:** `cargo build --release --locked` from a clean target folder.
- **Binary:** the `get` example from `cargo build --release --examples`, before and after `strip`.

The charts are drawn by [pito-compare](https://docs.pitomd.com/pito-compare/) from the data files in the repository's [`docs/charts/`](https://github.com/gmrdad82/pito-fetch/tree/{{version}}/docs/charts).

## Measure it yourself

`bin/bench` in the repository runs the download-and-verify comparison: it builds the examples, serves a 1 GiB and a 100 MiB file of random bytes on port 8472, and times `get`, then curl and wget each followed by the sha256 check, interleaved, printing the median of each. A client missing from the `PATH` is skipped; it needs `perl` for the clock. The optional argument is the number of runs, 5 by default.

```sh
git clone https://github.com/gmrdad82/pito-fetch && cd pito-fetch
bin/bench 5
```

It prints one line per client and size, `<client> <MiB> MiB median <ms> ms of <runs> runs`. The test files stay in `tmp/bench/files` for the next run.
