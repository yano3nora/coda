#!/bin/sh
# coda bootstrap installer
#
# Usage:
#   curl -fsSL https://raw.githubusercontent.com/yano3nora/coda/main/scripts/bootstrap.sh | sh
#   curl -fsSL https://raw.githubusercontent.com/yano3nora/coda/main/scripts/bootstrap.sh | sh -s -- 0.1.0
#
# Env overrides:
#   CODA_INSTALL_DIR     install directory (default: $HOME/.local/bin)
#   CODA_SKIP_CHECKSUM   set to 1 to allow install without checksum
#                        verification (checksum failures are fatal otherwise)
#
# Written in POSIX sh (no bashisms) so it also works under dash / busybox sh,
# which is what most minimal SSH targets and containers ship. This is the
# primary "SSH into a server or docker container and edit files" bootstrap
# path (docs/TASK-260712-mouse-verify-inactive-ssh.md), so it must not assume
# bash, jq, or any package manager is present.
set -eu

REPO="yano3nora/coda"

log() {
    echo "coda bootstrap: $1"
}

fail() {
    echo "coda bootstrap: error: $1" >&2
    exit 1
}

# Explicit guard instead of letting `set -u` abort cryptically: minimal
# containers sometimes run without HOME (e.g. docker exec as a raw uid).
if [ -n "${CODA_INSTALL_DIR:-}" ]; then
    INSTALL_DIR="$CODA_INSTALL_DIR"
elif [ -n "${HOME:-}" ]; then
    INSTALL_DIR="$HOME/.local/bin"
else
    fail "HOME is not set; set CODA_INSTALL_DIR to choose an install directory"
fi

usage() {
    cat <<'EOF'
coda bootstrap: install coda from GitHub Releases

Usage:
  curl -fsSL https://raw.githubusercontent.com/yano3nora/coda/main/scripts/bootstrap.sh | sh
  curl -fsSL https://raw.githubusercontent.com/yano3nora/coda/main/scripts/bootstrap.sh | sh -s -- <version>

Arguments:
  <version>   Release version to install, e.g. "0.1.0" or "v0.1.0".
              Defaults to the latest release.

Environment:
  CODA_INSTALL_DIR     Install directory (default: $HOME/.local/bin)
  CODA_SKIP_CHECKSUM   Set to 1 to allow install without checksum verification

Options:
  -h, --help  Show this help and exit
EOF
}

# Detect a HTTP client once and remember which one, since curl and wget take
# different flags for "silent, follow redirects" and "headers only".
detect_downloader() {
    if command -v curl >/dev/null 2>&1; then
        DOWNLOADER="curl"
    elif command -v wget >/dev/null 2>&1; then
        DOWNLOADER="wget"
    else
        fail "neither curl nor wget is available; install one and retry"
    fi
}

# fetch_to_file URL DEST
fetch_to_file() {
    url="$1"
    dest="$2"
    if [ "$DOWNLOADER" = "curl" ]; then
        curl -fsSL "$url" -o "$dest" || fail "download failed: $url"
    else
        wget -q -O "$dest" "$url" || fail "download failed: $url"
    fi
}

# resolve_latest_version: GitHub has no unauthenticated, jq-free JSON API
# call we want to depend on here, but /releases/latest 302-redirects to the
# tagged release URL. Reading the Location header (without following it)
# gives us the tag name with no JSON parsing required.
resolve_latest_version() {
    latest_url="https://github.com/$REPO/releases/latest"
    if [ "$DOWNLOADER" = "curl" ]; then
        location=$(curl -fsSI "$latest_url" | tr -d '\r' | grep -i '^location:' | tail -n 1)
    else
        # wget has no head-only redirect probe as simple as curl -I;
        # --max-redirect=0 makes it stop at the first redirect and print it.
        location=$(wget -q -S --max-redirect=0 "$latest_url" -O /dev/null 2>&1 | grep -i '^  location:' | tail -n 1)
    fi
    [ -n "$location" ] || fail "could not resolve latest release (no redirect location found)"

    version=$(echo "$location" | sed -n 's#.*/tag/v\{0,1\}\([0-9][^[:space:]]*\).*#\1#p')
    [ -n "$version" ] || fail "could not parse version from redirect: $location"
    echo "$version"
}

# detect_os: map `uname -s` to the release asset naming scheme.
detect_os() {
    case "$(uname -s)" in
        Darwin) echo "macos" ;;
        Linux) echo "linux" ;;
        *) fail "unsupported OS: $(uname -s) (coda only ships macos/linux binaries)" ;;
    esac
}

# detect_arch: map `uname -m` to the release asset naming scheme.
detect_arch() {
    case "$(uname -m)" in
        x86_64 | amd64) echo "x64" ;;
        aarch64 | arm64) echo "arm64" ;;
        *) fail "unsupported architecture: $(uname -m)" ;;
    esac
}

