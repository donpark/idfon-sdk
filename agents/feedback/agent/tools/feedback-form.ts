import { randomUUID } from "node:crypto";
import { readFile } from "node:fs/promises";

import { defineTool } from "eve/tools";
import { z } from "zod";

import { bridge } from "eve-idfon/bridge";
import { envelopes } from "eve-idfon/envelopes";
import { emitEnvelope } from "eve-idfon/outbox";

// Sends the feedback form: page/dist/feedback.html (built by page/build.mjs,
// one self-contained file) as an html artifact. The app opens it in its
// sandboxed web view; `metadata.form` tells the shell what the page is.

const PAGE_PATH = process.env.IDFON_FEEDBACK_PAGE ?? "page/dist/feedback.html";

export default defineTool({
  description: "Send the caller the feedback form.",
  inputSchema: z.object({}),
  async execute(_input, ctx) {
    const bytes = await readFile(PAGE_PATH);
    const blob = await bridge.putBlob(bytes.toString("base64"));
    const artifactId = randomUUID();
    emitEnvelope(
      ctx.session.id,
      envelopes.artifact({
        artifact_id: artifactId,
        kind: "html",
        mime: "text/html",
        title: "Send feedback",
        size_bytes: blob.size_bytes,
        blob_ticket: blob.ticket,
        created_at: new Date().toISOString(),
        metadata: { form: "feedback/1" },
      }),
    );
    return { artifact_id: artifactId, size_bytes: blob.size_bytes };
  },
});
