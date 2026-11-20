# Crates

What each crate is for, one line each. Every crate has a `README.md` with the
detail; `docs/README.md` indexes the design docs.

These crates build on idfon-core's crates (protocol, core, client, media, voice,
h3, director), which live in the sibling idfon-core repo.

## Daemon and its clients

| Crate | What it is |
| --- | --- |
| `idfon-cli` | The `idfon` CLI: noun-verb subcommands mapped one-to-one onto daemon methods. |

## MCP

| Crate | What it is |
| --- | --- |
| `idfon-mcp-server` | MCP *server* adapter (stdio), a separate process over `idfon-client`; consent decisions stay in the daemon. |

## Media

| Crate | What it is |
| --- | --- |
| `idfon-live-media` | MoQ plumbing shared by live-call handlers (parse an invite, subscribe PCM, publish and sign the return leg). Provider-neutral. |
| `idfon-radio` | A talk-radio programme: the schedule, its clips, and the frames a transport pumps. Call-relative, not a timer. |

## Voice

| Crate | What it is |
| --- | --- |
| `idfon-voice-agent` | The voice-agent kit and `eve-idfon-voice`: one config-driven runner, pluggable backends (cascade, GPT-Live, local). |
| `eve-idfon` | A generic Eve ingress endpoint holder (`eve-idfon`); live calls are a handler seam this binary leaves unregistered. |

## Building one

```sh
cargo test -p <crate>            # the crate
cargo check --workspace --all-targets
```

`docs/build.md` maps a change to the subprojects that must be rebuilt — the Apple
apps and the c-FFI artifact have an ordering that a bare `cargo build` does not
express.
