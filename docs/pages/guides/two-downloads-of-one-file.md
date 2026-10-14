---
title: Two downloads of one file
description: What happens when two threads or two processes download the same file at the same moment.
sidebar:
  order: 14
---

![A terminal: two get commands start the same download at once; both print models/model.bin, ls shows one model.bin and sha256sum prints the expected sum.](../images/together.gif)

Two fetchers can ask for the same destination at once: two threads of one app, or two copies of an app started together. You don't need a lock of your own; both calls end with the same verified file.

```sh
$ ( get $url $sum models/model.bin & get $url $sum models/model.bin; wait )
models/model.bin
models/model.bin
$ ls models
model.bin
```

## How it works

- **One writer at a time.** The part file is locked (an OS file lock) for every write, so two writers never interleave bytes.
- **A writer that loses its place steps back.** Before each write a writer checks that the part file is still the one it opened and still the length it left it at. If the other download wrote bytes it didn't write, or replaced or removed the part file, it stops writing and follows the other one.
- **The follower waits, then checks.** It watches the part file. When the part file goes away, the other download has landed, and the follower hashes the destination itself: a match returns the path, a mismatch returns `ChecksumMismatch`.
- **A quiet leader is taken over.** If the part file doesn't grow for the stall timeout (30 s by default), the follower resumes the download itself from the bytes already there.
- **Verify under the lock.** The final hash and rename happen while holding the lock, so nothing mixed or half-written ever reaches the destination.

A follower stays cancellable: its `CancelToken` stops the wait the same way it stops a download.

## Same file, different sums

Two requests for one destination with different sha256s can't both succeed: at least one fails, and its cleanup can make the other fail too. Nothing unverified reaches the destination either way. Give each sum its own destination.
