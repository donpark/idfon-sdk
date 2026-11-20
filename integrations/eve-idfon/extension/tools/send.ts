import { defineTool } from "eve/tools";
import { z } from "zod";

import { bridge } from "../bridge";
import { envelopes } from "../envelopes";

// Send a message to another peer (not the caller: replies to the caller go
// through your normal reply). Things other tools produced (a stored file, a
// live stream, a contact card) travel as structured `attach` items; the tool
// writes their IDFON-*/1 envelopes, so the model never does.
const attachment = z.discriminatedUnion("kind", [
  z.object({
    kind: z.literal("data"),
    ticket: z.string().min(1).describe("Blob ticket from put / add_artifact."),
    size_bytes: z.number().int().nonnegative(),
  }),
  z.object({
    kind: z.literal("live"),
    ticket: z.string().min(1).describe("Live ticket from publish_live."),
    media: z.enum(["audio", "video"]).default("audio"),
  }),
  z.object({
    kind: z.literal("card"),
    subject: z.string().min(1),
    contact: z.unknown().describe("endpoint_addr from issue_card."),
    ticket: z.unknown().describe("ticket from issue_card."),
  }),
]);

export default defineTool({
  description:
    "Send a message to another authenticated idfon peer (not the caller). To pass along a " +
    "file, live stream or contact card another tool returned, add it to `attach`; never paste " +
    "tickets or IDFON-* text into `text`.",
  inputSchema: z.object({
    peerId: z.string().min(1),
    endpointId: z.string().min(1),
    // The peer's capability ticket: an rcan token string (older JSON objects
    // are still accepted).
    capabilityTicket: z.union([z.string().min(1), z.record(z.string(), z.unknown())]),
    text: z.string().min(1),
    attach: z.array(attachment).default([]),
    conversation: z.string().optional(),
    a2aDepth: z.number().int().min(0).max(1).default(0),
  }),
  async execute(input) {
    const attached = input.attach.map((item) => {
      switch (item.kind) {
        case "data":
          return envelopes.data(item.ticket, item.size_bytes);
        case "live":
          return envelopes.live(item.media, item.ticket);
        case "card":
          return envelopes.card(item.subject, item.contact, item.ticket);
      }
    });
    return bridge.send({
      peer_id: input.peerId,
      endpoint_id: input.endpointId,
      capability_ticket: input.capabilityTicket,
      text: [input.text, ...attached].join("\n"),
      conversation: input.conversation,
      a2a_depth: input.a2aDepth,
    });
  },
});
