#!/usr/bin/env bash
# The build gate, from WSL: clippy (deny warnings) → tests → release exe.
#
# Cross-compiled with cargo-zigbuild; the test binary runs on the Windows side
# through interop because it links Win32. Set HEADROOM_TEMP to override the
# Windows temp directory used for the test exe.
set -euo pipefail
cd "$(dirname "$0")/.."
export PATH="$HOME/.local/cc-monitor-rc-shims:$HOME/.cargo/bin:$HOME/.local/bin:$PATH"
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$HOME/.cache/ccum-upstream-target}"
TARGET=x86_64-pc-windows-gnu
TEMP_DIR="${HEADROOM_TEMP:-/mnt/c/Users/$USER/AppData/Local/Temp}"

echo "== clippy"
cargo clippy --target "$TARGET" --locked -- -D warnings 2>&1 | grep -vE "font subset|^\s*Compiling|^\s*Checking|^\s*Finished" || true
cargo clippy --target "$TARGET" --locked -- -D warnings > /dev/null 2>&1

echo "== tests (build)"
cargo zigbuild --tests --target "$TARGET" --release --locked --message-format=json > "$CARGO_TARGET_DIR/tests.json" 2> "$CARGO_TARGET_DIR/tests.err" || {
  python3 - "$CARGO_TARGET_DIR/tests.json" <<'PY'
import json, sys
for line in open(sys.argv[1]):
    try: m = json.loads(line)
    except ValueError: continue
    if m.get("reason") == "compiler-message" and m["message"]["level"] == "error":
        print(m["message"]["rendered"][:800])
PY
  exit 1
}
exe=$(python3 - "$CARGO_TARGET_DIR/tests.json" <<'PY'
import json, sys
for line in open(sys.argv[1]):
    try: m = json.loads(line)
    except ValueError: continue
    if m.get("reason") == "compiler-artifact" and m.get("executable") and m["target"]["name"] == "headroom" and m["profile"]["test"]:
        print(m["executable"])
PY
)
[ -n "$exe" ] || { echo "no test executable produced"; exit 1; }
cp "$exe" "$TEMP_DIR/headroom-tests.exe"

echo "== tests (run)"
"$TEMP_DIR/headroom-tests.exe" --test-threads=4 2>&1 | tr -d '\r' | tee "$CARGO_TARGET_DIR/tests.out" | grep -E "^test result|FAILED|panicked at|^    [a-z_:]+$" || true
grep -q "^test result: ok" "$CARGO_TARGET_DIR/tests.out"

echo "== release"
cargo zigbuild --target "$TARGET" --release --locked > "$CARGO_TARGET_DIR/release.log" 2>&1 || { grep -E "^error" -A8 "$CARGO_TARGET_DIR/release.log" | head -40; exit 1; }
# Staging only: a running dist/headroom.exe is locked by Windows, and the
# gate is not the deployer. The fresh build always lands in dist/staged/.
mkdir -p dist/staged && cp "$CARGO_TARGET_DIR/$TARGET/release/headroom.exe" dist/staged/headroom.exe
cp "$CARGO_TARGET_DIR/$TARGET/release/headroom.exe" dist/headroom.exe 2>/dev/null \
  && echo "== gate green: $(stat -c %s dist/headroom.exe) bytes → dist/headroom.exe" \
  || echo "== gate green: dist/headroom.exe is in use (app running); fresh build at dist/staged/headroom.exe"
