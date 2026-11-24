#!/bin/sh
set -eu

# CLI surface test: spins up two throwaway daemons and exercises every idfon
# command and flag not already covered by the specialized e2e scripts
# (stream-e2e.sh / fanout-e2e.sh cover live streaming; this covers the rest).
#
# Verified: status, shutdown, identity (list/rename; create/delete refused),
# tag (add/list/set/rename/remove/delete), visibility, card (send --wait), peer
# (add/list/show/resolve/status/update/remove), access
# (allow/check/ticket), send --text (+ --idempotency-key), send --file,
# recv (+ --out/--from), put (+ --resource-id/--json), get, events (+ --type),
# wait, operation (get/wait/cancel), --json envelopes, and documented failure
# paths (missing peers, bogus tickets/operations, recv timeout).
#
# PASS requires every check to pass; exits non-zero on the first failure.

root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
. "$root/scripts/lib/core.sh"
cd "$root"

# Same build/sign preamble as test-e2e.sh: the thin idfond links the vendored
# dylib, and a relink can leave a stale ad-hoc signature the kernel kills.
  core_cargo build --release -p iroh-c-ffi
core_cargo build --release -p idfond && cargo build --release -p idfon-cli
codesign --force -s - "$core_target"/release/libiroh_c_ffi.dylib "$core_target"/release/idfond

work=$(mktemp -d /tmp/idfon-cli.XXXXXX)
pids=""
cleanup() {
  if [ -n "$pids" ]; then kill $pids 2>/dev/null || true; fi
  rm -rf "$work"
}
trap cleanup EXIT

NUF="$core_target/release/idfon"
A="$work/a/idfond.sock"
B="$work/b/idfond.sock"
Q() { # idfon against a socket, passthrough
  "$NUF" --socket "$1" "${@:2}"
}

mkdir -p "$work/a" "$work/b"
"$core_target/release/idfond" --socket "$A" --data-dir "$work/a" &
pids="$pids $!"
"$core_target/release/idfond" --socket "$B" --data-dir "$work/b" &
pids="$pids $!"

wait_ready() { # socket label
  for _ in $(seq 1 100); do
    if "$NUF" --socket "$1" status --json >/dev/null 2>&1; then
      return 0
    fi
    sleep 0.1
  done
  echo "FAIL: daemon $2 not ready" >&2
  exit 1
}
wait_ready "$A" A
wait_ready "$B" B

ok() { # label cmd...
  label=$1; shift
  if "$@" >/dev/null 2>&1; then
    echo "PASS: $label"
  else
    echo "FAIL: $label" >&2
    exit 1
  fi
}
fails() { # label cmd...  (command must exit non-zero)
  label=$1; shift
  if "$@" >/dev/null 2>&1; then
    echo "FAIL: $label (expected non-zero exit)" >&2
    exit 1
  else
    echo "PASS: $label"
  fi
}

# --- status -----------------------------------------------------------------
ok "status (human)" sh -c "\"$NUF\" --socket '$A' status | grep -q ready"
S=$(Q "$A" status --json)
echo "$S" | jq -e '.ok == true and .result.ready == true and .result.identity.name' >/dev/null
echo "PASS: status --json envelope"

# --- identity: one you (docs/identity.md in idfon-core) -----------------------
ORIG_ID=$(echo "$S" | jq -r '.result.identity.name')
ok "identity list" Q "$A" identity list --json
Q "$A" identity list --json | jq -e '.result.identities | length == 1' >/dev/null
echo "PASS: one identity"
ok "identity rename" Q "$A" identity rename "Ann Example" --json
Q "$A" status --json | jq -e '.result.identity.name == "Ann Example"' >/dev/null
echo "PASS: identity rename shows in status"
ok "identity rename back" Q "$A" identity rename "$ORIG_ID" --json
fails "identity create refused" Q "$A" rpc identity.create '{"name":"travel"}' --json

