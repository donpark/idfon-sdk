// The one client for the local idfon bridge (the managed holder's HTTP side).
//
// Every tool, instruction, channel route and agent goes through here instead
// of hand-writing `fetch` + the secret header, so the URL/secret lookup, the
// error shape and the endpoint names live in one place.
//
// Configuration: the eve-idfon extension config (`bridgeUrl`, `secret`), else
// `IDFON_BRIDGE_URL` / `IDFON_BRIDGE_SECRET`. The older names
// `EVE_IDFON_BRIDGE_URL`, `EVE_IDFON_SECRET` and `IDFON_CHANNEL_SECRET` are
// still read.

import extension from "./extension";

export interface BridgeOptions {
  url?: string;
  secret?: string;
}

/** A non-2xx bridge response: the path, the status and the bridge's own text. */
export class BridgeError extends Error {
  readonly path: string;
  readonly status: number;
  readonly body: string;

  constructor(path: string, status: number, body: string) {
    super(`idfon bridge ${path} returned HTTP ${status}${body ? `: ${body}` : ""}`);
    this.name = "BridgeError";
    this.path = path;
    this.status = status;
    this.body = body;
  }
}

function env(...names: string[]): string | undefined {
  for (const name of names) {
    const value = process.env[name];
    if (value && value.trim()) return value.trim();
  }
  return undefined;
}

function config(): { bridgeUrl?: string; secret?: string } {
  try {
    return (extension as { config?: { bridgeUrl?: string; secret?: string } }).config ?? {};
  } catch {
    // Outside an Eve runtime the extension config is not available.
    return {};
  }
}

export interface BlobPut {
  ticket: string;
  size_bytes: number;
}

export interface LivePublishInput {
  path: string;
  name: string;
  loop_playback: boolean;
  relay: boolean;
  video: boolean;
  quality: string;
}

export interface FilePublishInput {
  bytes_base64: string;
  name?: string;
  expires_in?: number;
  permanent?: boolean;
}

export interface FilePublish {
  url: string;
  cdn_url?: string;
  name_url?: string;
  sha256: string;
  size: number;
  existed: boolean;
  expires_at?: number;
}

export interface SendInput {
  peer_id: string;
  endpoint_id: string;
  capability_ticket?: unknown;
  text: string;
  conversation?: string;
  a2a_depth?: number;
  await_reply?: boolean;
  reply_to?: string;
  reply_timeout_ms?: number;
}

export interface Card {
  ticket: unknown;
  endpoint_addr: unknown;
}

/** A bridge client. `createBridge()` with no options uses the extension config/env. */
export function createBridge(options: BridgeOptions = {}) {
  const url = () =>
    options.url ?? config().bridgeUrl ?? env("IDFON_BRIDGE_URL", "EVE_IDFON_BRIDGE_URL");
  const secret = () =>
    options.secret ??
    config().secret ??
    env("IDFON_BRIDGE_SECRET", "EVE_IDFON_SECRET", "IDFON_CHANNEL_SECRET");

  /** POST JSON to a bridge path; throws `BridgeError` on a non-2xx status. */
  async function post(path: string, body: unknown = {}): Promise<Response> {
    const base = url();
    const key = secret();
    if (!base) throw new Error("idfon bridge URL is not configured (IDFON_BRIDGE_URL)");
    if (!key) throw new Error("idfon bridge secret is not configured (IDFON_BRIDGE_SECRET)");
    const response = await fetch(`${base}${path}`, {
      method: "POST",
      headers: { "content-type": "application/json", "x-idfon-channel-secret": key },
      body: JSON.stringify(body),
    });
    if (!response.ok) {
      throw new BridgeError(path, response.status, (await response.text()).trim().slice(0, 500));
    }
    return response;
  }

  async function json<T>(path: string, body: unknown = {}): Promise<T> {
    return (await post(path, body)).json() as Promise<T>;
  }

  /** `post` with three attempts and 100/200 ms backoff. */
  async function retry(path: string, body: unknown = {}): Promise<Response> {
    let lastError: unknown;
    for (let attempt = 0; attempt < 3; attempt += 1) {
      try {
        return await post(path, body);
      } catch (error) {
        lastError = error;
        if (attempt < 2) await new Promise((resolve) => setTimeout(resolve, 100 * 2 ** attempt));
      }
    }
    throw lastError;
  }

  return {
    post,
    json,
    retry,
    /** Store bytes in the holder's blob store; the ticket downloads them. */
    putBlob: (bytesBase64: string) => json<BlobPut>("/blob/put", { bytes_base64: bytesBase64 }),
    /** Fetch blob bytes by ticket (`/blob`). */
    getBlob: (body: Record<string, unknown>) => post("/blob", body),
    /** Pin bytes on the edge's durable file store. */
    publishFile: (input: FilePublishInput) => json<FilePublish>("/files/publish", input),
    publishLive: (input: LivePublishInput) => json<{ id: string; ticket: string }>("/live/publish", input),
    stopLive: (id: string) => json<unknown>("/live/stop", { id }),
    /** Send a message to a peer (optionally waiting for the reply). */
    send: <T = unknown>(input: SendInput) => json<T>("/send", input),
    /** A fresh contact card bound to `subject`. */
    issueCard: (subject: string, capabilities: string[] = []) =>
      json<Card>("/card", { subject, capabilities }),
  };
}

export type Bridge = ReturnType<typeof createBridge>;

/** The default client: extension config, else env. */
export const bridge = createBridge();
