#!/usr/bin/env bash
# Rotate agent identity keys; pair running agents with the Apple apps.
#
#   pnpm agent:rekey <agent...|weak>   stop, move holder.key aside, start, then pair
#   pnpm agent:pair  <agent...|all>    pair running agents (any ~/.idfon/<home>)
#
# A new key is a new endpoint id, so every contact of a rekeyed agent must
# re-pair. This pairs the mac app (its daemon on IDFON_SOCKET) and one iPhone
# (IPHONE_UDID or IPHONE_NAME, else the only paired physical iPhone); other
# phones re-add the agent's card. The old key stays as holder.key.old-<time>.
set -euo pipefail

root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
. "$root/scripts/lib/core.sh"
cli="${IDFON_CLI:-$core_target/release/idfon}"
socket="${IDFON_SOCKET:-/tmp/idfon/idfond.sock}"
action=${1:-}
[ $# -gt 0 ] && shift

usage() { sed -n '2,10p' "$0" | sed 's/^# \{0,1\}//'; exit 2; }
note() { printf '%s\n' "$*" >&2; }
home_of() { printf '%s' "$HOME/.idfon/$1"; }
running() { local pid; pid=$(cat "$(home_of "$1")/holder.pid" 2>/dev/null) && kill -0 "$pid" 2>/dev/null; }
# Keys from the old serve script: `printf '%064d' $((RANDOM*RANDOM))`.
weak_key() { grep -qE '^0{40}' "$(home_of "$1")/holder.key" 2>/dev/null; }
served_agents() { bash "$root/scripts/agent.sh" list 2>/dev/null | sed -n 's/^Served agents: *//p'; }

rekey() {
  local agents=() a h stamp started
  if [ "${1:-}" = weak ]; then
    for a in $(served_agents); do weak_key "$a" && agents+=("$a"); done
    for h in "$HOME"/.idfon/*/holder.key; do
      a=$(basename "$(dirname "$h")")
      case " $(served_agents) " in *" $a "*) ;; *) weak_key "$a" && note "$a: weak key, but not an agent.sh agent; rekey it by hand (move holder.key aside, restart its launcher)" ;; esac
    done
  else
    agents=("$@")
  fi
  [ ${#agents[@]} -gt 0 ] || { note "nothing to rekey"; return 0; }
  # The agency first: the others register with it and admit its peer id at start.
  local ordered=()
  for a in "${agents[@]}"; do [ "$a" = agency ] && ordered=(agency); done
  for a in "${agents[@]}"; do [ "$a" = agency ] || ordered+=("$a"); done
  stamp=$(date +%Y%m%d-%H%M%S)
  started=$(date +%s)
  for a in "${ordered[@]}"; do
    case " $(served_agents) " in *" $a "*) ;; *) note "$a is not an agent.sh agent"; exit 2 ;; esac
    h=$(home_of "$a")
    bash "$root/scripts/agent.sh" stop "$a" >/dev/null 2>&1 || true
    [ -f "$h/holder.key" ] && mv "$h/holder.key" "$h/holder.key.old-$stamp"
    bash "$root/scripts/agent.sh" start "$a"
  done
  # Up = its serve script wrote pairing.txt after this run began (that is its
  # last step, after the new tickets). Builds are serialized, so allow minutes.
  for a in "${ordered[@]}"; do
    h=$(home_of "$a")
    for _ in $(seq 1 900); do
      [ -s "$h/pairing.txt" ] && [ "$(stat -f %m "$h/pairing.txt")" -ge "$started" ] && running "$a" && break
      sleep 1
    done
    running "$a" && [ "$(stat -f %m "$h/pairing.txt")" -ge "$started" ] || { note "$a did not come up; see $h/serve-manager.log"; exit 1; }
    note "$a: new key, endpoint $(head -1 "$h/holder.ticket" | jq -r .id | cut -c1-12)…"
  done
  pair "${ordered[@]}"
}

pair() {
  local agents=() a h contact pid name
  if [ "${1:-}" = all ]; then
    for h in "$HOME"/.idfon/*/holder.ticket; do a=$(basename "$(dirname "$h")"); running "$a" && agents+=("$a"); done
  else
    agents=("$@")
  fi
  [ ${#agents[@]} -gt 0 ] || usage

  # Only a daemon that is already listening: the CLI would otherwise spawn a blank one.
  local mac=""
  lsof -t "$socket" >/dev/null 2>&1 && mac=1
  [ -n "$mac" ] || note "no daemon on $socket (mac app not running): skipping the mac"
  local ios_endpoint=""
  [ -n "$mac" ] && ios_endpoint=$("$cli" --socket "$socket" peer show iphone --json 2>/dev/null | jq -r '.result.peer.endpoint_id // empty' || true)

  local device="${IPHONE_UDID:-${IPHONE_NAME:-}}"
  if [ -z "$device" ]; then
    local found
    found=$(xcrun devicectl list devices 2>/dev/null | grep -E 'physical' | grep -E 'connected|available \(paired\)' \
      | grep -oE '[A-F0-9]{8}-[A-F0-9]{16}|[A-F0-9]{8}-([A-F0-9]{4}-){3}[A-F0-9]{12}' || true)
    if [ "$(printf '%s\n' "$found" | grep -c .)" = 1 ]; then device=$found
    elif [ -n "$found" ]; then note "several iPhones; set IPHONE_UDID or IPHONE_NAME to pair one (xcrun devicectl list devices)"
    fi
  fi

  local ios_pairs=() tickets=() monitor=""
  for a in "${agents[@]}"; do
    h=$(home_of "$a")
    running "$a" && [ -s "$h/holder.ticket" ] || { note "$a is not running: skipped"; continue; }
    contact=$(head -1 "$h/holder.ticket")
    pid=$(printf '%s' "$contact" | jq -r .id)
    name=$(sed -n 's/.*--name "\([^"]*\)".*/\1/p' "$h/pairing.txt" 2>/dev/null | head -1)
    name=${name:-$a}
    if [ -n "$mac" ]; then
      "$cli" --socket "$socket" peer remove "$name" >/dev/null 2>&1 || true
      "$cli" --socket "$socket" peer add "$pid" --name "$name" --endpoint-id "$pid" --endpoint-addr "$contact" >/dev/null
      [ -s "$h/capability-ticket.json" ] \
        && defaults write com.idfon.mac idfon.capabilityTickets -dict-add "$pid" "$(<"$h/capability-ticket.json")"
      note "mac: $name -> ${pid:0:12}…"
    fi
    ios_pairs+=(-pair "$contact" "$name")
    if [ -n "$ios_endpoint" ] && [ -s "$h/capability-ticket-$ios_endpoint.json" ]; then
      if [ "$a" = monitor ]; then monitor="$pid|$h/capability-ticket-$ios_endpoint.json"
      else tickets+=("$pid|$h/capability-ticket-$ios_endpoint.json"); fi
    elif [ -n "$device" ]; then
      note "$a: no iPhone ticket (restart $a with the iphone peer in the mac daemon to mint one)"
    fi
  done
  [ -n "$device" ] && [ ${#ios_pairs[@]} -gt 0 ] || return 0

  # One launch adds every contact (requests run in order, then -pair-exit);
  # the app reads one -pair-ticket per launch, so tickets go one at a time.
  # The monitor goes last with -diagpeer: the apps keep the monitor by id.
  local want
  log=$(mktemp "${TMPDIR:-/tmp}/idfon-agent-pair.XXXXXX")
  trap 'rm -f "$log"' EXIT
  launch() { # <wait-for-pattern> <count> <args...>
    local pattern=$1 count=$2 launcher
    shift 2
    xcrun devicectl device process launch --device "$device" --terminate-existing --console \
      com.idfon.ios -- "$@" >"$log" 2>&1 &
    launcher=$!
    for _ in $(seq 1 300); do
      [ "$(grep -c "$pattern" "$log" || true)" -ge "$count" ] && break
      kill -0 "$launcher" 2>/dev/null || break
      sleep 0.2
    done
    kill "$launcher" 2>/dev/null || true
    wait "$launcher" 2>/dev/null || true
    grep -E 'idfon (pair|ticket) failed' "$log" >&2 || true
    [ "$(grep -c "$pattern" "$log" || true)" -ge "$count" ] || { note "iPhone: expected $count × '$pattern' (is it unlocked?)"; tail -5 "$log" >&2; return 1; }
  }
  want=$(( ${#ios_pairs[@]} / 3 ))
  launch 'idfon paired:' "$want" -pair-exit "${ios_pairs[@]}" && note "iPhone: $want contacts"
  [ -n "$monitor" ] && tickets+=("$monitor")
  local entry stored=0
  for entry in ${tickets[@]+"${tickets[@]}"}; do
    launch 'idfon ticket stored' 1 -pair-ticket "${entry%%|*}" "$(<"${entry#*|}")" && stored=$((stored + 1))
  done
  note "iPhone: $stored of ${#tickets[@]} tickets"
  # Leave the app running (and, for the monitor, keep -diagpeer in its arguments).
  xcrun devicectl device process launch --device "$device" --terminate-existing com.idfon.ios -- \
    ${monitor:+-diagpeer "${monitor%%|*}"} >/dev/null 2>&1 || true
}

case "$action" in
  rekey) [ $# -gt 0 ] || usage; rekey "$@" ;;
  pair) [ $# -gt 0 ] || usage; pair "$@" ;;
  *) usage ;;
esac