# verify_checksum FILE SHA256_FILE
# Fail-closed: an installer that runs whatever it downloaded must not treat a
# missing checksum as a warning. `CODA_SKIP_CHECKSUM=1` is the explicit
# opt-out for containers that lack both sha256sum and shasum.
verify_checksum() {
    file="$1"
    sha_file="$2"
    dir=$(dirname "$file")
    base=$(basename "$file")

    if command -v sha256sum >/dev/null 2>&1; then
        ( cd "$dir" && sha256sum -c "$(basename "$sha_file")" >/dev/null ) \
            || fail "checksum verification failed for $base"
    elif command -v shasum >/dev/null 2>&1; then
        ( cd "$dir" && shasum -a 256 -c "$(basename "$sha_file")" >/dev/null ) \
            || fail "checksum verification failed for $base"
    else
        checksum_or_fail "neither sha256sum nor shasum is available"
        return 0
    fi
    log "checksum verified"
}

# checksum_or_fail REASON: shared fail-closed gate for every "cannot verify"
# path (tool missing, .sha256 download failed).
checksum_or_fail() {
    if [ "${CODA_SKIP_CHECKSUM:-0}" = "1" ]; then
        log "warning: $1; continuing because CODA_SKIP_CHECKSUM=1"
    else
        fail "$1; refusing to install unverified binary (set CODA_SKIP_CHECKSUM=1 to override)"
    fi
}

main() {
    version_arg=""
    for arg in "$@"; do
        case "$arg" in
            -h | --help)
                usage
                exit 0
                ;;
            *)
                version_arg="$arg"
                ;;
        esac
    done

    detect_downloader

    os=$(detect_os)
    arch=$(detect_arch)
    log "detected platform: $os-$arch"

    if [ -n "$version_arg" ]; then
        # Accept both "0.1.0" and "v0.1.0".
        version="${version_arg#v}"
        log "installing pinned version: $version"
    else
        log "resolving latest release..."
        version=$(resolve_latest_version)
        log "latest version: $version"
    fi

    asset="coda-v${version}-${os}-${arch}.tar.gz"
    base_url="https://github.com/$REPO/releases/download/v${version}"
    tarball_url="$base_url/$asset"
    sha_url="$tarball_url.sha256"

    tmp_dir=$(mktemp -d) || fail "mktemp -d failed"
    trap 'rm -rf "$tmp_dir"' EXIT INT TERM

    log "downloading $asset ..."
    fetch_to_file "$tarball_url" "$tmp_dir/$asset"

    log "downloading checksum ..."
    checksum_fetched=1
    if [ "$DOWNLOADER" = "curl" ]; then
        curl -fsSL "$sha_url" -o "$tmp_dir/$asset.sha256" 2>/dev/null || checksum_fetched=0
    else
        # wget leaves an empty/partial file behind on failure; remove it so a
        # failed fetch can never be mistaken for a real checksum file.
        wget -q -O "$tmp_dir/$asset.sha256" "$sha_url" 2>/dev/null \
            || { rm -f "$tmp_dir/$asset.sha256"; checksum_fetched=0; }
    fi
    if [ "$checksum_fetched" = "1" ] && [ -s "$tmp_dir/$asset.sha256" ]; then
        verify_checksum "$tmp_dir/$asset" "$tmp_dir/$asset.sha256"
    else
        checksum_or_fail "could not fetch $asset.sha256"
    fi

    log "extracting ..."
    # Release archives contain exactly one root member. Reject extras before
    # extraction so traversal entries and unexpected payloads are never
    # materialized, even if a tar implementation has permissive defaults.
    archive_members=$(tar -tzf "$tmp_dir/$asset") || fail "could not inspect $asset"
    [ "$archive_members" = "coda" ] || fail "unexpected archive contents; expected only 'coda'"
    tar -xzf "$tmp_dir/$asset" -C "$tmp_dir" coda || fail "failed to extract 'coda' from $asset"
    [ -f "$tmp_dir/coda" ] || fail "binary 'coda' not found at archive root of $asset"
    [ ! -L "$tmp_dir/coda" ] || fail "'coda' in $asset is a symlink; refusing to install"
    chmod +x "$tmp_dir/coda"

    # Run the new binary BEFORE it replaces anything, so a broken download
    # never clobbers a working install; then rename within the install dir
    # (same filesystem) so the swap is atomic.
    if ! "$tmp_dir/coda" --version >/dev/null 2>&1; then
        fail "downloaded binary failed to run: coda --version"
    fi
    version_output=$("$tmp_dir/coda" --version)

    mkdir -p "$INSTALL_DIR" || fail "could not create install dir: $INSTALL_DIR"
    staging="$INSTALL_DIR/.coda.new.$$"
    cp "$tmp_dir/coda" "$staging" || fail "could not write to $INSTALL_DIR"
    chmod +x "$staging"
    mv -f "$staging" "$INSTALL_DIR/coda" || { rm -f "$staging"; fail "could not install to $INSTALL_DIR"; }
    log "installed to $INSTALL_DIR/coda ($version_output)"

    case ":$PATH:" in
        *":$INSTALL_DIR:"*) ;;
        *)
            log "note: $INSTALL_DIR is not in your PATH. Add this to your shell profile:"
            echo "    export PATH=\"$INSTALL_DIR:\$PATH\""
            ;;
    esac
}

main "$@"
