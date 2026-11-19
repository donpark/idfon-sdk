#!/bin/sh
# Group-call signaling acceptance (idfon-home docs/group-live-chat.md, G1).
#
# Three real daemons share a room. Each joins the room's call in turn; every
# one must end up in one session with the other two on its roster. Then one
# leaves (the others drop it), and the last two leave (auto-close ends the
# session everywhere). Signaling only: the tickets are placeholders, no media.
#
#   scripts/room-call-e2e.sh
set -eu

root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$root"

pnpm cli build

work=$(mktemp -d /tmp/idfon-room-call.XXXXXX)
pids=""
cleanup() {
  for pid in $pids; do kill "$pid" 2>/dev/null || true; done
  [ -n "$pids" ] && wait $pids 2>/dev/null || true
  rm -rf "$work"
}
trap cleanup EXIT

NUF="$root/target/release/idfon"
ROOM="r_$(printf '%032d' 7)"

for who in a b c; do
  mkdir -p "$work/$who"
  target/release/idfond --socket "$work/$who/idfond.sock" --data-dir "$work/$who" \
    >"$work/$who.log" 2>&1 &
  pids="$pids $!"
done

sock() { printf '%s' "$work/$1/idfond.sock"; }

for who in a b c; do
  for _ in $(seq 1 150); do
    if "$NUF" --socket "$(sock "$who")" status --json >/dev/null 2>&1; then break; fi
    sleep 0.1
  done
done

identity_field() {
  "$NUF" --socket "$(sock "$1")" status --json | jq -r ".result.identity.$2"
}

A_PID=$(identity_field a public_key); A_EID=$(identity_field a endpoint_id); A_ADDR=$($NUF --socket "$(sock a)" status --json | jq -r '.result.ticket | implode')
B_PID=$(identity_field b public_key); B_EID=$(identity_field b endpoint_id); B_ADDR=$($NUF --socket "$(sock b)" status --json | jq -r '.result.ticket | implode')
C_PID=$(identity_field c public_key); C_EID=$(identity_field c endpoint_id); C_ADDR=$($NUF --socket "$(sock c)" status --json | jq -r '.result.ticket | implode')

# Add the other two peers on every daemon and grant both directions. This is the
# ordinary peer path — nothing here is room-specific.
for entry in "a $B_PID $B_EID $B_ADDR" "a $C_PID $C_EID $C_ADDR" \
             "b $A_PID $A_EID $A_ADDR" "b $C_PID $C_EID $C_ADDR" \
             "c $A_PID $A_EID $A_ADDR" "c $B_PID $B_EID $B_ADDR"; do
  # shellcheck disable=SC2086
  set -- $entry
  self=$1; peer=$2; endpoint=$3; address=$4
  "$NUF" --socket "$(sock "$self")" peer add "$peer" --name "$peer" \
    --endpoint-id "$endpoint" --endpoint-addr "$address" --json >/dev/null
  for capability in message.send message.receive; do
    "$NUF" --socket "$(sock "$self")" access allow --subject "$peer" \
      --capability "$capability" --json >/dev/null
  done
done


for entry in "a $B_PID $C_PID" "b $A_PID $C_PID" "c $A_PID $B_PID"; do
  # shellcheck disable=SC2086
  set -- $entry
  "$NUF" --socket "$(sock "$1")" room create --id "$ROOM" --name call \
    --member "$2" --member "$3" --json >/dev/null
done

call() { "$NUF" --socket "$(sock "$1")" room call status "$ROOM" --json | jq -c "$2"; }

# wait_for WHO JQ EXPECTED: poll WHO's call status until JQ yields EXPECTED.
wait_for() {
  for _ in $(seq 1 150); do
    [ "$(call "$1" "$2")" = "$3" ] && return 0
    sleep 0.1
  done
  echo "FAIL: $1: $2 is $(call "$1" "$2"), expected $3" >&2
  exit 1
}

for who in a b c; do
  "$NUF" --socket "$(sock "$who")" room call join "$ROOM" --ticket "ilv1:$who" \
    --audio-codec liquid --json >/dev/null
  sleep 0.3
done

for who in a b c; do wait_for "$who" '.result.call.people' 3; done
session=$(call a '.result.call.call_id')
for who in b c; do wait_for "$who" '.result.call.call_id' "$session"; done
echo "ok: a, b and c are in one session ($session), three people each"

"$NUF" --socket "$(sock b)" room call leave "$ROOM" --json >/dev/null
for who in a c; do wait_for "$who" '.result.call.people' 2; done
wait_for a "[.result.call.members[].peer]" "[\"$C_PID\"]"
echo "ok: b left; a and c dropped it"

"$NUF" --socket "$(sock a)" room call leave "$ROOM" --json >/dev/null
"$NUF" --socket "$(sock c)" room call leave "$ROOM" --json >/dev/null
for who in a b c; do wait_for "$who" '.result.call.call_id' null; done
echo "ok: the last two left; auto-close ended the session everywhere"

echo "PASS: room call signaling across three daemons"
