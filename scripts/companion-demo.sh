#!/usr/bin/env bash
# A demo agent for cards (idfon-ios docs: ios-ui-audit.md §5.2, §5.3), built
# on `idfon card`. It plays "Luigi's", a restaurant:
#
#   companion-demo.sh actions PEER   Answer "Book a Table" requests from the
#                                    phone's + menu (IDFON-UI-ACTION/1,
#                                    card=compose) with a booking form, then
#                                    confirm. Runs until stopped.
#   companion-demo.sh call PEER      Answer PEER's next call and, while it
#                                    runs, show cards over their screen (the
#                                    call companion): how many people, then
#                                    a confirmation that changes in place.
#
# The phone shows + actions only when the agent's idfon.json lists them
# ("actions": [{"id": "book", "title": "Book a Table", "glyph": "fork.knife"}])
# and You → Developer → Contacts' Actions in + is on. A host profile serves no
# idfon.json, so for `actions` you can also ask from another shell:
#   idfon send luigi "$(printf 'IDFON-UI-ACTION/1\ncard=compose\naction=book')"
#
# Options come from the environment: IDFON (the CLI, default `idfon`) and
# IDFON_PROFILE (the agent's profile).
set -euo pipefail

idfon=${IDFON:-idfon}
mode=${1:-}
peer=${2:-}
[ -n "$mode" ] && [ -n "$peer" ] || { sed -n '2,24p' "$0"; exit 2; }

field() { python3 -c 'import json,sys; print(json.load(sys.stdin).get(sys.argv[1], ""))' "$1"; }
value() { python3 -c 'import json,sys; print(json.load(sys.stdin)["values"].get(sys.argv[1], ""))' "$1"; }

book() {
  local answer action name people
  answer=$("$idfon" card send "$peer" "Book a table" --subtitle "Luigi's" --glyph fork.knife \
    --field name=Name --field people=People --field time=Time \
    --action book=Book --action cancel="Never Mind" --danger cancel --wait --json)
  action=$(field action <<<"$answer")
  if [ "$action" != "book" ]; then
    "$idfon" card send "$peer" "No booking" --subtitle "Maybe next time" --glyph fork.knife >/dev/null
    return
  fi
  name=$(value name <<<"$answer")
  people=$(value people <<<"$answer")
  time=$(value time <<<"$answer")
  "$idfon" card send "$peer" "Booked: ${people:-2} at ${time:-7:30}" \
    --subtitle "Under ${name:-your name}. See you at Luigi's." --glyph checkmark.seal >/dev/null
  echo "booked ${people:-2} at ${time:-7:30} for ${name:-?}"
}

case "$mode" in
  actions)
    echo "waiting for + actions from $peer (Ctrl-C to stop)"
    while true; do
      answer=$("$idfon" card wait "$peer" --card compose --timeout 1h --json) || continue
      case "$(field action <<<"$answer")" in
        book) book ;;
        *) "$idfon" card send "$peer" "Sorry, Luigi's can only book tables" >/dev/null ;;
      esac
    done
    ;;
  call)
    echo "call me from $peer"
    "$idfon" answer --from "$peer" --timeout 10m --duration 10m -o /dev/null &
    answering=$!
    trap 'kill $answering 2>/dev/null || true' EXIT
    # The phone's call invite comes first; give the call a moment to connect.
    "$idfon" wait --type message.received --timeout-ms 600000 >/dev/null
    sleep 3
    id="luigi-$$"
    answer=$("$idfon" card send "$peer" "Table for how many?" --sheet --id "$id" --glyph person.2 \
      --action 2=Two --action 4=Four --action 6=Six --wait --timeout 5m --json)
    people=$(field action <<<"$answer")
    "$idfon" card send "$peer" "Table for $people at 7:30" --sheet --id "$id-done" --glyph checkmark.seal \
      --subtitle "Booked. Anything else?" --action thanks="That's All" --wait --timeout 5m --json >/dev/null || true
    echo "booked a table for $people during the call"
    wait "$answering" || true
    ;;
  *) sed -n '2,24p' "$0"; exit 2 ;;
esac
