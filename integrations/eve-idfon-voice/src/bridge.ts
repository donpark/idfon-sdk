// Channel helpers for the TS voice tools: blob upload (reply audio) and an
// agent→agent send that waits for the reply (the wrapping relay).

import idfonExtension from "eve-idfon";
import { createBridge } from "eve-idfon/bridge";

export const bridgeUrl = (): string =>
  process.env.EVE_IDFON_BRIDGE_URL || idfonExtension.config?.bridgeUrl || "http://127.0.0.1:18766";

export const bridgeSecret = (): string =>
  process.env.EVE_IDFON_SECRET || idfonExtension.config?.secret || "m2-test-secret";

const client = () => createBridge({ url: bridgeUrl(), secret: bridgeSecret() });

/** Upload reply audio and return its blob ticket. */
export async function putAudio(wav: Buffer): Promise<{ ticket: string; size_bytes: number }> {
  return client().putBlob(wav.toString("base64"));
}

/** Push agent-produced PCM to the active call's return leg. */
export async function playAudio(peerId: string, pcm: Buffer): Promise<boolean> {
  const result = await client().json<{ accepted?: boolean }>("/live/audio", {
    peer_id: peerId,
    pcm_base64: pcm.toString("base64"),
  });
  return result.accepted ?? false;
}

/** Send text to a wrapped agent and wait for its reply (A2A relay). */
export async function sendAwait(
  peerId: string,
  endpointId: string,
  capabilityTicket: Record<string, unknown>,
  text: string,
  replyTo: string,
  timeoutMs = 60_000,
): Promise<string> {
  const body = await client().send<{ reply?: { text?: string } }>({
    peer_id: peerId,
    endpoint_id: endpointId,
    capability_ticket: capabilityTicket,
    text,
    a2a_depth: 1,
    await_reply: true,
    reply_to: replyTo,
    reply_timeout_ms: timeoutMs,
  });
  return body.reply?.text ?? "";
}
