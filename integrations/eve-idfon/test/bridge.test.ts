// npm test  (builds, then runs against dist: the extension imports resolve there)
import assert from "node:assert/strict";
import { createServer } from "node:http";
import { test } from "node:test";

import { BridgeError, createBridge } from "../dist/extension/bridge.mjs";
import { envelopeField, envelopes } from "../dist/extension/envelopes.mjs";

async function withServer(
  handler: (path: string, secret: string | undefined, body: unknown) => [number, unknown],
  run: (url: string) => Promise<void>,
) {
  const server = createServer((req, res) => {
    let data = "";
    req.on("data", (chunk) => (data += chunk));
    req.on("end", () => {
      const [status, body] = handler(req.url ?? "", req.headers["x-idfon-channel-secret"] as string, JSON.parse(data || "{}"));
      res.writeHead(status, { "content-type": "application/json" });
      res.end(typeof body === "string" ? body : JSON.stringify(body));
    });
  });
  await new Promise<void>((resolve) => server.listen(0, "127.0.0.1", resolve));
  const { port } = server.address() as { port: number };
  try {
    await run(`http://127.0.0.1:${port}`);
  } finally {
    server.close();
  }
}

test("posts JSON with the secret and returns typed results", async () => {
  await withServer(
    (path, secret, body) => {
      assert.equal(path, "/blob/put");
      assert.equal(secret, "s");
      assert.deepEqual(body, { bytes_base64: "aGk=" });
      return [200, { ticket: "blobx", size_bytes: 2 }];
    },
    async (url) => {
      const result = await createBridge({ url, secret: "s" }).putBlob("aGk=");
      assert.deepEqual(result, { ticket: "blobx", size_bytes: 2 });
    },
  );
});

test("a non-2xx response throws BridgeError with path, status and body", async () => {
  await withServer(
    () => [403, "denied: no grant"],
    async (url) => {
      await assert.rejects(createBridge({ url, secret: "s" }).stopLive("x"), (error: unknown) => {
        assert.ok(error instanceof BridgeError);
        assert.equal(error.path, "/live/stop");
        assert.equal(error.status, 403);
        assert.match(error.message, /denied: no grant/);
        return true;
      });
    },
  );
});

test("retry tries three times", async () => {
  let calls = 0;
  await withServer(
    () => (++calls < 3 ? [500, "busy"] : [200, {}]),
    async (url) => {
      await createBridge({ url, secret: "s" }).retry("/reply", {});
      assert.equal(calls, 3);
    },
  );
});

test("missing configuration is a clear error", async () => {
  const saved = { ...process.env };
  for (const name of ["IDFON_BRIDGE_URL", "EVE_IDFON_BRIDGE_URL"]) delete process.env[name];
  try {
    await assert.rejects(createBridge({ secret: "s" }).post("/x"), /IDFON_BRIDGE_URL/);
  } finally {
    Object.assign(process.env, saved);
  }
});

test("envelopes are byte-for-byte the formats the apps parse", () => {
  assert.equal(envelopes.data("blobx", 22), "IDFON-DATA/1\nticket=blobx\nsize=22");
  assert.equal(envelopes.live("audio", "ilv1:t"), "IDFON-LIVE/1\naction=start\nkind=audio\nticket=ilv1:t");
  assert.equal(
    envelopes.card("p1", { id: "e" }, { t: 1 }),
    'IDFON-CARD/1\nsubject=p1\ncontact={"id":"e"}\nticket={"t":1}',
  );
  assert.equal(
    envelopes.cardRequest("p1", "m", "r1"),
    "IDFON-CARD-REQUEST/1\npeer=p1\nmodel=m\nreply_to=r1",
  );
  assert.equal(envelopes.screenshot(null), 'IDFON-SCREENSHOT/1\n{"reason":null}');
  assert.equal(
    envelopes.speak({ text: "hi", voice: null, language: null, rate: null, priority: "normal", correlation_id: null }),
    'IDFON-SPEAK/1\n{"text":"hi","voice":null,"language":null,"rate":null,"priority":"normal","correlation_id":null}',
  );
  assert.equal(envelopeField(envelopes.card("p1", 1, 2), "contact"), "1");
});
