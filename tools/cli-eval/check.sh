#!/bin/sh
# Grades the tasks a fixture's agent attempted and summarizes calls.log.
#   check.sh DIR TASK...      e.g. check.sh /tmp/idfe-1 t1 t2 t9
# Prints one PASS/FAIL line per task, then call counts. Tasks whose result is
# only in the agent's answer (t2, t5, t9, t11) are marked ANSWER: grade those
# from the agent's report.
set -u
root=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
dir=$1
shift
A="$dir/a/s.sock"
Q() { "$root/target/release/idfon" --socket "$A" "$@"; }
inbox() { cat "$dir/bob-inbox.jsonl" 2>/dev/null; }
w="$dir/work"
result() { if eval "$2" >/dev/null 2>&1; then echo "PASS $1"; else echo "FAIL $1"; fi; }

for t in "$@"; do
  case $t in
    t1) result t1 "Q peer list --json | jq -e '[.result.peers[].name] | index(\"carol\")'" ;;
    t2) result t2 "inbox | jq -e 'select(.text == \"hello\")'"; echo "ANSWER t2: expect 'pong: hello'" ;;
    t3) result t3 "inbox | grep -q 'IDFON-DATA/1'" ;;
    t4) result t4 "cmp -s '$w/received.txt' '$dir/b/bobfile.txt'" ;;
    t5) echo "ANSWER t5: verify with: Q get <ticket>" ;;
    t6) result t6 "cmp -s '$w/out.bin' '$dir/b/shared.txt'" ;;
    t7) result t7 "Q identity list --json | jq -e '(.result.identities | map(.name) | index(\"travel\")) and (.result.identities[] | select(.active) | .name != \"travel\")'" ;;
    t8) result t8 "test -s '$w/ticket-for-bob.txt'" ;;
    t9) echo "ANSWER t9: expect yes" ;;
    t10) result t10 "grep -q 'stream\\|live' '$dir/calls.log' && Q send --stream --list --json | jq -e '(.result.ids // []) | length == 0'" ;;
    t11) echo "ANSWER t11: expect 'lunch at noon?' and 'bring the slides'" ;;
    t12) result t12 "Q room list --json | jq -e '[.result.rooms[].name] | index(\"team\")' && inbox | jq -e 'select(.text == \"standup\" and .conversation != null)'" ;;
  esac
done
calls=$(wc -l <"$dir/calls.log" | tr -d ' ')
failed=$(awk -F'\t' '$1 != 0' "$dir/calls.log" | wc -l | tr -d ' ')
helps=$(grep -cE '(^|[[:space:]])(-h|--help|help)([[:space:]]|$)' "$dir/calls.log")
echo "calls=$calls failed=$failed help=$helps"
