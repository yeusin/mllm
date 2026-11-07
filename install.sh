#!/usr/bin/env sh
# Multi-LLM (mllm) Installation Script
# Supports Linux and macOS (x86_64, aarch64 / Apple Silicon)
#
# Threat Model Note:
# Checksum verification provides transport integrity and corruption detection against
# the official GitHub release assets. Archive extraction occurs unprivileged within an
# isolated temporary directory prior to installing the binary.

set -eu

REPO="${REPO:-yeusin/mllm}"
BASE_URL="${BASE_URL:-https://github.com/${REPO}/releases}"
VERSION="${VERSION:-latest}"
BIN_DIR="${BIN_DIR:-}"
LOCAL_BUILD="${LOCAL_BUILD:-0}"

show_help() {
  cat << 'EOF'
Multi-LLM (mllm) Installer

Usage: install.sh [OPTIONS]

Options:
  -b, --bin-dir <DIR>    Target directory for installation [default: /usr/local/bin or ~/.local/bin]
  -v, --version <VER>    Specify release version to install (e.g. v0.1.0) [default: latest]
  --local                Install from local release build or compile via cargo
  -h, --help             Show this help message

Environment Variables:
  BIN_DIR                Same as --bin-dir
  VERSION                Same as --version
  REPO                   GitHub repository [default: yeusin/mllm]
  LOCAL_BUILD            Set to 1 to prefer local binary/cargo build
EOF
}

# Parse options
while [ $# -gt 0 ]; do
  case "$1" in
    -b|--bin-dir)
      if [ $# -lt 2 ]; then
        echo "Error: --bin-dir requires a directory argument." >&2
        exit 1
      fi
      BIN_DIR="$2"
      shift 2
      ;;
    -v|--version)
      if [ $# -lt 2 ]; then
        echo "Error: --version requires a version argument." >&2
        exit 1
      fi
      VERSION="$2"
      shift 2
      ;;
    --local)
      LOCAL_BUILD=1
      shift
      ;;
    -h|--help)
      show_help
      exit 0
      ;;
    *)
      echo "Error: Unknown option: $1" >&2
      echo "Run 'install.sh --help' for usage." >&2
      exit 1
      ;;
  esac
done

# Detect operating system
OS="$(uname -s)"
case "$OS" in
  Linux)
    OS_FAMILY="unknown-linux-gnu"
    ;;
  Darwin)
    OS_FAMILY="apple-darwin"
    ;;
  *)
    echo "Error: Unsupported operating system: $OS" >&2
    echo "mllm officially supports Linux and macOS." >&2
    exit 1
    ;;
esac

# Detect architecture
ARCH="$(uname -m)"
case "$ARCH" in
  x86_64|amd64)
    TARGET_ARCH="x86_64"
    ;;
  arm64|aarch64)
    TARGET_ARCH="aarch64"
    ;;
  *)
    echo "Error: Unsupported CPU architecture: $ARCH" >&2
    echo "mllm supports x86_64 and aarch64 architectures." >&2
    exit 1
    ;;
esac

TARGET="${TARGET_ARCH}-${OS_FAMILY}"

# Temporary directory with automatic cleanup
TMP_DIR="$(mktemp -d 2>/dev/null || mktemp -d -t 'mllm_install')"
cleanup() {
  rm -rf "$TMP_DIR"
}
trap cleanup EXIT
trap 'exit 1' INT TERM

download() {
  _url="$1"
  _dest="$2"
  if command -v curl >/dev/null 2>&1; then
    curl -fsSL "$_url" -o "$_dest"
  elif command -v wget >/dev/null 2>&1; then
    wget -q "$_url" -O "$_dest"
  else
    echo "Error: Neither curl nor wget was found on your system." >&2
    return 1
  fi
}

