#!/bin/sh
# liftmon — the installed launcher for the liftmon TUI.
#
# A wrapper around the real binary (installed beside it as liftmon-bin) so a
# normally-installed liftmon defaults its database to this user's own
# ~/.liftmon/db.sqlite (the directory is created on first use) instead of
# wherever the working directory happens to be. The binary itself refuses to
# start without LIFTMON_DB_PATH, so this wrapper is the only thing that ever
# supplies that default; if you already set LIFTMON_DB_PATH yourself, that
# path is used untouched and neither it nor ~/.liftmon is touched. Any
# arguments are passed straight through to the binary.

set -eu

# The real binary sits next to this script as liftmon-bin. Resolve symlinks
# so the wrapper also works when reached through a symlinked PATH entry.
BIN="$(dirname "$(readlink -f "$0")")/liftmon-bin"

if [ -z "${LIFTMON_DB_PATH:-}" ]; then
    LIFTMON_DB_PATH="${HOME}/.liftmon/db.sqlite"
    export LIFTMON_DB_PATH
    mkdir -p "$(dirname "$LIFTMON_DB_PATH")"
fi

if [ ! -x "$BIN" ]; then
    echo "liftmon: binary not found at ${BIN} — run install.sh again" >&2
    exit 1
fi

exec "$BIN" "$@"
