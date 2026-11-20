import { defineTool } from "eve/tools";
import { z } from "zod";

import { bridge } from "../bridge";

export default defineTool({
  description: "Send a text message to another authenticated idfon peer.",
  inputSchema: z.object({
    peerId: z.string().min(1),
    endpointId: z.string().min(1),
    // The peer's capability ticket: an rcan token string (older JSON objects
    // are still accepted).
    capabilityTicket: z.union([z.string().min(1), z.record(z.string(), z.unknown())]),
    text: z.string().min(1),
    conversation: z.string().optional(),
    a2aDepth: z.number().int().min(0).max(1).default(0),
  }),
  async execute(input) {
    return bridge.send({
      peer_id: input.peerId,
      endpoint_id: input.endpointId,
      capability_ticket: input.capabilityTicket,
      text: input.text,
      conversation: input.conversation,
      a2a_depth: input.a2aDepth,
    });
  },
});
