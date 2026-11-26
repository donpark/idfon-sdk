# idfon-sdk

What developers use to build on Idfon:
the `idfon` CLI (and its npm packages), the MCP server, the Eve integration and
agents, the voice-agent runner, and the end-to-end tests that drive the daemon
through the CLI.

> **Status:** experimental and pre-release. The CLI is available as an npm
> package.

## The idfon repositories

Clone them side by side (for example in `~/dev`):

| Repo | What it holds |
| --- | --- |
| [idfon-core](https://github.com/donpark/idfon-core) | protocol, identity, transport, media, the daemon (`idfond`), the C ABI the apps link, and the Swift packages both apps use |
| [idfon-sdk](https://github.com/donpark/idfon-sdk) | the `idfon` CLI, the MCP server, the Eve integration and agents, the voice-agent runner, and the end-to-end tests |
| idfon-ios, idfon-mac (private) | the iPhone and macOS apps |
| idfon-edge (private) | the `idfon.net` edge server, its deployments, and the relay auth service |
| idfon-home, idfon (private) | product docs and planning; the pre-split archive and model R&D |

idfon-sdk and both apps build idfon-core from `../idfon-core`; set
`IDFON_CORE` to use another checkout.

## Building

```sh
# idfon-core checked out beside this repo (or IDFON_CORE=/path/to/idfon-core)
cargo check --workspace --all-targets
scripts/test-cli.sh        # builds core's idfond + dylib and this repo's CLI
pnpm install && pnpm build
```

Both cargo workspaces build into `../idfon-core/target` (see
`.cargo/config.toml`), so `idfon`, `idfond` and `libiroh_c_ffi` land side by
side; the CLI starts the daemon from its own directory.

## CLI installation

The npm package includes prebuilt binaries for macOS (arm64, x64) and Linux
(x64, arm64). The launcher requires Node.js 18 or later.

```sh
npx idfon status
npm install --global idfon
```

Windows is not supported yet. To build from source, install Rust and run:

```sh
cargo build --release -p idfon-cli -p idfond
```

The resulting binaries are `target/release/idfon` and
`target/release/idfond`.

## Daemon

`idfon` is a client for `idfond`, which owns the iroh endpoint and persistent
local state. The CLI starts `idfond` when needed and connects to it through a
Unix socket. By default the socket is `/tmp/idfon/idfond.sock`.

An automatically started daemon exits after its idle timeout. Use
`--keep-alive` to disable that behavior, or run `idfon shutdown` to stop it.
The daemon is also used by the native clients, so the CLI and an app can share
an identity and peer state.

## CLI

Run `idfon --help` for the complete command list. Common commands:

```sh
# Inspect and manage local state
idfon status
idfon identity list            # one you; contacts get tags instead
idfon tag add bob Work         # tags decide what contacts see (presence, photo)
idfon card send bob "Lunch?" --action yes=Yes --action no=No --wait   # buttons; prints the answer
idfon peer list
idfon peer add <endpoint-id-or-address> --name bob

# Store data and retrieve it by ticket
idfon put --file recording.wav > ticket.txt
idfon get "$(cat ticket.txt)" --out copy.wav

# Send text or a signaled file to a peer
idfon send bob --text "hello"
idfon send bob --file notes.pdf
idfon recv --out notes.pdf

# Publish or receive live audio
idfon send --stream --file audio.wav --loop > live-ticket.txt
idfon get "$(cat live-ticket.txt)" --out capture.wav

# A session-scoped 1:1 stream
idfon send bob --stream --file audio.wav
idfon recv --stream --out reply.wav
```

`put` stores data and prints a `BlobTicket`. `get` fetches a blob ticket to
stdout or a file. The CLI chunks transfers and the daemon stores them. The
data does not go through the chat message path.

`send PEER --file` stores a blob and sends its ticket as a peer message.
`recv` waits for that message, then fetches the blob. Text and file delivery
require the relevant local capability grants. See
[CLI data transfer](docs/cli-data.md) for pairing and transfer details.

`send --stream` without a peer publishes live audio and prints an
`iroh-live:` ticket. With a peer it opens a session-scoped 1:1 stream instead.
`recv --stream` accepts that stream. The CLI supports file sources. The native
clients provide microphone capture, not this command. Use
`send --stream --video` for video broadcasts. See
[video media](docs/video-media.md).

Useful operational commands:

```sh
idfon events --follow
idfon wait --type message.received --timeout-ms 60000
idfon operation get <operation-id>
idfon access check --subject <peer-id> --capability message.receive
idfon mcp listen --to <peer>
idfon shutdown
```

Someone who adds you without being your contact reaches you as a **contact
request**: their plain-text messages are held (capped) and nothing is accepted
automatically. `idfon request list` shows them, `idfon request accept REQ
[--name N] [--conversation ROOM]` adds the sender and delivers the held
messages as ordinary `message.received` events, and `idfon request decline REQ
[--block]` drops them. See "Contact requests" in idfon-core's
`docs/protocol.md`.

Use `--json` for response envelopes intended for scripts. `--socket`,
`--identity`, and `--keep-alive` are global options.

## MCP transport

Idfon includes an MCP transport bridge. It carries MCP stdio traffic over the
`idfon/mcp/1` iroh protocol. It does not implement MCP semantics.

```sh
# On the MCP-server side
idfon-mcp serve --command '<local-mcp-server>'

# On the client side
idfon-mcp connect --peer '<peer-ticket>'
```

`idfon-mcp --help` shows the exact bridge options. The daemon also has
`idfon mcp listen` and `idfon mcp configure` commands for daemon-backed
connections. This area is experimental. See
[MCP transport](docs/mcp-transport.md).

To give an agent idfon itself as tools, run `idfon mcp server` (stdio): every
CLI command is a tool (`peer add` is `peer_add`), and the skill
(`idfon skill`) is the server's instructions:

```json
{ "mcpServers": { "idfon": { "command": "idfon", "args": ["mcp", "server"] } } }
```

## Eve extension and agents

Build the `eve-idfon` extension and the agents under `agents/` separately from
the native clients. `pnpm eve` builds the extension first, then every
installed agent:

```sh
pnpm eve build      # eve-idfon + all agents
pnpm eve clean      # remove dist/.output everywhere
```

Use `pnpm agent` to manage agents. Keys such as `AI_GATEWAY_API_KEY` go in the
gitignored `idfon-sdk/.env`, which the scripts load. If `restart all` leaves an
agent stopped, see "Serving agents: what must hold" in
[docs/live-voice.md](docs/live-voice.md).

```sh
pnpm agent build <name|all>     # eve build in agents/<name>
pnpm agent clean <name|all>
pnpm agent start <name|all>     # any agents/<name>/agent/agent.ts
pnpm agent restart <name|all>   # stop then start
pnpm agent stop <name|all>
pnpm agent list
```

The agents under `agents/` are:

| Agent | Served | What it is |
| --- | --- | --- |
| `gpt-live-1` | yes | GPT-Live full-duplex voice agent (audio/video calls, artifacts) |
| `llm` | yes | text-only agent reached by voice. The model is per-instance, so one agent can back several contacts |
| `agency` | yes | go-between. It introduces registered agents (A2A card relay) and signs nothing |
| `chat-echo` | no | deterministic echo mock used by the e2e scripts and `eve-spike` |

See [live-voice agent](docs/live-voice.md),
[voice side channel](docs/voice-side-channel.md) (the `llm` agent and
multi-contact model selection), [agent agency](docs/agent-agency.md),
and [Eve ingress channel](docs/idfon-eve.md).

## Documentation

Start at [docs/README.md](docs/README.md). The CLI's agent skill is
[skills/idfon/SKILL.md](skills/idfon/SKILL.md); `tools/cli-eval` measures how
easily an agent uses the CLI from it.

## License

Dual-licensed under [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE).
