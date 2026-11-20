import { defineTool } from "eve/tools";
import { z } from "zod";

import { bridge } from "../bridge";
import { envelopes } from "../envelopes";

export default defineTool({
  description: "Publish a local audio file as an idfon live stream ticket.",
  inputSchema: z.object({
    filePath: z.string().min(1),
    name: z.string().min(1).default("eve-live"),
    loop: z.boolean().default(false),
    relay: z.boolean().default(true),
    video: z.boolean().default(false),
    quality: z.enum(["all", "180p", "360p", "720p"]).default("all"),
  }),
  async execute(input, ctx) {
    const capabilities = ctx.session.auth.current?.attributes?.capabilities;
    const grant = input.video ? "live.video.publish" : "live.audio.publish";
    if (!capabilities?.includes(grant)) {
      throw new Error(`idfon grant ${grant} is required`);
    }
    const result = await bridge.publishLive({
      path: input.filePath,
      name: input.name,
      loop_playback: input.loop,
      relay: input.relay,
      video: input.video,
      quality: input.quality,
    });
    return { ...result, envelope: envelopes.live(input.video ? "video" : "audio", result.ticket) };
  },
});
