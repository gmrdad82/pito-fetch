# Changelog

## v1.0.3 — 2026-10-23

### Fixed

- The progress callback now gets one last event when the body ends, so `received` reaches the full size even on downloads faster than 100 ms.
- A resumed download whose server sends a huge `Content-Length` no longer overflows the reported total.

### Changed

- 77 tests now cover the library, up from 13: line coverage is 96% for the fetcher and 100% for the rest.
- The README links to Discord for questions.

## v1.0.2 — 2026-10-16

### Changed

- The README's Licence section shows the PITO MD wordmark.
- The Licence wordmark comes from the brand masters' README export.
- CI saves its caches from main only.
- The README and NOTICE name pcod, which made pconv's GIFs.
- Hash while streaming.

## v1.0.1 — 2026-10-14

### Added

- Each release gains a docs bundle with a guide for each task, the API reference and the changelog.

## v1.0.0 — 2026-10-14

### Added

- `bin/bench` times a download and sha256 check against curl and wget followed by `sha256sum`, to reproduce the README's numbers.

### Changed

- The README shows each feature with a clip or a chart: two downloads of one file, download-and-verify time and peak memory against curl and wget, size and build numbers, and a first use that takes 30 seconds.
- The brand now reads PITO MD in the README, NOTICE and TRADEMARKS.md.

### Fixed

- Two downloads of the same file started at once now both return the verified file. Before, their bytes mixed in the part file and both failed the sha256 check; nothing unverified ever reached the destination.

## v0.1.2 — 2026-10-12

### Added

- The get example, a small downloader with a progress line, and the serve example, a local server with `Range` support at a set rate for trying a resume.
- The README for strangers, led by a clip of a download stopped with Ctrl-C, resumed and verified.
- MIT licence, TRADEMARKS.md, CODE_OF_CONDUCT.md, SECURITY.md and NOTICE.
- CI runs the gate (ShellCheck, rustfmt, clippy with warnings as errors, the tests) on Linux and macOS on every push and nightly, and a tag publishes its GitHub Release from this file.

### Changed

- The integration tests write their scratch inside the repository's own `tmp/`.

## v0.1.1 — 2026-09-28

### Fixed

- Body reads poll again on ureq 3.4.1 and later, so a stall or a cancel is noticed while a body is quiet.

## v0.1.0 — 2026-09-28

### Added

- Atomic, resumable downloads pinned to a sha256: bytes go to a locked `.part` file, resume with `Range`, and land by rename only after the sha256 matches.
- Destinations contained in the fetcher's folder, bounded timeouts, retries and byte caps, a `CancelToken` and a progress callback.
- `sha256_hex` and `StreamHasher` for writing the sums.