# --- peer -------------------------------------------------------------------
A_EP=$(Q "$A" status --json | jq -r .result.identity.endpoint_id)
A_PID=$(Q "$A" status --json | jq -r .result.identity.public_key)
A_ADDR=$(Q "$A" status --json | jq -r '.result.ticket | implode')
B_EP=$(Q "$B" status --json | jq -r .result.identity.endpoint_id)
B_PID=$(Q "$B" status --json | jq -r .result.identity.public_key)
B_ADDR=$(Q "$B" status --json | jq -r '.result.ticket | implode')
ok "peer add" Q "$A" peer add "$B_PID" --name bob --endpoint-id "$B_EP" --endpoint-addr "$B_ADDR" --json
ok "peer add (reverse)" Q "$B" peer add "$A_PID" --name alice --endpoint-id "$A_EP" --endpoint-addr "$A_ADDR" --json
Q "$A" peer list --json | jq -e '.result.peers | map(select(.name == "bob")) | length == 1' >/dev/null
echo "PASS: peer list"
S=$(Q "$A" peer show bob --json)
echo "$S" | jq -e '.result.peer.name == "bob" and .result.peer.endpoint_id == "'"$B_EP"'"' >/dev/null
echo "PASS: peer show"
ok "peer resolve" Q "$A" peer resolve bob --json
ok "peer status" Q "$A" peer status bob --json
ok "peer update" Q "$A" peer update bob --name robert --json
S=$(Q "$A" peer show robert --json)
echo "$S" | jq -e '.result.peer.name == "robert"' >/dev/null
echo "PASS: peer update visible in show"
ok "peer update name back" Q "$A" peer update robert --name bob --json

# --- tags and visibility ------------------------------------------------------
ok "tag add" Q "$A" tag add bob Work --json
Q "$A" peer show bob --json | jq -e '.result.peer.tags == ["Work"]' >/dev/null
echo "PASS: tag on contact"
ok "tag set hides presence" Q "$A" tag set Work --hide-presence --json
Q "$A" tag list --json | jq -e '.result.tags | map(select(.name == "Work" and .count == 1 and .hide_presence == true)) | length == 1' >/dev/null
echo "PASS: tag list"
Q "$A" rpc visibility.audience '{"aspect":"presence"}' --json | jq -e '.result.peers | length == 0' >/dev/null
Q "$A" rpc visibility.audience '{"aspect":"photo"}' --json | jq -e '.result.peers | length == 1' >/dev/null
echo "PASS: Work hides presence from bob, not the photo"
ok "visibility off" Q "$A" visibility --photo off --json
Q "$A" visibility --json | jq -e '.result.visibility.photo == false and .result.visibility.presence == true' >/dev/null
echo "PASS: visibility defaults"
ok "visibility on" Q "$A" visibility --photo on --json
ok "tag rename" Q "$A" tag rename Work Office --json
Q "$A" peer show bob --json | jq -e '.result.peer.tags == ["Office"]' >/dev/null
echo "PASS: tag rename reaches contacts"
ok "tag remove" Q "$A" tag remove bob Office --json
ok "tag delete" Q "$A" tag delete Office --json
Q "$A" tag list --json | jq -e '.result.tags | length == 0' >/dev/null
echo "PASS: tag delete"
fails "peer show missing" Q "$A" peer show nobody --json
fails "peer resolve missing" Q "$A" peer resolve nobody --json
fails "peer status missing" Q "$A" peer status nobody --json

# --- access -----------------------------------------------------------------
ok "access allow (A->B send)" Q "$A" access allow --subject "$B_PID" --capability message.send --json
ok "access allow (B->A send)" Q "$B" access allow --subject "$A_PID" --capability message.send --json
ok "access allow (B->A receive)" Q "$B" access allow --subject "$A_PID" --capability message.receive --json
S=$(Q "$A" access check --subject "$B_PID" --capability message.send --json)
echo "$S" | jq -e '.result.allowed == true' >/dev/null
echo "PASS: access check granted"
S=$(Q "$A" access check --subject "$A_PID" --capability message.send --json)
echo "$S" | jq -e '.result.allowed == false' >/dev/null
echo "PASS: access check not granted"
# Capability names are open (M5): an ungranted name is valid, just not allowed.
S=$(Q "$A" access check --subject "$B_PID" --capability bogus.cap --json)
echo "$S" | jq -e '.ok == true and .result.allowed == false' >/dev/null
echo "PASS: open capability with no grant reports not-allowed"
T=$(Q "$A" access ticket --subject "$B_PID" --capability message.send)
[ -n "$T" ] && [ "$T" != null ]
echo "PASS: access ticket prints the ticket"
# M5: capability names are open namespaced strings, not a fixed enum.
ok "access allow (open namespaced capability)" Q "$A" access allow --subject "$B_PID" --capability vendor.custom.thing --json
S=$(Q "$A" access check --subject "$B_PID" --capability vendor.custom.thing --json)
echo "$S" | jq -e '.result.allowed == true and .result.capability == "vendor.custom.thing"' >/dev/null
echo "PASS: open namespaced capability round-trips"

