#!/bin/sh
if [ -f "$1/serve.pid" ]; then
  kill "$(cat "$1/serve.pid")" 2> /dev/null || true
fi
