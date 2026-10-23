#!/bin/sh
# Install the liftmon release binary and its launcher wrapper system-wide.
#
# Copies the release binary (built beforehand with `cargo build --release`)
# and the wrapper (kept in the repo as wrapper.sh, not executable)
# to /usr/local/bin by default, where the wrapper becomes `liftmon` with the
# execute bit set:
#   /usr/local/bin/liftmon     — the wrapper (the command users run)
#   /usr/local/bin/liftmon-bin — the real binary
# The wrapper defaults the database to ~/.liftmon/db.sqlite, so only an
# installed liftmon ever touches that file; a raw source build still refuses
# to start until LIFTMON_DB_PATH is set.
# If the release binary's `--version` reports a snapshot build (the version
# string contains "snapshot", case-insensitively — e.g. "1.0.0-SNAPSHOT"),
# install.sh asks before installing it.
#
# Build first as your own user (root usually has no cargo), then install as
# root:
#
#     cargo build --release && sudo ./install.sh
#
# Override the destination (and skip the root requirement) with
# BIN_DIR=/some/dir. Uninstall is just removing the two files above.

set -eu
cd "$(dirname "$0")"

release="target/release/liftmon"
if [ ! -f "${release}" ]; then
    echo "release binary not found at ${release} — build it first: cargo build --release" >&2
    exit 1
fi

# Refuse to silently install a snapshot build: when the binary's --version says
# it is one, ask first. A version we cannot read (e.g. a binary built before
# the CLI existed, so `--version` fails) also refuses to install rather than
# guessing. Reads as a decline on EOF, so a non-interactive run never installs
# a snapshot without an explicit yes.
version="$("${release}" --version 2>/dev/null)" || {
    echo "could not read the release binary's version (\"${release} --version\" failed) — build it first: cargo build --release" >&2
    exit 1
}
case "$(printf '%s' "$version" | tr '[:upper:]' '[:lower:]')" in
    *snapshot*)
        printf "%s is a snapshot build (version '%s'). Install it anyway? [y/N] " \
            "${release}" "$version" >&2
        read ans || ans=
        if [ "$ans" != y ] && [ "$ans" != Y ]; then
            echo "aborting; install a non-snapshot release binary instead" >&2
            exit 1
        fi
        ;;
esac

BIN_DIR="${BIN_DIR:-/usr/local/bin}"

# The default destination is root-owned; a custom BIN_DIR is the caller's own
# choice of destination, so root is not assumed there.
if [ "${BIN_DIR}" = "/usr/local/bin" ] && [ "$(id -u)" -ne 0 ]; then
    echo "run as root: sudo ${0##*/} (or set BIN_DIR to a directory you can write)" >&2
    exit 1
fi

# A custom BIN_DIR may not exist yet (e.g. ~/bin); create it so the install
# below never fails on a missing directory.
mkdir -p "${BIN_DIR}"
install -m 0755 "${release}" "${BIN_DIR}/liftmon-bin"
install -m 0755 wrapper.sh "${BIN_DIR}/liftmon"
echo "Installed liftmon and liftmon-bin to ${BIN_DIR}"
