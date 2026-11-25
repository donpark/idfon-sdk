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

## Checks

- Rust: `cargo check --workspace --all-targets`; `cargo test -p <crate>`.
- CLI end to end: `scripts/test-cli.sh`. Eve: the `scripts/eve-*-e2e.sh`
  scripts (mock models, no keys needed).
- TypeScript: `npm test` in `integrations/eve-idfon`.


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