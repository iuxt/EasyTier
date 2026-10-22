#!/usr/bin/env bash

set -euo pipefail

SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
REPO_ROOT=$(CDPATH= cd -- "$SCRIPT_DIR/.." && pwd)

CORE_BIN=${CORE_BIN:-"$REPO_ROOT/target/debug/easytier-core"}
TMPDIR_PATH=""
CORE_PID=""
PYTHON_BIN=${PYTHON_BIN:-python3}

print_section() {
  printf '\n==> %s\n' "$1"
}

print_output() {
  local title="$1"
  local content="$2"
  printf -- '---- %s ----\n' "$title"
  printf '%s\n' "$content"
  printf -- '---- end %s ----\n' "$title"
}

build_binaries() {
  print_section "Building easytier-core with integrated management commands"
  cargo build -p easytier --bin easytier-core
}

ensure_binaries() {
  if [[ "${SKIP_BUILD:-0}" != "1" ]] || [[ ! -x "$CORE_BIN" ]]; then
    build_binaries
  fi
}

make_tmpdir() {
  "$PYTHON_BIN" - <<'PY'
import tempfile
print(tempfile.mkdtemp(prefix="easytier-core-management-e2e-"))
PY
}

cleanup_tmpdir() {
  TMPDIR_TO_DELETE="$1" "$PYTHON_BIN" - <<'PY'
import os
import shutil

shutil.rmtree(os.environ["TMPDIR_TO_DELETE"], ignore_errors=True)
PY
}

check_management_port() {
  "$PYTHON_BIN" - <<'PY'
import socket

sock = socket.socket()
try:
    sock.bind(("127.0.0.1", 15888))
except OSError as error:
    raise SystemExit(f"Local management port 15888 is in use; stop the existing core before testing: {error}")
print(15888)
sock.close()
PY
}

wait_for_management() {
  local attempts=0
  while (( attempts < 50 )); do
    if "$CORE_BIN" -o json node >/dev/null 2>&1; then
      return 0
    fi
    attempts=$((attempts + 1))
    sleep 0.2
  done
  return 1
}

run_cmd() {
  local __var_name="$1"
  local title="$2"
  shift 2

  print_section "$title"
  printf '+'
  for arg in "$@"; do
    printf ' %q' "$arg"
  done
  printf '\n'

  local output
  if ! output=$("$@" 2>&1); then
    print_output "$title output" "$output"
    return 1
  fi

  print_output "$title output" "$output"
  printf -v "$__var_name" '%s' "$output"
}

assert_text_output() {
  local text_output="$1"
  grep -F '== 018f4fb1-7a2c-7d1f-9d89-935b0ad7e135 ==' <<<"$text_output" >/dev/null
  grep -F '== 018f4fb1-7a2c-7d1f-9d89-935b0ad7e136 ==' <<<"$text_output" >/dev/null
}

assert_multi_instance_json() {
  local json_payload="$1"
  JSON_PAYLOAD="$json_payload" JSON_RESULT_TYPE="${2:-dict}" "$PYTHON_BIN" - <<'PY'
import json
import os

data = json.loads(os.environ["JSON_PAYLOAD"])
result_type = {"dict": dict, "list": list}[os.environ["JSON_RESULT_TYPE"]]
assert isinstance(data, list), data
assert len(data) == 2, data

ids = {item["instance_id"] for item in data}
assert ids == {"018f4fb1-7a2c-7d1f-9d89-935b0ad7e135", "018f4fb1-7a2c-7d1f-9d89-935b0ad7e136"}, ids

for item in data:
    assert set(item) == {"instance_id", "result"}, item
    assert item["instance_id"], item
    assert isinstance(item["result"], result_type), item
PY
}

assert_single_instance_json() {
  local json_payload="$1"
  JSON_PAYLOAD="$json_payload" "$PYTHON_BIN" - <<'PY'
import json
import os

data = json.loads(os.environ["JSON_PAYLOAD"])
assert isinstance(data, dict), data
assert 'instance_id = "018f4fb1-7a2c-7d1f-9d89-935b0ad7e135"' in data["config"], data["config"]
PY
}

assert_whitelist_fanout() {
  local json_payload="$1"
  JSON_PAYLOAD="$json_payload" "$PYTHON_BIN" - <<'PY'
import json
import os

data = json.loads(os.environ["JSON_PAYLOAD"])
assert len(data) == 2, data
for item in data:
    assert item["result"]["tcp_ports"] == ["80", "443"], item
    assert item["result"].get("udp_ports", []) == [], item
PY
}

assert_single_instance_write() {
  local json_payload="$1"
  JSON_PAYLOAD="$json_payload" "$PYTHON_BIN" - <<'PY'
import json
import os

data = {item["instance_id"]: item["result"] for item in json.loads(os.environ["JSON_PAYLOAD"])}
assert data["018f4fb1-7a2c-7d1f-9d89-935b0ad7e135"]["tcp_ports"] == ["80", "443"], data
assert data["018f4fb1-7a2c-7d1f-9d89-935b0ad7e136"].get("tcp_ports", []) == [], data
PY
}

