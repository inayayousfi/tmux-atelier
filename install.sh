#!/usr/bin/env bash

# Installs the latest release without modifying tmux configuration.
#
# Usage: ./install.sh [--install-dir DIRECTORY] [--update]
# Default destination: ${XDG_CONFIG_HOME:-$HOME/.config}/tmux/tmux-atelier
# Test/local overrides: TMUX_ATELIER_INSTALL_DIR, TMUX_ATELIER_RELEASE_BASE_URL,
# TMUX_ATELIER_LATEST_VERSION_URL, TMUX_ATELIER_UNAME_S, and TMUX_ATELIER_UNAME_M.

set -euo pipefail

usage() {
    printf 'usage: %s [-d|--install-dir DIRECTORY] [--update]\n' "$0"
}

install_dir=${TMUX_ATELIER_INSTALL_DIR:-${XDG_CONFIG_HOME:-$HOME/.config}/tmux/tmux-atelier}
update=0
state_home=${XDG_STATE_HOME:-${HOME:?HOME is not configured}/.local/state}
state_dir=$state_home/tmux-atelier
state_file=$state_dir/install.state

while (($#)); do
    case $1 in
        --update)
            update=1
            shift
            ;;
        -d | --install-dir)
            if (($# < 2)) || [[ -z $2 ]]; then
                printf '%s requires a directory\n' "$1" >&2
                usage >&2
                exit 2
            fi
            install_dir=$2
            shift 2
            ;;
        --install-dir=*)
            install_dir=${1#*=}
            if [[ -z $install_dir ]]; then
                printf '%s requires a directory\n' "${1%%=*}" >&2
                usage >&2
                exit 2
            fi
            shift
            ;;
        -h | --help)
            usage
            exit 0
            ;;
        *)
            printf 'unknown argument: %s\n' "$1" >&2
            usage >&2
            exit 2
            ;;
    esac
done

if ((update)); then
    if [[ -n ${TMUX_ATELIER_INSTALL_DIR:-} ]]; then
        printf '--update cannot be combined with TMUX_ATELIER_INSTALL_DIR\n' >&2
        exit 2
    fi
    if [[ ! -f $state_file ]]; then
        printf 'installation state not found: %s\n' "$state_file" >&2
        exit 1
    fi
    while IFS='=' read -r key value; do
        case $key in
            install_dir) install_dir=$value ;;
            platform) saved_platform=$value ;;
        esac
    done < "$state_file"
    if [[ -z ${saved_platform:-} || -z ${install_dir:-} ]]; then
        printf 'invalid installation state: %s\n' "$state_file" >&2
        exit 1
    fi
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

if ((update)) && [[ $saved_platform != "$platform" ]]; then
    printf 'installation platform is %s, current platform is %s\n' "$saved_platform" "$platform" >&2
    exit 1
fi

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

version_from_binary() {
    local output=$1 version
    output=$("$output" --version)
    if [[ $output =~ ^tmux-atelier[[:space:]]+([0-9]+\.[0-9]+\.[0-9]+)$ ]]; then
        version=${BASH_REMATCH[1]}
        printf '%s\n' "$version"
    else
        printf 'could not read tmux-atelier version from %s\n' "$1" >&2
        return 1
    fi
}

version_gt() {
    local left=$1 right=$2 left_major left_minor left_patch right_major right_minor right_patch
    [[ $left =~ ^([0-9]+)\.([0-9]+)\.([0-9]+)$ ]] || return 2
    left_major=${BASH_REMATCH[1]}
    left_minor=${BASH_REMATCH[2]}
    left_patch=${BASH_REMATCH[3]}
    [[ $right =~ ^([0-9]+)\.([0-9]+)\.([0-9]+)$ ]] || return 2
    right_major=${BASH_REMATCH[1]}
    right_minor=${BASH_REMATCH[2]}
    right_patch=${BASH_REMATCH[3]}
    ((10#$left_major > 10#$right_major)) ||
        ((10#$left_major == 10#$right_major && 10#$left_minor > 10#$right_minor)) ||
        ((10#$left_major == 10#$right_major && 10#$left_minor == 10#$right_minor && 10#$left_patch > 10#$right_patch))
}

if ((update)); then
    installed_version=$(version_from_binary "$install_dir/bin/tmux-atelier")
    latest_url=${TMUX_ATELIER_LATEST_VERSION_URL:-https://api.github.com/repos/inayayousfi/tmux-atelier/releases/latest}
    latest_json=$(curl -fsSL --retry 3 "$latest_url")
    if [[ $latest_json =~ \"tag_name\"[[:space:]]*:[[:space:]]*\"v([0-9]+\.[0-9]+\.[0-9]+)\" ]]; then
        latest_version=${BASH_REMATCH[1]}
    else
        printf 'could not read latest release version\n' >&2
        exit 1
    fi
    if [[ $installed_version == "$latest_version" ]]; then
        printf 'tmux-atelier %s is already up to date\n' "$installed_version"
        exit 0
    fi
    if ! version_gt "$latest_version" "$installed_version"; then
        printf 'installed version %s is newer than latest release %s\n' "$installed_version" "$latest_version" >&2
        exit 1
    fi
fi

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
installed_version=$(version_from_binary "$stage_dir/bin/tmux-atelier")

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

mkdir -p "$state_dir"
state_tmp=$(mktemp "$state_dir/.install.state.XXXXXX")
printf 'install_dir=%s\nplatform=%s\nversion=%s\n' "$install_dir" "$platform" "$installed_version" > "$state_tmp"
chmod 600 "$state_tmp"
mv "$state_tmp" "$state_file"

printf 'run-shell %s\n' "$(quote_sh "$install_dir/tmux-atelier.tmux")"
