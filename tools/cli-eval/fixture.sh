#!/bin/sh
# CLI usability eval fixture: two local daemons ("me" and "bob"), a bob that
# auto-replies, and a logging `idfon` wrapper for the agent under test.
#
#   fixture.sh up DIR     start daemons, seed state, print DIR
#   fixture.sh down DIR   stop everything
#
# DIR must be short (unix socket path limit): use /tmp/idfe-<n>.
# The agent runs `idfon` from DIR/bin; every call and its exit code is
# appended to DIR/calls.log. Bob logs what he receives to DIR/bob-inbox.jsonl.
set -eu

root=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
NUF="$root/target/release/idfon"
IDFOND="$root/target/release/idfond"
cmd=${1:?usage: fixture.sh up|down DIR}
dir=${2:?usage: fixture.sh up|down DIR}

if [ "$cmd" = down ]; then
  [ -f "$dir/pids" ] && kill $(cat "$dir/pids") 2>/dev/null || true
  pkill -f "$dir/" 2>/dev/null || true
  exit 0
fi

rm -rf "$dir"
mkdir -p "$dir/a" "$dir/b" "$dir/bin" "$dir/work"
A="$dir/a/s.sock"
B="$dir/b/s.sock"
"$IDFOND" --socket "$A" --data-dir "$dir/a" >"$dir/a/log" 2>&1 &
echo $! >>"$dir/pids"
"$IDFOND" --socket "$B" --data-dir "$dir/b" >"$dir/b/log" 2>&1 &
echo $! >>"$dir/pids"
QA() { "$NUF" --socket "$A" "$@"; }
QB() { "$NUF" --socket "$B" "$@"; }
for s in "$A" "$B"; do
  for _ in $(seq 1 100); do
    "$NUF" --socket "$s" status --json >/dev/null 2>&1 && break
    sleep 0.1
  done
done

# Peers both ways (A calls B "bob"; B calls A "alice") plus message grants.
A_EP=$(QA status --json | jq -r .result.identity.endpoint_id)
A_PID=$(QA status --json | jq -r .result.identity.public_key)
A_ADDR=$(QA status --json | jq -r '.result.ticket | implode')
B_EP=$(QB status --json | jq -r .result.identity.endpoint_id)
B_PID=$(QB status --json | jq -r .result.identity.public_key)
B_ADDR=$(QB status --json | jq -r '.result.ticket | implode')
QA peer add "$B_PID" --name bob --endpoint-id "$B_EP" --endpoint-addr "$B_ADDR" >/dev/null
QB peer add "$A_PID" --name alice --endpoint-id "$A_EP" --endpoint-addr "$A_ADDR" >/dev/null
for cap in message.send message.receive; do
  QA access allow --subject "$B_PID" --capability $cap >/dev/null
  QB access allow --subject "$A_PID" --capability $cap >/dev/null
done

# Files the tasks refer to.
printf 'Meeting notes\n- ship the CLI\n' >"$dir/work/notes.txt"
printf 'quarterly numbers: 42\n' >"$dir/b/bobfile.txt"
python3 - "$dir/work/song.wav" <<'EOF'
import math, struct, sys, wave
with wave.open(sys.argv[1], "wb") as w:
    w.setnchannels(1); w.setsampwidth(2); w.setframerate(48000)
    w.writeframes(b"".join(struct.pack("<h", int(8000 * math.sin(2 * math.pi * 440 * i / 48000))) for i in range(48000 * 3)))
EOF
printf 'shared payload\n' >"$dir/b/shared.txt"
QB put --file "$dir/b/shared.txt" >"$dir/work/ticket.txt"
# Carol: a third daemon (there is one identity per daemon now), exported as a
# vCard for the add-contact task.
mkdir -p "$dir/c"
C="$dir/c/s.sock"
"$IDFOND" --socket "$C" --data-dir "$dir/c" >"$dir/c/log" 2>&1 &
echo $! >>"$dir/pids"
for _ in $(seq 1 100); do
  "$NUF" --socket "$C" status --json >/dev/null 2>&1 && break
  sleep 0.1
done
"$NUF" --socket "$C" identity rename carol >/dev/null
"$NUF" --socket "$C" contact export --out "$dir/work/carol.vcf"
# Two earlier messages from bob.
QB send alice --text "lunch at noon?" >/dev/null
QB send alice --text "bring the slides" >/dev/null

# Bob: reply "pong: <text>" to texts; on "send file", send bobfile.txt.
cat >"$dir/bob.sh" <<EOF
#!/bin/sh
cursor=\$("$NUF" --socket "$B" events --json | jq -r '.result.events | last | .cursor // ""')
while :; do
  r=\$("$NUF" --socket "$B" wait --type message.received --after "\$cursor" --timeout-ms 30000 --json 2>/dev/null) || { sleep 0.5; continue; }
  n=\$(printf "%s" "\$r" | jq '.result.events | length')
  [ "\$n" = 0 ] && continue
  cursor=\$(printf "%s" "\$r" | jq -r '.result.events | last | .cursor')
  printf "%s" "\$r" | jq -c '.result.events[] | .data' >>"$dir/bob-inbox.jsonl"
  printf "%s" "\$r" | jq -r '.result.events[] | .data.text // "" | @base64' | while read -r b64; do
    text=\$(echo "\$b64" | base64 -d)
    case "\$text" in
      IDFON-*) ;;
      "send file") "$NUF" --socket "$B" send alice --file "$dir/b/bobfile.txt" >/dev/null 2>&1 & ;;
      *) "$NUF" --socket "$B" send alice --text "pong: \$text" >/dev/null 2>&1 ;;
    esac
  done
done
EOF
chmod +x "$dir/bob.sh"
"$dir/bob.sh" >"$dir/bob.log" 2>&1 &
echo $! >>"$dir/pids"

# The agent's `idfon`: fixed socket, every call logged.
cat >"$dir/bin/idfon" <<EOF
#!/bin/sh
"$NUF" --socket "$A" "\$@"
code=\$?
printf '%s\t%s\n' "\$code" "\$*" >>"$dir/calls.log"
exit \$code
EOF
chmod +x "$dir/bin/idfon"
: >"$dir/calls.log"
echo "$dir"
