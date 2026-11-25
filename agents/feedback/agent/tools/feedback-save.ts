import { appendFile, mkdir } from "node:fs/promises";
import { homedir } from "node:os";
import { dirname, join } from "node:path";

import { defineTool } from "eve/tools";
import { z } from "zod";

import { parseFeedback } from "../../page/message";

// Saves one submission as a JSON line: who sent it, when, and the parsed
// fields next to the raw message. The file is the inbox for now
// (README.md, "Where feedback goes").

export const FEEDBACK_FILE =
  process.env.IDFON_FEEDBACK_FILE ?? join(homedir(), ".idfon", "feedback", "submissions.jsonl");

export function feedbackRecord(message: string, peerId: string | undefined, at = new Date()) {
  return {
    received_at: at.toISOString(),
    peer_id: peerId ?? null,
    ...(parseFeedback(message) ?? { category: null, rating: undefined, text: message }),
    message,
  };
}

export default defineTool({
  description: "Save a feedback submission from the form.",
  inputSchema: z.object({ message: z.string().min(1).max(8000) }),
  async execute({ message }, ctx) {
    const peerId = ctx.session.auth.current?.attributes?.peer_id as string | undefined;
    const record = feedbackRecord(message, peerId);
    await mkdir(dirname(FEEDBACK_FILE), { recursive: true });
    await appendFile(FEEDBACK_FILE, `${JSON.stringify(record)}\n`, { mode: 0o600 });
    return { saved: true, category: record.category };
  },
});
