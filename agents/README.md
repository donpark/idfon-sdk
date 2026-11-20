# Agents

Each directory is an Eve agent that becomes an idfon contact. `pnpm agent`
runs them:

```sh
pnpm agent list                 # every agent, its status, port and voice mode
pnpm agent start <agent>        # serve it (holder + bridge + Eve), prints pairing lines
pnpm agent stop|restart|kill <agent>
pnpm agent build|clean <agent|all>
```

## Add an agent

1. Copy `chat-echo/` (the smallest agent) to `agents/<name>/` and rename the
   package in `package.json`.
2. Edit `agent/agent.ts` (model) and `agent/instructions.md`. Add tools under
   `agent/tools/`.
3. Optional:
   - `live.json` to take voice calls (`docs/voice-agent.md`);
   - `serve.env` for serve defaults (see `monitor/serve.env`);
   - `idfon.json` for the agent's manifest.
4. `pnpm agent start <name>`.

No `scripts/<name>-serve.sh` is needed: any `agents/<name>/agent/agent.ts`
is served by `scripts/live-voice-serve.sh`. Add a serve script only for real
extra work, such as the agency's provisioner.

`serve.env` holds `KEY=VALUE` defaults, applied only when the variable is
unset. An empty `EVE_IDFON_MODEL=` keeps the agent's own model (e.g. a mock).
Only a gateway model id (`provider/model`) needs `AI_GATEWAY_API_KEY`.

## Talking to idfon from a tool

Use the shared client and envelope builders (`integrations/eve-idfon/README.md`):

```ts
import { bridge } from "eve-idfon/bridge";
import { envelopes } from "eve-idfon/envelopes";
import { emitEnvelope } from "eve-idfon/outbox";
```

Never hand-write `fetch` + `x-idfon-channel-secret`, or an `IDFON-*/1` string.
A tool that produces something for the caller calls `emitEnvelope`; it never
returns an `envelope` for the model to paste, and instructions never ask the
model to write one (`docs/reply-blocks.md`, "Who writes an envelope").

## Identity and secrets

Each instance keeps its state in `~/.idfon/<instance>/`:
- `holder.key`: its identity, 32 random bytes;
- `bridge-secret`: a random local bridge secret;
- `endpoint-port`: a pinned port, so pairing survives restarts.

Keys minted by older serve scripts came from `$RANDOM`, about 30 bits, which
is guessable. The serve script warns about those. Delete the key to mint a
strong one; callers must then re-pair.
