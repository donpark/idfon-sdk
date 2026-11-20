# eve-idfon

Eve ingress **channel** that makes an idfon peer a first-class Eve agent
contact. The caller authenticates with its idfon endpoint key instead of a
bearer token, and the agent needs no public endpoint, DNS name, or TLS cert.

A human idfon client and another agent reach the channel the same way: they
send `message.send` over iroh, the provider's endpoint holder verifies the
peer, and the turn lands in an Eve session with the peer as the principal.

## Requirements

- Node.js >= 24 (Eve's requirement)
- A holder binary for your platform. It ships as an optional platform package
  (`eve-idfon-{darwin,linux}-{arm64,x64}`) and is resolved
automatically; there is no Windows package (the holder's IPC is a Unix socket).
  For an unsupported platform or a self-built holder, pass `--holder-command`
  (or set `EVE_IDFON_HOLDER`) pointing at
  `cargo build --release -p eve-idfon` output from the
  [idfon-sdk workspace](https://github.com/donpark/idfon-sdk).

## Install

```sh
npm install eve-idfon
```

With pnpm inside the idfon workspace:

```sh
pnpm add eve-idfon --filter <your-eve-app>
```

## Use in an Eve app

```ts
// agent/extensions/idfon.ts
import idfon from "eve-idfon";

export default idfon({
  bridgeUrl: "http://127.0.0.1:18766",
  secret: process.env.IDFON_BRIDGE_SECRET!,
});
```

`bridgeUrl` must point at the local bridge (loopback only). The channel
handler requires the same `secret` on every request.

The `idfon__send`, `idfon__put`, `idfon__publish-live`, and
`idfon__stop-live` tools are available at `eve-idfon/tools`.

**Tools deliver; the model never writes envelopes.** `idfon__put`,
`idfon__publish-live`, `issue-card`, `speak`, `add_artifact` and the view
tools attach their `IDFON-*/1` envelope to the turn's reply automatically.
To hand something to a *different* peer, call `idfon__send` with
`attach: [{kind: "data" | "live" | "card", ...}]`. The full rule is in
`docs/reply-blocks.md` ("Who writes an envelope").

### Calling the bridge from your own tools

Use the shared client instead of hand-writing `fetch` + the secret header:

```ts
import { bridge, createBridge } from "eve-idfon/bridge";   // typed bridge client
import { envelopes } from "eve-idfon/envelopes";          // every IDFON-*/1 string
import { emitEnvelope } from "eve-idfon/outbox";          // deliver with the reply

const { ticket, size_bytes } = await bridge.putBlob(bytes.toString("base64"));
emitEnvelope(ctx.session.id, envelopes.data(ticket, size_bytes));
```

`bridge` reads the extension config, else `IDFON_BRIDGE_URL` /
`IDFON_BRIDGE_SECRET` (the older `EVE_IDFON_BRIDGE_URL`, `EVE_IDFON_SECRET`
and `IDFON_CHANNEL_SECRET` still work). A non-2xx response throws
`BridgeError` with the path, status and the bridge's message.

## Run the managed sidecar

`managed.mjs` owns the holder and bridge as child processes, takes a lock on
the socket, forwards termination, and cleans up on exit. Eve 0.55 has no
custom-channel startup hook, so this is the deployment entrypoint.

```sh
npx eve-idfon \
  --key-file  ~/.config/idfon/eve.key \
  --socket    /run/user/$UID/idfon-eve.sock \
  --blob-dir  ~/.local/share/idfon/eve-blobs \
  --target    http://127.0.0.1:52776 \
  --secret    "$IDFON_BRIDGE_SECRET" \
  --port      18766
```

`--holder-command` defaults to the installed platform package's binary (with a
repo-layout fallback for checkouts). Pass `--allow <PEER_ID>` (repeatable) to
admit peers, and `--live-ttl-secs` to cap abandoned live publishers
(default 3600).

## License

MIT OR Apache-2.0. See `LICENSE-MIT` and `LICENSE-APACHE`.