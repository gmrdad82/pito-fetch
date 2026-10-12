# Changelog

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
