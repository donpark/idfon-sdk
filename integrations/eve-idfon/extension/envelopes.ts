// Every `IDFON-*/1` envelope this package writes, built in one place.
//
// Two body encodings exist on the wire today: a JSON object (SPEAK, ARTIFACT,
// POINT, SHOW, SCREENSHOT, RENDER) and `key=value` lines (DATA, LIVE, CARD,
// CARD-REQUEST, SESSION). The apps parse both, so the encodings here are
// byte-for-byte what the tools wrote before; changing one is a protocol
// change (docs/reply-blocks.md, crates/idfon-protocol/src/blocks.rs).

/** `HEADER\n{json}`. */
export function jsonEnvelope(kind: string, body: unknown): string {
  return `IDFON-${kind}/1\n${JSON.stringify(body)}`;
}

/** `HEADER\nkey=value\n…`, skipping undefined values. */
export function kvEnvelope(kind: string, fields: Record<string, string | number | undefined>): string {
  const lines = Object.entries(fields)
    .filter(([, value]) => value !== undefined)
    .map(([key, value]) => `${key}=${value}`);
  return [`IDFON-${kind}/1`, ...lines].join("\n");
}

export interface SpeakBody {
  text: string;
  voice: string | null;
  language: string | null;
  rate: number | null;
  priority: string;
  correlation_id: string | null;
}

export const envelopes = {
  speak: (body: SpeakBody) => jsonEnvelope("SPEAK", body),
  artifact: (artifact: Record<string, unknown>) => jsonEnvelope("ARTIFACT", artifact),
  point: (body: Record<string, unknown>) => jsonEnvelope("POINT", body),
  show: (body: Record<string, unknown>) => jsonEnvelope("SHOW", body),
  screenshot: (reason: string | null) => jsonEnvelope("SCREENSHOT", { reason }),
  render: (body: Record<string, unknown>) => jsonEnvelope("RENDER", body),
  /** A blob the receiver can fetch (`idfon recv` saves it). */
  data: (ticket: string, sizeBytes: number) => kvEnvelope("DATA", { ticket, size: sizeBytes }),
  live: (kind: "audio" | "video", ticket: string) =>
    kvEnvelope("LIVE", { action: "start", kind, ticket }),
  /** A contact card: values are JSON-encoded, as the apps expect. */
  card: (subject: string, contact: unknown, ticket: unknown) =>
    kvEnvelope("CARD", { subject, contact: JSON.stringify(contact), ticket: JSON.stringify(ticket) }),
  cardRequest: (peer: string, model: string, replyTo: string) =>
    kvEnvelope("CARD-REQUEST", { peer, model, reply_to: replyTo }),
  /** An introduction: `contact`/`ticket` are already JSON text from a card. */
  invite: (fields: { name: string; model: string; expires_at: number; contact: string; ticket: string }) =>
    kvEnvelope("INVITE", fields),
};

/** The `key=value` field of a k=v envelope body, or undefined. */
export function envelopeField(text: string, key: string): string | undefined {
  const match = new RegExp(`(?:^|\\n)${key}=([^\\n]*)`).exec(text);
  return match?.[1];
}
