# Agent notes: idfon-sdk

## Documentation

Start at **`docs/README.md`** (the index) and read only the entries relevant to
the task. Do not bulk-read `docs/`. If you add or materially change a doc,
update the index.

## Layout

- `crates/`: idfon-cli (the `idfon` binary; `idfon mcp server` serves its
  commands as MCP tools), eve-idfon (the Eve holder), idfon-voice-agent (the
  `eve-idfon-voice` runner), idfon-live-media, idfon-radio. They depend on idfon-core crates by path
  (`../idfon-core/crates/...`).
- `cli/`: the npm packaging (`idfon` shim + per-platform packages);
  `scripts/build-cli.sh` fills them.
- `integrations/`: the eve-idfon extension (bridge client, envelopes, outbox,
  tools) and eve-idfon-voice. `agents/`: the Eve agents; `agents/README.md`
  says how to add one.
- `skills/idfon/SKILL.md`: the CLI skill; `tools/cli-eval/`: the agent eval.
- `scripts/`: end-to-end tests (`test-cli.sh`, `test-e2e.sh`, `eve-*-e2e.sh`,
  …), agent serving (`agent.sh`, `live-voice-serve.sh`), CLI packaging.

## idfon-core

This repo builds a sibling idfon-core checkout (default `../idfon-core`, or
`IDFON_CORE`). Scripts source `scripts/lib/core.sh`, which sets `$core`,
`$core_target` (the shared target dir) and `core_cargo` (cargo inside core).
Use `core_cargo build -p idfond` / `-p iroh-c-ffi`; `cargo build -p idfon-cli`
for this repo's crates. A core change that breaks this repo is fixed in core.

## Security invariants

From the security review of 2026-11-26 (idfon-home
`docs/security-review-2026-11-26.md`). Peer text (messages, names, cards,
fetched pages) reaches models through every agent and tool here, so treat any
model-chosen value as attacker-chosen.

- **A new CLI command is an MCP tool automatically.** `idfon mcp server`
  generates its tools from the clap definitions. Before adding a command,
  decide whether a prompt-injected model may run it: a command that
  reconfigures the daemon, grants capabilities, serves or publishes local
  files, writes to a caller-chosen path, or sends stored tokens goes in
  `EXCLUDED` in `crates/idfon-cli/src/mcp_server.rs` (or waits for an
  out-of-model approval). `rpc`, `config set`/`unset`, `provider` and `gateway`
  are excluded, and `REFUSED_CAPABILITIES` keeps `mcp.transport` out of
  `access grant`/`ticket`: together they once handed a peer `sh -c`. Tests:
  `every_command_is_a_tool_except_the_excluded`,
  `tools_cannot_grant_mcp_transport`; never delete an entry from their lists to
  make a test pass.
- **Agent tools take no host paths or free-form command lines from the model.**
  Map model choices onto operator-owned values (an id from a list, a file
  under the instance's content dir); a model-chosen path or URL is a file read
  or SSRF for whoever messages the agent.
- **Keys and secrets.** Generate keys and secrets with `openssl rand` under
  `umask 077` and keep the files 0600; never `$RANDOM` (eleven holder keys
  made that way were brute-forceable). Pass secrets by environment or file,
  never on a command line (`ps` shows it to every local user), and never fall
  back to a default secret.

## Agent pairing

Read this before pairing, renaming, rekeying or retiring an agent.
`docs/live-voice.md` ("Pairing the Apple apps") has the manual steps.

- **Use the scripts.** `pnpm agent:pair <name...|all> [--drop NAME]...` pairs
  running agents with the mac app and one iPhone; `pnpm agent:rekey
  <name...|weak>` rotates identity keys (stop, move `holder.key` aside, start,
  wait, pair). `scripts/agent-keys.sh` holds what they learned:
  - mac: the contact goes into the daemon on `/tmp/idfon/idfond.sock`
    (replacing one of the same name); the script never talks to a socket
    nobody listens on, since the CLI would spawn a blank daemon there. The
    holder ticket goes in `defaults com.idfon.mac idfon.capabilityTickets`.
  - iPhone: all contacts in one launch (`-unpair`/`-pair`, then `-pair-exit`),
    then **one launch per ticket**: the app reads only the first
    `-pair-ticket`. The phone must be unlocked (`Locked` launch errors).
  - monitor: the apps keep the diagnostics monitor **by id**
    (`idfon.diag.peer`), so a rekeyed monitor is re-set with `-diagpeer`.
- **Which iPhone.** `IPHONE_NAME` (or `IPHONE_UDID`) in the gitignored `.env`
  names the owner's phone. Other phones paired with this Mac belong to test
  users, who install TestFlight builds; never pair agents to them unless the
  owner asks.
- **Names.** A contact is named by its home's `pairing.txt` (`--name`, from
  `EVE_CONTACT_NAME`, else the instance). When a name changes, or an agent is
  retired, pass the old name with `--drop` so no stale contact stays behind.
