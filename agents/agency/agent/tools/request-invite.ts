import { defineTool } from "eve/tools";
import { z } from "zod";
import { createBridge } from "eve-idfon/bridge";
import { envelopeField, envelopes } from "eve-idfon/envelopes";
import { emitEnvelope } from "eve-idfon/outbox";

// The agency never mints the card itself. It asks the target agent (over
// idfon, A2A) to issue one, and relays the result back in this same turn so the
// caller — who only has the Agency contact — receives it.
const provisionerUrl = process.env.AGENCY_PROVISIONER_URL ?? "http://127.0.0.1:18777";
const provisionerSecret = process.env.AGENCY_PROVISIONER_SECRET ?? "m2-test-secret";
const bridgeUrl = process.env.IDFON_BRIDGE_URL ?? "";
const bridge = createBridge({
  url: bridgeUrl,
  secret: process.env.IDFON_BRIDGE_SECRET ?? "m2-test-secret",
});

const provisionerHeaders = {
  "content-type": "application/json",
  "x-idfon-provisioner-secret": provisionerSecret,
};

export default defineTool({
  description:
    "List the agent catalog, or arrange an introduction so the caller can reach an agent. " +
    "Omit `model` to list.",
  inputSchema: z.object({
    model: z
      .string()
      .min(1)
      .optional()
      .describe("Model or contact name the caller asked for; omit to list the catalog."),
  }),
  async execute(input, ctx) {
    if (!input.model) {
      const response = await fetch(`${provisionerUrl}/catalog`, { headers: provisionerHeaders });
      if (!response.ok) throw new Error(`agency catalog returned HTTP ${response.status}`);
      return response.json();
    }

    const caller = ctx.session.auth.current?.attributes?.peer_id;
    if (!caller) throw new Error("turn has no authenticated peer id");
    if (!bridgeUrl) throw new Error("agency bridge is not configured");

    // Pre-pairing material so *we* (the agency) can send to the target; the
    // target's own holder signs the card the caller actually receives.
    const targetResponse = await fetch(`${provisionerUrl}/target`, {
      method: "POST",
      headers: provisionerHeaders,
      body: JSON.stringify({ model: input.model }),
    });
    const target = (await targetResponse.json()) as {
      error?: string;
      name?: string;
      model?: string;
      endpoint_addr?: { id?: string; endpoint_id?: string };
      capability_ticket?: unknown;
    };
    if (!targetResponse.ok) {
      // Unknown or not-currently-running model: hand the model the catalog so
      // it can offer alternatives. Throwing here (the previous behavior) left
      // the turn with a tool error and no spoken reply.
      if (targetResponse.status === 404) {
        const catalogResponse = await fetch(`${provisionerUrl}/catalog`, { headers: provisionerHeaders });
        const catalog = catalogResponse.ok ? await catalogResponse.json() : { contacts: [] };
        return { unavailable: input.model, catalog: catalog.contacts ?? [] };
      }
      throw new Error(target.error ?? `agency target returned HTTP ${targetResponse.status}`);
    }
    const endpointId = target.endpoint_addr?.id ?? target.endpoint_addr?.endpoint_id;
    if (!endpointId) throw new Error("registered target has no endpoint id");

    const replyTo = `intro-${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 8)}`;
    const request = envelopes.cardRequest(String(caller), target.model ?? input.model, replyTo);

    const sent = await bridge.send<{ reply?: { text?: string } }>({
      peer_id: endpointId,
      endpoint_id: endpointId,
      capability_ticket: target.capability_ticket,
      text: request,
      a2a_depth: 1,
      await_reply: true,
      reply_to: replyTo,
      reply_timeout_ms: 60_000,
    });

    // The target's reply is its own text plus the card its issue-card tool
    // attached; find the envelope rather than assume it comes first.
    const replyText = sent.reply?.text ?? "";
    const at = replyText.indexOf("IDFON-CARD/1\n");
    if (at < 0) {
      throw new Error(`target did not return a card (got: ${replyText.slice(0, 60) || "empty"})`);
    }
    const card = replyText.slice(at);
    const contact = envelopeField(card, "contact");
    const ticket = envelopeField(card, "ticket");
    if (!contact || !ticket) throw new Error("card is missing contact or ticket");

    const expiresAt = Math.floor(Date.now() / 1000) + 3600;
    const envelope = envelopes.invite({
      name: target.name ?? input.model,
      model: target.model ?? input.model,
      expires_at: expiresAt,
      contact,
      ticket,
    });

    // Attached to this turn's reply: the app turns it into an "Add contact"
    // action. The model only says it has an invite.
    emitEnvelope(ctx.session.id, envelope);
    return {
      name: target.name ?? input.model,
      model: target.model ?? input.model,
      delivered: "invite attached to your reply",
    };
  },
});