main() {
  command -v "$PYTHON_BIN" >/dev/null 2>&1 || {
    echo "python interpreter not found: $PYTHON_BIN" >&2
    exit 1
  }

  ensure_binaries

  local rpc_port
  rpc_port=$(check_management_port)
  print_section "Checked local management port"
  printf '%s\n' "$rpc_port"

  TMPDIR_PATH=$(make_tmpdir)
  print_section "Created temporary test directory"
  printf '%s\n' "$TMPDIR_PATH"

  cleanup() {
    if [[ -n "$CORE_PID" ]] && kill -0 "$CORE_PID" >/dev/null 2>&1; then
      kill "$CORE_PID" >/dev/null 2>&1 || true
      wait "$CORE_PID" >/dev/null 2>&1 || true
    fi
    if [[ -n "$TMPDIR_PATH" ]]; then
      cleanup_tmpdir "$TMPDIR_PATH"
    fi
  }
  trap cleanup EXIT

  cat >"$TMPDIR_PATH/inst-a.toml" <<'EOF'
instance_id = "018f4fb1-7a2c-7d1f-9d89-935b0ad7e135"
hostname = "e2e-inst-a"
listeners = []

[network_identity]
network_name = "e2e-net-a"
network_secret = ""

[flags]
no_tun = true
enable_ipv6 = false
EOF

  cat >"$TMPDIR_PATH/inst-b.toml" <<'EOF'
instance_id = "018f4fb1-7a2c-7d1f-9d89-935b0ad7e136"
hostname = "e2e-inst-b"
listeners = []

[network_identity]
network_name = "e2e-net-b"
network_secret = ""

[flags]
no_tun = true
enable_ipv6 = false
EOF

  mkdir -p "$TMPDIR_PATH/networks"
  mv "$TMPDIR_PATH/inst-a.toml" "$TMPDIR_PATH/networks/"
  mv "$TMPDIR_PATH/inst-b.toml" "$TMPDIR_PATH/networks/"
  cat >"$TMPDIR_PATH/config.toml" <<EOF
[core]
config_dir = "networks"
rpc_portal = "127.0.0.1:${rpc_port}"
EOF
  "$CORE_BIN" -c "$TMPDIR_PATH/config.toml" >"$TMPDIR_PATH/core.log" 2>&1 &
  CORE_PID=$!
  print_section "Started easytier-core"
  printf 'pid=%s\n' "$CORE_PID"

  wait_for_management
  print_output "easytier-core startup log" "$(cat "$TMPDIR_PATH/core.log")"

  local text_output
  run_cmd text_output \
    "Case 1: node fanout in table mode" \
    "$CORE_BIN" node
  assert_text_output "$text_output"

  local json_output
  run_cmd json_output \
    "Case 2: node fanout in JSON mode" \
    "$CORE_BIN" -o json node
  assert_multi_instance_json "$json_output"

  local single_output
  run_cmd single_output \
    "Case 3: explicit instance selector stays single-instance" \
    "$CORE_BIN" --instance-id 018f4fb1-7a2c-7d1f-9d89-935b0ad7e135 -o json node
  assert_single_instance_json "$single_output"

  local set_whitelist_output
  run_cmd set_whitelist_output \
    "Case 4: whitelist set-tcp fans out to all instances" \
    "$CORE_BIN" whitelist set-tcp 80,443

  local whitelist_output
  run_cmd whitelist_output \
    "Case 5: whitelist show confirms fanout write" \
    "$CORE_BIN" -o json whitelist show
  assert_whitelist_fanout "$whitelist_output"

  local clear_whitelist_output
  run_cmd clear_whitelist_output \
    "Case 6: explicit selector write only touches one instance" \
    "$CORE_BIN" --instance-id 018f4fb1-7a2c-7d1f-9d89-935b0ad7e136 whitelist clear-tcp

  local cleared_output
  run_cmd cleared_output \
    "Case 7: whitelist show confirms single-instance write isolation" \
    "$CORE_BIN" -o json whitelist show
  assert_single_instance_write "$cleared_output"

  local credential_table
  run_cmd credential_table \
    "Case 8: credential peers table fans out to all instances" \
    "$CORE_BIN" credential peers
  assert_text_output "$credential_table"

  local credential_json
  run_cmd credential_json \
    "Case 9: credential peers JSON preserves instance wrappers" \
    "$CORE_BIN" -o json credential peers
  assert_multi_instance_json "$credential_json"
  JSON_PAYLOAD="$credential_json" "$PYTHON_BIN" - <<'PY'
import json
import os

data = json.loads(os.environ["JSON_PAYLOAD"])
assert all(item["result"] == {"credentials": []} for item in data), data
PY

  local credential_single
  run_cmd credential_single \
    "Case 10: credential peers explicit selector returns one instance" \
    "$CORE_BIN" --instance-id 018f4fb1-7a2c-7d1f-9d89-935b0ad7e135 -o json credential peers
  JSON_PAYLOAD="$credential_single" "$PYTHON_BIN" - <<'PY'
import json
import os

assert json.loads(os.environ["JSON_PAYLOAD"]) == {"credentials": []}
PY

  local peer_output
  run_cmd peer_output \
    "Case 11: peer query works while the core is running" \
    "$CORE_BIN" -o json peer
  assert_multi_instance_json "$peer_output" list

  local route_output
  run_cmd route_output \
    "Case 12: route query works while the core is running" \
    "$CORE_BIN" -o json route
  assert_multi_instance_json "$route_output" list
  kill -0 "$CORE_PID"

  print_section "Result"
  echo "Core management multi-instance E2E passed"
}

main "$@"
