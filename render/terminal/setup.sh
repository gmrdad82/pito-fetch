#!/bin/sh
set -eu
mkdir -p "$2/files"
cd "$2"
yes 'PITO MD fetch: atomic, resumable, verified.' | head -c 16000000 > files/model.bin
sha256sum files/model.bin | cut -d' ' -f1 > model.bin.sha256
nohup "$1/serve" files 8471 4000000 > /dev/null 2>&1 &
echo $! > serve.pid
sleep 0.3
