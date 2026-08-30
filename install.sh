#!/usr/bin/env bash

# Installs the latest release without modifying tmux configuration.
#
# Usage: ./install.sh
# Default destination: ${XDG_CONFIG_HOME:-$HOME/.config}/tmux/tmux-atelier
# Test/local overrides: TMUX_ATELIER_INSTALL_DIR, TMUX_ATELIER_RELEASE_BASE_URL,
# TMUX_ATELIER_UNAME_S, and TMUX_ATELIER_UNAME_M.

set -euo pipefail

if (($#)); then
    printf 'usage: %s\n' "$0" >&2
    exit 2
fi

os=${TMUX_ATELIER_UNAME_S:-$(uname -s)}
arch=${TMUX_ATELIER_UNAME_M:-$(uname -m)}

case "$os:$arch" in
    Linux:x86_64)
        platform=linux-x86_64
        ;;
    Linux:aarch64 | Linux:arm64)
        platform=linux-aarch64
        ;;
    Darwin:arm64 | Darwin:aarch64)
        platform=macos-arm64
        ;;
    *)
        printf 'unsupported platform: %s %s\n' "$os" "$arch" >&2
        exit 1
        ;;
esac

install_dir=${TMUX_ATELIER_INSTALL_DIR:-${XDG_CONFIG_HOME:-$HOME/.config}/tmux/tmux-atelier}
release_base=${TMUX_ATELIER_RELEASE_BASE_URL:-https://github.com/inayayousfi/tmux-atelier/releases/latest/download}
archive=tmux-atelier-${platform}.tar.gz
parent=${install_dir%/*}
[[ $parent != "$install_dir" ]] || parent=.
[[ -n $parent ]] || parent=/

mkdir -p "$parent"
download_dir=$(mktemp -d "${TMPDIR:-/tmp}/tmux-atelier.download.XXXXXX")
stage_dir=$(mktemp -d "$parent/.tmux-atelier.install.XXXXXX")
backup_dir=

cleanup() {
    status=$?
    rm -rf "$download_dir"
    [[ ! -d $stage_dir ]] || rm -rf "$stage_dir"
    if [[ -n $backup_dir && -e $backup_dir ]]; then
        if [[ ! -e $install_dir ]]; then
            mv "$backup_dir" "$install_dir"
        else
            rm -rf "$backup_dir"
        fi
    fi
    exit "$status"
}
trap cleanup EXIT HUP INT TERM

quote_sh() {
    local value=${1//\'/\'\\\'\'}
    printf "'%s'" "$value"
}

curl -fsSL --retry 3 --output "$download_dir/$archive" "$release_base/$archive"
curl -fsSL --retry 3 --output "$download_dir/$archive.sha256" "$release_base/$archive.sha256"

read -r expected_checksum _ < "$download_dir/$archive.sha256"
if [[ ! $expected_checksum =~ ^[[:xdigit:]]{64}$ ]]; then
    printf 'invalid checksum file for %s\n' "$archive" >&2
    exit 1
fi

if command -v sha256sum >/dev/null 2>&1; then
    actual_checksum=$(sha256sum "$download_dir/$archive")
    actual_checksum=${actual_checksum%% *}
elif command -v shasum >/dev/null 2>&1; then
    actual_checksum=$(shasum -a 256 "$download_dir/$archive")
    actual_checksum=${actual_checksum%% *}
else
    printf 'sha256sum or shasum is required\n' >&2
    exit 1
fi

if [[ $actual_checksum != "$expected_checksum" ]]; then
    printf 'checksum verification failed for %s\n' "$archive" >&2
    exit 1
fi

tar -xzf "$download_dir/$archive" -C "$stage_dir"
if [[ ! -f $stage_dir/bin/tmux-atelier || -L $stage_dir/bin/tmux-atelier ||
      ! -f $stage_dir/tmux-atelier.tmux || -L $stage_dir/tmux-atelier.tmux ]]; then
    printf 'release archive does not contain bin/tmux-atelier and tmux-atelier.tmux\n' >&2
    exit 1
fi
chmod 755 "$stage_dir/bin/tmux-atelier"

if [[ -e $install_dir || -L $install_dir ]]; then
    backup_dir=$(mktemp -d "$parent/.tmux-atelier.backup.XXXXXX")
    rmdir "$backup_dir"
    mv "$install_dir" "$backup_dir"
fi
mv "$stage_dir" "$install_dir"
stage_dir=

if [[ -n $backup_dir ]]; then
    rm -rf "$backup_dir"
    backup_dir=
fi

printf 'run-shell %s\n' "$(quote_sh "$install_dir/tmux-atelier.tmux")"
