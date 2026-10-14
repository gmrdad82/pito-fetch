---
title: Formats
description: The files pito-fetch writes and the strings and values it promises, for tools that read them or clean up after it.
sidebar:
  order: 50
---

pito-fetch writes no JSON and no metadata files. What it promises is the shape of the files it leaves on disk and of the values it hands back.

## Files on disk

For a `Fetcher` made with folder `<root>` and a request with `dest_rel` `<rel>`:

| Path | What it is |
|---|---|
| `<root>/<rel>` | the verified file. It appears only by an atomic rename, after its sha256 matched; it is never written in place. |
| `<root>/<rel>.part` | the bytes of a download in progress or interrupted. The name is the destination's full name with `.part` appended (`model.bin` → `model.bin.part`). |

- Subfolders of `<rel>` are created as needed; nothing else is written in them.
- The part file holds the first bytes of the file, in order, with nothing before or after: its length is how many bytes were received, and the next download resumes at that offset.
- While a download writes to the part file, it holds an OS file lock on it (`flock` on Linux and macOS). A tool that reads a part file should take a shared lock first, or expect it to change.
- A part file whose download failed its sha256 check is deleted. A part file left by a cancel, a crash or a lost connection stays until the next download of the same destination resumes it.
- Deleting a part file is always safe when no download of it is running: the next download starts from zero.

## The sha256 string

`Request.sha256`, `FetchSpec.sha256`, the result of `sha256_hex` and `StreamHasher::finalize_hex`, and the `expected` and `got` fields of `FetchError::ChecksumMismatch` are all the same format: the SHA-256 digest as 64 lowercase hexadecimal characters, with no prefix and no whitespace. The comparison is an exact string match, so a sum in uppercase never matches.

## The destination path

`dest_rel` is a relative path whose parts are all plain names (or `.`); it may not be empty, absolute, contain `..`, or carry a Windows drive or UNC prefix. Anything else fails with `FetchError::Io`, kind `InvalidInput`, before any request or write.

## Progress values

`FetchProgress` carries three `u64`s and an option:

| Field | Unit | Meaning |
|---|---|---|
| `received` | bytes | bytes in the part file, including those from an earlier, resumed attempt |
| `total` | bytes | the whole file's size from `Content-Range` or `Content-Length`, else the request's `size_hint`, else `None` |
| `speed_bps` | bytes a second | average speed of the current request since it started sending |

It is delivered on the calling thread, on the first bytes of a response and then at most every 100 ms.

## Error text

The `Display` text of each `FetchError` is in [Handle errors](../guides/handle-errors.md). It is meant for people; match on the enum's variants and fields in code, not on the text.
