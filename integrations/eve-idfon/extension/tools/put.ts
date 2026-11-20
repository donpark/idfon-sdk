import { defineTool } from "eve/tools";
import { z } from "zod";

import { bridge } from "../bridge";
import { envelopes } from "../envelopes";
import { emitEnvelope } from "../outbox";

// Store bytes and hand them to the caller. The `IDFON-DATA/1` envelope is
// attached to this turn's reply automatically (the outbox); the model never
// writes it. To give the bytes to a different peer, pass the returned ticket
// to `send` as `attach: [{ kind: "data", ticket, size_bytes }]`.
export default defineTool({
  description:
    "Store base64 data and attach it to your reply as a file the caller can save. " +
    "Delivery is automatic: do not write the ticket or any IDFON-* text in your reply. " +
    "To send it to another peer instead, call send with attach: [{kind: 'data', ticket, size_bytes}].",
  inputSchema: z.object({
    bytesBase64: z.string().min(1),
  }),
  async execute(input, ctx) {
    const result = await bridge.putBlob(input.bytesBase64);
    emitEnvelope(ctx.session.id, envelopes.data(result.ticket, result.size_bytes));
    return { ...result, delivered: "attached to your reply" };
  },
});
