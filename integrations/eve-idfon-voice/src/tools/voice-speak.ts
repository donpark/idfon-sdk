import { defineTool } from "eve/tools";
import { emitEnvelope } from "eve-idfon/outbox";
import { envelopes } from "eve-idfon/envelopes";
import { z } from "zod";

import { RATE, wavWrap } from "../audio";
import { putAudio } from "../bridge";
import { createVoiceProvider, engineConfig } from "../providers";

// Self-answering, outbound half: speak the agent's reply back to the caller as
// an idfon audio message. Pair with `voice_transcribe` on the inbound memo.
export function voiceSpeakTool() {
  return defineTool({
    description:
      "Speak text back to the caller as an idfon audio message. The audio is attached " +
      "to your reply automatically: do not write the ticket or any IDFON-* text.",
    inputSchema: z.object({ text: z.string().min(1).describe("Text to speak") }),
    async execute({ text }, ctx) {
      const engine = engineConfig();
      const tts = await createVoiceProvider(engine.tts, "tts");
      if (!tts.synthesize) throw new Error(`voice provider '${tts.name}' has no TTS`);
      const wav = wavWrap(await tts.synthesize(text));
      const blob = await putAudio(wav);
      emitEnvelope(ctx.session.id, envelopes.data(blob.ticket, blob.size_bytes));
      return {
        ticket: blob.ticket,
        size_bytes: blob.size_bytes,
        delivered: "attached to your reply",
        seconds: Math.round((wav.length / (RATE * 2)) * 10) / 10,
      };
    },
  });
}
