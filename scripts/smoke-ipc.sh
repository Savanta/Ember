#!/usr/bin/env bash
set -euo pipefail

# End-to-end smoke test for Ember IPC socket.
# Verifies command/response shape for get_state, history, and search.

SOCKET_PATH="${XDG_RUNTIME_DIR:-/run/user/$(id -u)}/ember.sock"

if [[ ! -S "$SOCKET_PATH" ]]; then
  echo "[FAIL] IPC socket not found: $SOCKET_PATH"
  echo "       Is ember.service running?"
  exit 1
fi

python3 - <<'PY'
import json
import os
import socket
import sys

sock_path = os.path.join(os.environ.get("XDG_RUNTIME_DIR", f"/run/user/{os.getuid()}"), "ember.sock")


def ipc(payload: dict) -> dict:
    s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    s.settimeout(2.0)
    s.connect(sock_path)
    s.sendall((json.dumps(payload) + "\n").encode("utf-8"))
    buf = b""
    while b"\n" not in buf:
        chunk = s.recv(65536)
        if not chunk:
            break
        buf += chunk
    s.close()
    line = buf.split(b"\n", 1)[0]
    return json.loads(line.decode("utf-8"))


def assert_status(rsp: dict, expected: str, cmd: str):
    got = rsp.get("status")
    if got != expected:
        print(f"[FAIL] {cmd}: expected status={expected!r}, got {got!r}")
        print(json.dumps(rsp, indent=2, ensure_ascii=False))
        sys.exit(1)
    print(f"[OK] {cmd}: status={got}")


state = ipc({"cmd": "get_state"})
assert_status(state, "state", "get_state")
for key in ("unread", "dnd", "notifications"):
    if key not in state:
        print(f"[FAIL] get_state: missing key {key!r}")
        sys.exit(1)
print("[OK] get_state payload shape")

history = ipc({"cmd": "history", "limit": 3, "offset": 0})
assert_status(history, "history", "history")
if "items" not in history or not isinstance(history["items"], list):
    print("[FAIL] history: missing/invalid 'items'")
    sys.exit(1)
print(f"[OK] history items count={len(history['items'])}")

search = ipc({"cmd": "search", "query": "evolution", "limit": 2})
assert_status(search, "search", "search")
if "items" not in search or not isinstance(search["items"], list):
    print("[FAIL] search: missing/invalid 'items'")
    sys.exit(1)
print(f"[OK] search items count={len(search['items'])}")

print("[PASS] Ember IPC smoke test complete")
PY
