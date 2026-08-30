#!/bin/sh

set -eu

ROOT=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)
if [ -n "${TMUX_ATELIER_CLI:-}" ]; then
    CLI=$TMUX_ATELIER_CLI
elif [ -x "$ROOT/bin/tmux-atelier" ]; then
    CLI=$ROOT/bin/tmux-atelier
elif [ -x "$ROOT/target/debug/tmux-atelier" ]; then
    CLI=$ROOT/target/debug/tmux-atelier
else
    printf '%s\n' 'tmux-atelier: binary not found; run cargo build or install a release' >&2
    exit 1
fi

exec "$CLI" internal configure "$ROOT" "$CLI"
