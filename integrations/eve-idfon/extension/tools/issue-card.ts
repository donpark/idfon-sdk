import { defineTool } from "eve/tools";
import { z } from "zod";

import { bridge } from "../bridge";
import { envelopes } from "../envelopes";

// The holder signs its own card; this tool is how the agent asks it to. Each
// call mints a fresh, subject-bound capability ticket plus the holder's
// endpoint address, so the intro flow never reuses a card.
export default defineTool({
  description:
    "Issue a fresh contact card (endpoint address + capability ticket) for a peer id. " +
    "Use the requester's peer id so the ticket is bound to them.",
  inputSchema: z.object({
    subject: z.string().min(1).describe("Peer id the card is bound to (the requester)."),
    capabilities: z
      .array(z.string())
      .optional()
      .describe("Extra grants beyond message.receive (e.g. agent.receive)."),
  }),
  async execute(input) {
    const result = await bridge.issueCard(input.subject, input.capabilities ?? []);
    return {
      endpoint_addr: result.endpoint_addr,
      ticket: result.ticket,
      envelope: envelopes.card(input.subject, result.endpoint_addr, result.ticket),
    };
  },
});