compute_sha256() {
  _file="$1"
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$_file" | awk '{print $1}'
  elif command -v shasum >/dev/null 2>&1; then
    shasum -a 256 "$_file" | awk '{print $1}'
  elif command -v openssl >/dev/null 2>&1; then
    openssl dgst -sha256 "$_file" | awk '{print $NF}'
  else
    return 1
  fi
}

verify_checksum() {
  _archive="$1"
  _checksum_url="$2"
  _checksum_file="$TMP_DIR/checksum.sha256"

  if download "$_checksum_url" "$_checksum_file" 2>/dev/null; then
    EXPECTED_HASH="$(awk '{print $1}' "$_checksum_file")"
    if ACTUAL_HASH="$(compute_sha256 "$_archive")"; then
      if [ "$EXPECTED_HASH" != "$ACTUAL_HASH" ]; then
        echo "Error: SHA-256 checksum verification failed." >&2
        echo "Expected: $EXPECTED_HASH" >&2
        echo "Actual:   $ACTUAL_HASH" >&2
        exit 1
      fi
      echo "Checksum verified: SHA-256 matches ($ACTUAL_HASH)."
    else
      echo "Warning: No sha256 tool found; skipping checksum verification." >&2
    fi
  else
    echo "Warning: Checksum file unavailable at $_checksum_url; skipping checksum verification." >&2
  fi
}

BINARY=""

if [ "$LOCAL_BUILD" -eq 0 ]; then
  if [ "$VERSION" = "latest" ]; then
    DOWNLOAD_URL="${BASE_URL}/latest/download/mllm-${TARGET}.tar.gz"
    CHECKSUM_URL="${BASE_URL}/latest/download/mllm-${TARGET}.tar.gz.sha256"
  else
    case "$VERSION" in
      v*) TAG="$VERSION" ;;
      *)  TAG="v$VERSION" ;;
    esac
    DOWNLOAD_URL="${BASE_URL}/download/${TAG}/mllm-${TARGET}.tar.gz"
    CHECKSUM_URL="${BASE_URL}/download/${TAG}/mllm-${TARGET}.tar.gz.sha256"
  fi

  echo "Attempting to download mllm (${TARGET}) from GitHub releases..."
  if download "$DOWNLOAD_URL" "$TMP_DIR/mllm.tar.gz" 2>/dev/null; then
    verify_checksum "$TMP_DIR/mllm.tar.gz" "$CHECKSUM_URL"
    tar -xzf "$TMP_DIR/mllm.tar.gz" -C "$TMP_DIR"
    EXTRACTED_BIN="$(find "$TMP_DIR" -type f -name mllm | head -n 1)"
    if [ -n "$EXTRACTED_BIN" ] && [ -f "$EXTRACTED_BIN" ]; then
      BINARY="$EXTRACTED_BIN"
    fi
  else
    # On macOS, if architecture-specific binary download fails, attempt universal binary
    if [ "$OS_FAMILY" = "apple-darwin" ]; then
      UNIVERSAL_URL="${BASE_URL}/latest/download/mllm-universal-apple-darwin.tar.gz"
      if [ "$VERSION" != "latest" ]; then
        UNIVERSAL_URL="${BASE_URL}/download/${TAG}/mllm-universal-apple-darwin.tar.gz"
      fi
      if download "$UNIVERSAL_URL" "$TMP_DIR/mllm.tar.gz" 2>/dev/null; then
        verify_checksum "$TMP_DIR/mllm.tar.gz" "${UNIVERSAL_URL}.sha256"
        tar -xzf "$TMP_DIR/mllm.tar.gz" -C "$TMP_DIR"
        EXTRACTED_BIN="$(find "$TMP_DIR" -type f -name mllm | head -n 1)"
        if [ -n "$EXTRACTED_BIN" ] && [ -f "$EXTRACTED_BIN" ]; then
          BINARY="$EXTRACTED_BIN"
        fi
      fi
    fi
  fi
fi

SCRIPT_DIR="$(cd "$(dirname "$0")" 2>/dev/null && pwd || echo ".")"

