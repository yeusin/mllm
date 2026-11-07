#!/usr/bin/env sh
# Test harness for install.sh
# Verifies POSIX syntax, argument parsing, offline/local fallbacks, and checksum validation.

set -eu

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
INSTALLER="${REPO_ROOT}/install.sh"

TEST_TMP="$(mktemp -d 2>/dev/null || mktemp -d -t 'mllm_test_install')"
cleanup() {
  rm -rf "$TEST_TMP"
}
trap cleanup EXIT
trap 'exit 1' INT TERM

echo "=== Running install.sh Test Suite ==="

# 1. POSIX Syntax Verification
echo "[Test 1/6] Verifying shell syntax..."
sh -n "$INSTALLER"
if command -v dash >/dev/null 2>&1; then
  dash -n "$INSTALLER"
fi
if command -v bash >/dev/null 2>&1; then
  bash -n "$INSTALLER"
fi
echo "  -> Shell syntax valid under sh, dash, bash."

# 2. Help and Usage Output
echo "[Test 2/6] Verifying --help and -h flags..."
HELP_OUT="$("$INSTALLER" --help)"
case "$HELP_OUT" in
  *"Usage: install.sh"*) ;;
  *)
    echo "Error: --help output missing expected usage." >&2
    exit 1
    ;;
esac
"$INSTALLER" -h >/dev/null
echo "  -> Help output verified."

# 3. Argument Validation & Error Handling
echo "[Test 3/6] Verifying error handling on invalid flags..."
if "$INSTALLER" --invalid-flag >/dev/null 2>&1; then
  echo "Error: Expected failure on --invalid-flag, but succeeded." >&2
  exit 1
fi
if "$INSTALLER" -b >/dev/null 2>&1; then
  echo "Error: Expected failure on missing argument for -b, but succeeded." >&2
  exit 1
fi
if "$INSTALLER" -v >/dev/null 2>&1; then
  echo "Error: Expected failure on missing argument for -v, but succeeded." >&2
  exit 1
fi
echo "  -> Invalid arguments correctly rejected."

# 4. Local Installation into Target Directory
echo "[Test 4/6] Verifying local installation mode (--local)..."
TMP_BIN="${TEST_TMP}/bin1"
"$INSTALLER" --local -b "$TMP_BIN" >/dev/null
if [ ! -x "${TMP_BIN}/mllm" ]; then
  echo "Error: Binary not installed at ${TMP_BIN}/mllm." >&2
  exit 1
fi
VER_OUT="$("${TMP_BIN}/mllm" --version)"
case "$VER_OUT" in
  *"mllm"*) ;;
  *)
    echo "Error: Installed binary --version returned unexpected output: $VER_OUT" >&2
    exit 1
    ;;
esac
echo "  -> Local installation succeeded: $VER_OUT."

# 5. Offline Fallback on Unreachable Remote Version
echo "[Test 5/6] Verifying offline fallback on missing remote release..."
TMP_BIN2="${TEST_TMP}/bin2"
FALLBACK_LOG="${TEST_TMP}/fallback.log"
"$INSTALLER" -b "$TMP_BIN2" -v non_existent_version_999 >"$FALLBACK_LOG" 2>&1
if [ ! -x "${TMP_BIN2}/mllm" ]; then
  echo "Error: Fallback binary not installed at ${TMP_BIN2}/mllm." >&2
  cat "$FALLBACK_LOG" >&2
  exit 1
fi
if ! grep -q "Pre-built release not found online" "$FALLBACK_LOG"; then
  echo "Error: Missing fallback notice in output:" >&2
  cat "$FALLBACK_LOG" >&2
  exit 1
fi
echo "  -> Graceful fallback to local binary verified."

# 6. Pipe Execution Simulation
echo "[Test 6/6] Verifying stdin pipe execution (curl ... | sh)..."
TMP_BIN3="${TEST_TMP}/bin3"
cat "$INSTALLER" | (cd "$REPO_ROOT" && BIN_DIR="$TMP_BIN3" LOCAL_BUILD=1 sh) >/dev/null
if [ ! -x "${TMP_BIN3}/mllm" ]; then
  echo "Error: Pipe install failed to produce ${TMP_BIN3}/mllm." >&2
  exit 1
fi
echo "  -> Piped shell execution succeeded."

# 7. Checksum Mock Test (if python3 is present)
if command -v python3 >/dev/null 2>&1; then
  echo "[Optional 7/7] Verifying SHA-256 download and checksum validation via mock server..."
  python3 - << 'PYEOF'
import tempfile, tarfile, hashlib, os, subprocess, http.server, socketserver, threading, shutil

with tempfile.TemporaryDirectory() as tmpdir:
    srv_dir = os.path.join(tmpdir, "srv")
    os.makedirs(os.path.join(srv_dir, "latest", "download"), exist_ok=True)

    mock_bin_path = os.path.join(tmpdir, "mock_mllm")
    with open(mock_bin_path, "w") as f:
        f.write("#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then echo 'mllm 9.9.9-mock'; fi\n")
    os.chmod(mock_bin_path, 0o755)

    archive_name = "mllm-x86_64-unknown-linux-gnu.tar.gz"
    archive_path = os.path.join(srv_dir, "latest", "download", archive_name)
    with tarfile.open(archive_path, "w:gz") as tar:
        tar.add(mock_bin_path, arcname="mllm")

    with open(archive_path, "rb") as f:
        valid_sha = hashlib.sha256(f.read()).hexdigest()

    sha_path = archive_path + ".sha256"
    with open(sha_path, "w") as f:
        f.write(f"{valid_sha}  {archive_name}\n")

    class Handler(http.server.SimpleHTTPRequestHandler):
        def __init__(self, *args, **kwargs):
            super().__init__(*args, directory=srv_dir, **kwargs)
        def log_message(self, format, *args):
            pass

    httpd = socketserver.TCPServer(("127.0.0.1", 0), Handler)
    port = httpd.server_address[1]
    t = threading.Thread(target=httpd.serve_forever)
    t.daemon = True
    t.start()

    installer = os.environ.get("INSTALLER", "/home/ykim/Projects/mllm/install.sh")
    base_url = f"http://127.0.0.1:{port}"
    target_bin = os.path.join(tmpdir, "bin_mock")

    # Match check
    env = os.environ.copy()
    env["BASE_URL"] = base_url
    env["BIN_DIR"] = target_bin
    res = subprocess.run(["sh", installer], env=env, capture_output=True, text=True)
    assert res.returncode == 0, f"Checksum match failed: {res.stderr}\n{res.stdout}"
    assert "Checksum verified" in res.stdout

    # Mismatch check
    with open(sha_path, "w") as f:
        f.write(f"ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff  {archive_name}\n")
    shutil.rmtree(target_bin)
    res_mismatch = subprocess.run(["sh", installer], env=env, capture_output=True, text=True)
    assert res_mismatch.returncode != 0, "Checksum mismatch did not abort installer"
    assert "SHA-256 checksum verification failed" in res_mismatch.stderr

    httpd.shutdown()
PYEOF
  echo "  -> Checksum verification and mismatch rejection confirmed."
fi

echo "=== All install.sh tests passed successfully! ==="
