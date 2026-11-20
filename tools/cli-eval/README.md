# CLI usability eval

Measures how easily an agent uses the `idfon` CLI from a skill alone. A
fluent CLI for an agent with only a skill and `--help` is a fluent CLI for
people too.

## Run

```sh
cargo build --release -p iroh-c-ffi -p idfond -p idfon-cli
sh tools/cli-eval/fixture.sh up /tmp/idfe-1      # one fixture per agent
# give a fresh agent agent-prompt.md with {SKILL}, {FIX}, {TASKS} filled in
sh tools/cli-eval/check.sh /tmp/idfe-1 t1 t2 t9  # grade + call counts
sh tools/cli-eval/fixture.sh down /tmp/idfe-1
```

- `fixture.sh`: two daemons (you and `bob`), bob auto-replies `pong: <text>`
  and sends a file on `send file`; a logging `idfon` wrapper (`calls.log`).
- `tasks.md`: 12 tasks in 4 groups; `skill-baseline.md` is the skill for the
  pre-reshape CLI, `skills/idfon/SKILL.md` the current one.

## Results (2026-11-19, Claude Haiku, one run per group)

All 12 tasks were completed in both rounds. Before, two needed help: t4 only
worked by reading the ticket out of the raw message and running `get` (the
`recv --from bob` path failed), and t8 used the wrong capability.

| group | tasks | before: calls / help / failed | after: calls / help / failed |
| --- | --- | --- | --- |
| 1 | add contact, message + reply, permission | 17 / 7 / 0 | 7 / 1 / 0 |
| 2 | send file, receive file, inbox | 13 / 4 / 1 | 8 / 4 / 0 |
| 3 | share ticket, download, live broadcast | 15 / 5 / 1 | 9 / 3 / 0 |
| 4 | identities, access ticket, room | 24 / 8 / 1 | 20 / 7 / 0 |
| **total** | | **69 / 24 / 3** | **44 / 15 / 0** |

Friction the baseline exposed, and the fix:

| baseline friction | fix |
| --- | --- |
| waiting for a reply needed cursors; `wait` returned old events | `send PEER TEXT --wait`; `wait` defaults to new events |
| `recv --from bob` never matched (compared raw ids); `recv` missed files sent before it started | names resolve; `recv` takes a file from the last 2m, else waits |
| `send --stream --file song.wav` failed (daemon resolved the relative path) | CLI resolves paths |
| `access ticket` printed `null` with exit 0; date-only expiry rejected; subject had to be a key | real errors; `--expires 2026-12-31 / 30d`; `access ticket bob` |
| "let bob message you" picked `message.send` | default capability `message.receive`, said in help |
| `room send` needed the room id | rooms resolve by name |
| `send --file` printed only a ticket; raw JSON everywhere | short human lines; `--json` for machines |
| relay flags cluttered every `--help` | hidden; examples in top-level help |

Single run per group, so treat small differences as noise; the totals and
the removed failure modes are the signal.
