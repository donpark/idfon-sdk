#!/bin/sh
# Group voice call, end to end and headless (idfon-home docs/group-live-chat.md,
# G0-G2): three real daemons share a room, and three headless members
# (crates/iroh-c-ffi/examples/group_member.rs) drive the apps' FFI: each
# publishes a tone, joins through its daemon, follows the roster, and records
# the mix. Real media over iroh, no phones.
#
# 1. All three stay: each must hear the other two tones.
# 2. One leaves mid-call: the other two still hear each other, and its tone
#    is gone from their mix.
# 3. One crashes (its member and its daemon killed, no leave): the other two
#    drop it by expiry and still hear each other.
#
#   scripts/group-call-e2e.sh
set -eu

root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$root"

pnpm cli build
cargo build --release -p iroh-c-ffi --example group_member
MEMBER="$root/target/release/examples/group_member"

work=$(mktemp -d /tmp/idfon-group-call.XXXXXX)
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
  eval "daemon_$who=$!"
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


member() { # WHO HZ ARGS...
  who=$1; hz=$2; shift 2
  "$MEMBER" --idfon "$NUF" --socket "$(sock "$who")" --room "$ROOM" --hz "$hz" "$@" \
    >"$work/$who-$hz.json" 2>"$work/$who-$hz.err" &
}

status=0
judge() { # FILE LABEL
  if [ "$(jq -r .ok "$1" 2>/dev/null)" = true ]; then
    echo "ok: $2 $(jq -c '{heard, members_final}' "$1")"
  else
    echo "FAIL: $2 $(cat "$1" 2>/dev/null)" >&2
    sed -n '1,40p' "${1%.json}.err" >&2
    status=1
  fi
}

# 1. Everyone stays.
member a 440 --seconds 14 --expect 660,990; pa=$!
sleep 1
member b 660 --seconds 13 --expect 440,990; pb=$!
sleep 1
member c 990 --seconds 12 --expect 440,660; pc=$!
wait $pa $pb $pc || true
judge "$work/a-440.json" "a hears b and c:"
judge "$work/b-660.json" "b hears a and c:"
judge "$work/c-990.json" "c hears a and b:"
sleep 2

# 2. b leaves mid-call.
member a 440 --seconds 16 --expect 990 --expect-absent 660; pa=$!
sleep 1
member b 660 --seconds 15 --leave-after 6; pb=$!
sleep 1
member c 990 --seconds 14 --expect 440 --expect-absent 660; pc=$!
wait $pa $pb $pc || true
for f in "$work/a-440.json" "$work/c-990.json"; do
  judge "$f" "after b left, $(basename "$f" .json):"
done

sleep 2

# 3. b crashes: member and daemon killed outright, nothing sent.
member a 440 --seconds 62 --expect 990 --expect-absent 660 --expect-members 1; pa=$!
sleep 1
member b 660 --seconds 60; pb=$!
sleep 1
member c 990 --seconds 60 --expect 440 --expect-absent 660 --expect-members 1; pc=$!
sleep 8
kill -9 "$pb" 2>/dev/null || true
kill -9 "$daemon_b" 2>/dev/null || true
echo "(b crashed at ~10 s; expiry should drop it within ~35 s)"
wait $pa $pc || true
for f in "$work/a-440.json" "$work/c-990.json"; do
  judge "$f" "after b crashed, $(basename "$f" .json):"
done

if [ "$status" = 0 ]; then
  echo "PASS: three headless members heard each other through one mix; a leaver and a crashed member dropped out"
else
  exit 1
fi
