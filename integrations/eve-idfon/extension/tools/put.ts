import { defineTool } from "eve/tools";
import { z } from "zod";

import { bridge } from "../bridge";
import { envelopes } from "../envelopes";

export default defineTool({
  description: "Store base64 data in the authenticated idfon blob store for peer delivery.",
  inputSchema: z.object({
    bytesBase64: z.string().min(1),
  }),
  async execute(input) {
    const result = await bridge.putBlob(input.bytesBase64);
    return { ...result, envelope: envelopes.data(result.ticket, result.size_bytes) };
  },
});
