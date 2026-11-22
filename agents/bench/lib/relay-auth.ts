// Relay-auth admission for bench daemons (idfon-edge docs/relay-auth.md).
//
// A relay in webhook mode admits only endpoint ids with a live entry, and the
// bench mints fresh endpoint keys per pair, so each pair must be admitted
// before it can use the relay. Given an owner identity key (one of the relay's
// IDFON_RELAY_ISSUERS), we enroll daemon A as an owner endpoint (v2 owner
// proof), admit daemon B with the owner token on a short lease, and revoke
// both afterwards. Existing entries (the apps, the edge) are never touched.
//
//   IDFON_RELAY_OWNER_KEY  path to the owner identity key (64-hex seed, e.g.
//                          the mac app's daemon/identity-default.key)
//   IDFON_RELAY_AUTH_URL   relay-auth origin; default https://<relay host>:9443
import crypto from "node:crypto";
import { readFileSync } from "node:fs";

export interface RelayAuth {
  base: string;
  keyFile: string;
}

/** Admission config for a relay URL, or null when no owner key is set. */
export function relayAuthFor(relayUrl: string, env: NodeJS.ProcessEnv = process.env): RelayAuth | null {
  const keyFile = env.IDFON_RELAY_OWNER_KEY;
  if (!keyFile) return null;
  const base = env.IDFON_RELAY_AUTH_URL ?? `https://${new URL(relayUrl).hostname}:9443`;
  return { base: new URL(base).origin, keyFile };
}

function ownerKey(file: string): { privateKey: crypto.KeyObject; pubHex: string } {
  const seedHex = readFileSync(file, "utf8").trim();
  if (!/^[0-9a-f]{64}$/i.test(seedHex)) throw new Error(`${file}: expected a 64-hex seed`);
  const privateKey = crypto.createPrivateKey({
    key: Buffer.concat([Buffer.from("302e020100300506032b657004220420", "hex"), Buffer.from(seedHex, "hex")]),
    format: "der",
    type: "pkcs8",
  });
  const pubHex = crypto.createPublicKey(privateKey).export({ format: "der", type: "spki" }).subarray(-32).toString("hex");
  return { privateKey, pubHex };
}

async function call(base: string, route: string, opts: { body?: unknown; token?: string; method?: string } = {}) {
  const res = await fetch(`${base}${route}`, {
    method: opts.method ?? (opts.body === undefined ? "GET" : "POST"),
    headers: {
      "content-type": "application/json",
      ...(opts.token ? { authorization: `Bearer ${opts.token}` } : {}),
    },
    body: opts.body === undefined ? undefined : JSON.stringify(opts.body),
  });
  const text = await res.text();
  if (res.status !== 200) throw new Error(`relay-auth ${route} -> ${res.status} ${text}`);
  return JSON.parse(text);
}

/** Admit a daemon pair; returns the revoke function (B first: A's entry carries the token). */
export async function admitPair(auth: RelayAuth, a: string, b: string, label: string): Promise<() => Promise<void>> {
  const key = ownerKey(auth.keyFile);
  const { nonce } = await call(auth.base, "/owner/challenge", { body: { identity: key.pubHex, endpoint_id: a } });
  const proof = `idfon-relay-owner/2|${key.pubHex}|${a}|${nonce}|${auth.base}`;
  const signature = crypto.sign(null, Buffer.from(proof, "utf8"), key.privateKey).toString("hex");
  const { token } = await call(auth.base, "/owner/enroll", {
    body: { identity: key.pubHex, endpoint_id: a, nonce, proof: 2, signature },
  });
  const revoke = async () => {
    for (const endpoint of [b, a]) {
      await call(auth.base, `/owner/entries/${endpoint}`, { token, method: "DELETE" }).catch(() => {});
    }
  };
  try {
    // 0.05 day = 72 min: outlives any bench run, expires on its own if the revoke is lost.
    await call(auth.base, "/owner/entries", { token, body: { endpoint_id: b, label, lease_days: 0.05 } });
  } catch (error) {
    await revoke();
    throw error;
  }
  return revoke;
}