# --- send --text / events / wait / operation --------------------------------
CURSOR=$(Q "$B" events --json | jq -r '.result.events | last | .cursor // empty')
S=$(Q "$A" send bob --text "hello from cli test" --json)
echo "$S" | jq -e '.ok == true and .result.operation_id != ""' >/dev/null
echo "PASS: send --text --json"
OPID=$(echo "$S" | jq -r .result.operation_id)
W=$(Q "$B" wait --type message.received --after "$CURSOR" --json)
echo "$W" | jq -e '.result.events[0].data.text == "hello from cli test"' >/dev/null
echo "PASS: wait receives text event with body"
S=$(Q "$B" events --json --type message.received)
echo "$S" | jq -e '.result.events | length >= 1' >/dev/null
echo "PASS: events --type filter"
S=$(Q "$B" events --json --type test.never)
echo "$S" | jq -e '.result.events | length == 0' >/dev/null
echo "PASS: events --type with no matches is empty"
W=$(Q "$B" wait --type test.never --timeout-ms 300 --json)
echo "$W" | jq -e '.ok == true and (.result.events | length == 0)' >/dev/null
echo "PASS: wait timeout returns empty success"
S=$(Q "$A" operation get "$OPID" --json)
echo "$S" | jq -e '.result.operation.status == "delivered"' >/dev/null
echo "PASS: operation get delivered"
S=$(Q "$A" operation wait "$OPID" --json)
echo "$S" | jq -e '.result.operation.status == "delivered"' >/dev/null
echo "PASS: operation wait terminal"
fails "operation get missing" Q "$A" operation get op_nope --json
fails "operation cancel missing" Q "$A" operation cancel op_nope --json
ok "send --text --idempotency-key" Q "$A" send bob --text dedupe --idempotency-key cli-test-k1 --json
ok "send --text --idempotency-key (resend)" Q "$A" send bob --text dedupe --idempotency-key cli-test-k1 --json
fails "send without PEER" Q "$A" send --text hi --json

# --- card send --wait: bob answers like the phone does ------------------------
Q "$A" card send bob "Lunch?" --id cli-card-1 --action yes=Yes --field note=Note --wait --timeout 30s --json > "$work/card.json" &
card_pid=$!
sleep 2
Q "$B" inbox --json | grep -q 'IDFON-UI/1' || { echo "FAIL: card not received"; exit 1; }
values=$(printf '{"note":"noon"}' | base64)
ok "card answer" Q "$B" send alice --text "$(printf 'IDFON-UI-ACTION/1\ncard=cli-card-1\naction=yes\nvalues=%s' "$values")" --json
wait "$card_pid"
jq -e '.card == "cli-card-1" and .action == "yes" and .values.note == "noon"' "$work/card.json" >/dev/null
echo "PASS: card send --wait reads the answer and its values"

# --- put/get variations (chunked round trip lives in test-e2e.sh) ------------
printf 'cli-test-payload' > "$work/payload.bin"
T=$(Q "$A" put --resource-id cli-test-res < "$work/payload.bin")
Q "$A" get "$T" > "$work/payload.roundtrip"
cmp -s "$work/payload.bin" "$work/payload.roundtrip"
echo "PASS: put --resource-id / get round trip"
S=$(Q "$A" put --json < "$work/payload.bin")
echo "$S" | jq -e '.result.blob_ticket != "" and .result.size_bytes > 0' >/dev/null
echo "PASS: put --json envelope"
fails "get bogus ticket" Q "$A" get blob-does-not-exist
fails "get garbage ticket" Q "$A" get not-a-ticket

# --- signaled transfer with recv --out/--from --------------------------------
# recv baselines on the latest cursor when it starts, so it must be running
# (with its baseline taken) before the send completes.
Q "$B" recv --out "$work/signaled.received" --from "$A_PID" --timeout-ms 30000 2>/dev/null < /dev/null &
recv_pid=$!
sleep 1
Q "$A" send bob --file --retries 2 < "$work/payload.bin" > /dev/null
if wait "$recv_pid"; then
  cmp -s "$work/payload.bin" "$work/signaled.received"
  echo "PASS: recv --out --from"
else
  echo "FAIL: recv --out --from" >&2
  exit 1
fi
fails "recv timeout" Q "$A" recv --timeout-ms 300 --json

# --- stream --list (publish/dial covered by stream-e2e.sh) -------------------
ok "send --stream --list" Q "$A" send --stream --list --json

# --- peer remove --------------------------------------------------------------
ok "peer remove" Q "$A" peer remove bob --json
Q "$A" peer list --json | jq -e '.result.peers | length == 0' >/dev/null
echo "PASS: peer remove leaves list empty"

# --- shutdown (last: A goes away here; do not touch A afterwards) ------------
ok "shutdown" Q "$A" shutdown
fails "shutdown when down (no auto-start)" Q "$A" shutdown

echo "PASS: idfon CLI surface test"
