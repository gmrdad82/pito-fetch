---
title: Try it locally
description: Serve a file on your own machine and download it with the get example, to watch a resume, a mismatch and a race.
sidebar:
  order: 17
---

The repository carries two small programs built on the library: `get`, a downloader with a progress line, and `serve`, a local file server with `Range` support at a set speed. Everything below runs on Linux, macOS or WSL, with no network beyond 127.0.0.1.

## Build them

```sh
git clone https://github.com/gmrdad82/pito-fetch && cd pito-fetch
cargo build --release --examples
export PATH="$PWD/target/release/examples:$PATH"
```

## serve

```
usage: serve <DIR> <PORT> [BYTES_PER_SECOND]
```

It serves the files in `DIR` on `127.0.0.1:PORT`, answers `Range` requests with `206`, and sends at most `BYTES_PER_SECOND` when given, so a download lasts long enough to stop it. Make a file and start it in the background:

```sh
mkdir -p tmp/files && head -c 50000000 /dev/urandom > tmp/files/demo.bin
serve tmp/files 8471 10000000 &
sum=$(sha256sum tmp/files/demo.bin | cut -d' ' -f1)   # macOS: shasum -a 256
cd tmp
```

## get

```
usage: get <URL> <SHA256> <DEST>
```

It downloads `URL` to `DEST`, pinned to `SHA256`, and prints the path on success. On a terminal it draws a progress line on stderr; piped, it prints only the result. It exits 0 on success, 1 when the download fails and 2 on wrong arguments.

```sh
$ get http://127.0.0.1:8471/demo.bin $sum models/demo.bin
models/demo.bin
```

## Things to try

**Stop and resume.** Press Ctrl-C halfway, look at `ls models` (a `demo.bin.part`), and run the same line again: the bar picks up where it stopped. [Resume and verify](resume-and-verify.md) explains it.

**A wrong sum.** Any other 64 hex characters:

```sh
$ get http://127.0.0.1:8471/demo.bin $(printf '0%.0s' $(seq 64)) models/copy.bin
get: checksum mismatch: expected 0000000000000000000000000000000000000000000000000000000000000000, got adfffd73f4828f23fcc2dd03f8a91eca4f332e554b0dd8706afbd5cfb77a6295
$ echo $?
1
```

Your `got` differs: the file is random. `ls models` shows no `copy.bin` and no part file.

**A missing file and a dead server.**

```sh
$ get http://127.0.0.1:8471/nope.bin $sum models/x.bin
get: http 404 from http://127.0.0.1:8471/nope.bin
$ get http://127.0.0.1:8479/demo.bin $sum models/x.bin
get: offline: could not reach http://127.0.0.1:8479/demo.bin
```

**Two at once.** Remove `models/demo.bin` and start two downloads of it together; both print the path, and there is one verified file ([Two downloads of one file](two-downloads-of-one-file.md)):

```sh
$ rm models/demo.bin
$ ( get http://127.0.0.1:8471/demo.bin $sum models/demo.bin & get http://127.0.0.1:8471/demo.bin $sum models/demo.bin; wait )
models/demo.bin
models/demo.bin
```

**Stop the server mid-download.** While `get` runs, stop `serve` from another terminal (`pkill -f 'serve tmp/files'`). The broken connection is retried after a second, the server is gone, and `get` fails, keeping the part file:

```
get: offline: could not reach http://127.0.0.1:8471/demo.bin
```

Start `serve` again and rerun `get` to finish from where it stopped.

When you're done, `kill %1` stops the server.
