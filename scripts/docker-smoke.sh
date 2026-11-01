#!/usr/bin/env bash
# Smoke-test the container image over stdio: initialize, initialized, tools/list.
# Read-only must list the 4 read tools; FOLDER_WRITABLE=true must list 7.
# stdout must carry nothing but JSON-RPC messages.
#
#   scripts/docker-smoke.sh [image]     (default image: emperor-folder:smoke)
set -euo pipefail

image="${1:-emperor-folder:smoke}"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
echo "hello" > "$tmp/hello.txt"
chmod -R a+rwX "$tmp"

requests() {
  printf '%s\n' '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"smoke","version":"0"}}}'
  sleep 1
  printf '%s\n' '{"jsonrpc":"2.0","method":"notifications/initialized"}'
  printf '%s\n' '{"jsonrpc":"2.0","id":2,"method":"tools/list"}'
  sleep 3
}

check() {
  local expected="$1" out="$2"
  python3 - "$expected" "$out" <<'PY'
import json, sys
expected = sys.argv[1].split(",")
lines = [l for l in open(sys.argv[2]).read().splitlines() if l.strip()]
msgs = []
for l in lines:
    try:
        m = json.loads(l)
    except ValueError:
        sys.exit(f"stdout line is not JSON: {l!r}")
    if m.get("jsonrpc") != "2.0":
        sys.exit(f"stdout line is not JSON-RPC: {l!r}")
    msgs.append(m)
by_id = {m.get("id"): m for m in msgs}
init = by_id.get(1, {}).get("result")
if not init or init.get("protocolVersion") != "2025-06-18":
    sys.exit(f"bad initialize response: {by_id.get(1)}")
tools = sorted(t["name"] for t in by_id.get(2, {}).get("result", {}).get("tools", []))
if tools != sorted(expected):
    sys.exit(f"tools {tools}, expected {sorted(expected)}")
print(f"ok: {len(tools)} tools: {', '.join(tools)}")
PY
}

echo "read-only:"
requests | docker run -i --rm -v "$tmp:/data" "$image" > "$tmp/ro.out"
check "list,read,search,stat" "$tmp/ro.out"

echo "FOLDER_WRITABLE=true:"
requests | docker run -i --rm -v "$tmp:/data" -e FOLDER_WRITABLE=true "$image" > "$tmp/rw.out"
check "edit,list,move,read,search,stat,write" "$tmp/rw.out"