# Fallback: local release binary or cargo build if remote download is unavailable
if [ -z "$BINARY" ]; then
  if [ -f "${SCRIPT_DIR}/target/release/mllm" ]; then
    echo "Notice: Pre-built release not found online. Using existing local binary '${SCRIPT_DIR}/target/release/mllm'."
    BINARY="${SCRIPT_DIR}/target/release/mllm"
  elif [ -f "./target/release/mllm" ]; then
    echo "Notice: Pre-built release not found online. Using existing local binary './target/release/mllm'."
    BINARY="./target/release/mllm"
  elif [ -f "${SCRIPT_DIR}/Cargo.toml" ] && command -v cargo >/dev/null 2>&1; then
    echo "Notice: Pre-built release not found online. Compiling from local repository using cargo..."
    (cd "$SCRIPT_DIR" && cargo build --release)
    BINARY="${SCRIPT_DIR}/target/release/mllm"
  elif [ -f "./Cargo.toml" ] && command -v cargo >/dev/null 2>&1; then
    echo "Notice: Pre-built release not found online. Compiling from local repository using cargo..."
    cargo build --release
    BINARY="./target/release/mllm"
  else
    echo "Error: Failed to download mllm from:" >&2
    echo "  ${DOWNLOAD_URL:-https://github.com/${REPO}/releases}" >&2
    echo "" >&2
    echo "If no release assets have been published yet for version '${VERSION}'," >&2
    echo "you can build and install mllm from source:" >&2
    echo "  git clone https://github.com/${REPO}.git" >&2
    echo "  cd mllm" >&2
    echo "  ./install.sh --local" >&2
    exit 1
  fi
fi

# Determine target directory
if [ -z "$BIN_DIR" ]; then
  if [ -w "/usr/local/bin" ]; then
    BIN_DIR="/usr/local/bin"
  elif [ "$(id -u)" -eq 0 ]; then
    BIN_DIR="/usr/local/bin"
  else
    BIN_DIR="${HOME}/.local/bin"
  fi
fi

mkdir -p "$BIN_DIR" 2>/dev/null || {
  if command -v sudo >/dev/null 2>&1; then
    sudo mkdir -p "$BIN_DIR"
  else
    echo "Error: Cannot create directory $BIN_DIR (permission denied)." >&2
    exit 1
  fi
}

TARGET_BIN="${BIN_DIR}/mllm"
echo "Installing mllm to ${TARGET_BIN}..."

if [ -w "$BIN_DIR" ] && ( [ ! -e "$TARGET_BIN" ] || [ -w "$TARGET_BIN" ] ); then
  cp "$BINARY" "$TARGET_BIN"
  chmod 755 "$TARGET_BIN"
else
  if command -v sudo >/dev/null 2>&1; then
    echo "Elevated permissions required to install into ${BIN_DIR}."
    sudo cp "$BINARY" "$TARGET_BIN"
    sudo chmod 755 "$TARGET_BIN"
  else
    echo "Error: Cannot write to ${TARGET_BIN} and sudo is not available." >&2
    exit 1
  fi
fi

# Verification
if [ -x "$TARGET_BIN" ]; then
  INSTALLED_VERSION="$("$TARGET_BIN" --version 2>/dev/null || echo "mllm")"
  echo "Successfully installed ${INSTALLED_VERSION} to ${TARGET_BIN}"
else
  echo "Error: Installation failed; ${TARGET_BIN} is not executable." >&2
  exit 1
fi

# PATH advice
case ":$PATH:" in
  *:"$BIN_DIR":*)
    ;;
  *)
    echo ""
    echo "Notice: '${BIN_DIR}' is not in your current PATH."
    echo "To run mllm from anywhere, add it to your shell configuration (~/.bashrc, ~/.zshrc, etc.):"
    echo "  export PATH=\"\$PATH:${BIN_DIR}\""
    echo ""
    ;;
esac