- **A new key is a new endpoint id.** Every holder of the old card must re-add
  it (the scripts cover the mac and one iPhone). Rekey the agency first: the
  others admit its peer id and register with it at start.
- **Homes** (`~/.idfon/<instance>`) come from three launchers: `pnpm agent`
  (the agents in `agents/`), saved profiles (`instance.env`, e.g. agency
  roster runs: `EVE_INSTANCE=<instance> AGENCY_URL=http://127.0.0.1:18777
  scripts/llm-serve.sh`, see `agents/agency/roster.json`), and radio stations
  (`scripts/radio-serve.sh <instance>`). A home with no `holder.key` mints a
  strong one at its next start. Retired homes move to `~/.idfon/retired/`.

## Checks

- Rust: `cargo check --workspace --all-targets`; `cargo test -p <crate>`.
- CLI end to end: `scripts/test-cli.sh`. Eve: the `scripts/eve-*-e2e.sh`
  scripts (mock models, no keys needed).
- TypeScript: `npm test` in `integrations/eve-idfon`.
- Serving agents (`pnpm agent`, `scripts/live-voice-serve.sh`): read "Serving
  agents: what must hold" in `docs/live-voice.md` before changing them or
  debugging a start. Keys come from the gitignored `idfon-sdk/.env`; don't
  search shell profiles or secrets files for them.


- **Verify before retrying.** For network, device, or model checks, keep the returned operation ID and inspect that operation and its events before sending again. Reusing an idempotency key returns the existing operation; it does not re-run it. A new key creates a new request and may create another model turn.
- **MAST repeat/contradiction warning.** Stop and reconcile the latest tool result with the current process/config state before taking another action. Do not repeat the last command; state what evidence changed or switch to a read-only check.

- Rebuild scope: `docs/build.md` maps a change to the subprojects that must be
  rebuilt. When you change something shared, say which subprojects to rebuild.
- **Debugging calls/wakes/push: ask, don't grep.** Before any device-log
  archaeology (`devicectl` pulls, sysdiagnose, timestamp reconstruction), query
  the diagnostics the apps already report: `wake.trace` (one call's timeline:
  registration → ring → push → report → answer → media) and `diag.tail` via
  `idfon-mcp`; raw views at `<data-dir>/diag/`; edge health at `/_push/health`.
  See `docs/mcp-control.md` + `docs/diag.md`. (2026-11-10: hours were lost
  reconstructing from USB logs what the app was already reporting.)
- **No identifiers in public artifacts.** This repo is public, as is idfon-core;
  the apps, idfon-edge, idfon-home, idfon-site and the archive are private. Keep
  APNs key ids, push tokens, peer/endpoint ids, call UUIDs, droplet IPs and SSH
  key names out of commits, PR titles/bodies, issues and docs. Use placeholders
  (`<key-id>`, `<peer>`) and local files (`~/.idfon/edge.apns.keyid`).
- **Privacy posture lives in `README.md` ("Privacy").** Keep any new surface
  consistent with it: content end-to-end between the parties, the edge holds
  wake tokens + brief invites only, APNs payloads are identifiers-only,
  diagnostics are markers to a user-configured monitor, and control surfaces
  are tier-approved over iroh. If a change would add a new party to the table,
  update that table first.