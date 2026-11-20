import { defineTool } from "eve/tools";
import { z } from "zod";

import { bridge } from "../bridge";

export default defineTool({
  description: "Stop an idfon live stream publisher owned by this agent.",
  inputSchema: z.object({ id: z.string().min(1) }),
  async execute(input, ctx) {
    const capabilities = ctx.session.auth.current?.attributes?.capabilities;
    if (!capabilities?.includes("live.audio.publish")) {
      throw new Error("idfon grant live.audio.publish is required");
    }
    return bridge.stopLive(input.id);
  },
});
