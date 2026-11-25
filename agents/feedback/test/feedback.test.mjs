// node --test test/  (bundles the TypeScript under test with esbuild first)
import assert from "node:assert/strict";
import { mkdir, readFile, stat, writeFile } from "node:fs/promises";
import { dirname, join } from "node:path";
import { test } from "node:test";
import { fileURLToPath, pathToFileURL } from "node:url";
import { execFileSync } from "node:child_process";

import { build } from "esbuild";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");

async function load(entry) {
  const out = join(root, "test/.build", entry.replaceAll("/", "_") + ".mjs");
  await mkdir(dirname(out), { recursive: true });
  const { outputFiles } = await build({
    entryPoints: [join(root, entry)], bundle: true, format: "esm", platform: "node",
    packages: "external", write: false,
  });
  await writeFile(out, outputFiles[0].text);
  return import(pathToFileURL(out).href);
}

const { formatFeedback, parseFeedback } = await load("page/message.ts");
const { respond } = await load("agent/lib/respond.ts");
const { feedbackRecord } = await load("agent/tools/feedback-save.ts");

test("a submission round-trips through the message format", () => {
  const text = formatFeedback({ category: "idea", rating: 4, text: "Nearby should explain itself." });
  assert.equal(text, "Feedback · Idea · 4/5\nNearby should explain itself.");
  assert.deepEqual(parseFeedback(text), { category: "idea", rating: 4, text: "Nearby should explain itself." });
  assert.deepEqual(parseFeedback(formatFeedback({ category: "bug", text: "x\ny" })), { category: "bug", rating: undefined, text: "x\ny" });
  assert.equal(parseFeedback("hello"), undefined);
  assert.equal(parseFeedback("Feedback · Nonsense\nx"), undefined);
});

test("any message gets the form; a submission is saved", () => {
  const hi = [{ role: "user", text: "hi" }];
  assert.deepEqual(respond({ lastUserMessage: "hi", messages: hi }), { toolCalls: [{ name: "feedback-form", input: {} }] });
  assert.match(respond({ lastUserMessage: "hi", messages: [...hi, { role: "assistant", text: "" }, { role: "tool", text: "{}" }] }), /form/);

  const submission = formatFeedback({ category: "bug", text: "crash" });
  // Earlier turns' tool results must not count for this turn (eve's toolResults spans the session).
  const history = [...hi, { role: "tool", text: "{}" }, { role: "assistant", text: "Tap the form" }, { role: "user", text: submission }];
  assert.deepEqual(respond({ lastUserMessage: submission, messages: history }), { toolCalls: [{ name: "feedback-save", input: { message: submission } }] });
  assert.match(respond({ lastUserMessage: submission, messages: [...history, { role: "tool", text: "{}" }] }), /Thanks/);

  const again = [...history, { role: "tool", text: "{}" }, { role: "assistant", text: "Thanks" }, { role: "user", text: "one more" }];
  assert.deepEqual(respond({ lastUserMessage: "one more", messages: again }), { toolCalls: [{ name: "feedback-form", input: {} }] });
});

test("every tool the responder calls exists (eve names a tool after its file)", async () => {
  const { readdir } = await import("node:fs/promises");
  const files = new Set((await readdir(join(root, "agent/tools"))).map((f) => f.replace(/\.ts$/, "")));
  const calls = [
    respond({ lastUserMessage: "hi", messages: [{ role: "user", text: "hi" }] }),
    respond({ lastUserMessage: formatFeedback({ category: "bug", text: "x" }), messages: [{ role: "user", text: "x" }] }),
  ];
  for (const call of calls) for (const { name } of call.toolCalls) assert.ok(files.has(name), `agent/tools/${name}.ts`);
});

test("a record keeps the sender, the parsed fields and the raw message", () => {
  const message = formatFeedback({ category: "bug", rating: 2, text: "crash" });
  const record = feedbackRecord(message, "peer-1", new Date("2026-11-24T00:00:00Z"));
  assert.deepEqual(record, {
    received_at: "2026-11-24T00:00:00.000Z", peer_id: "peer-1",
    category: "bug", rating: 2, text: "crash", message,
  });
  assert.equal(feedbackRecord("free text", undefined).category, null);
});

test("the built page is one self-contained file", async () => {
  execFileSync(process.execPath, [join(root, "page/build.mjs")], { stdio: "ignore" });
  const path = join(root, "page/dist/feedback.html");
  const html = await readFile(path, "utf8");
  assert.ok((await stat(path)).size < 300 * 1024, "page stays under 300 KB");
  assert.doesNotMatch(html, /<script[^>]+src=|<link[^>]+href=/i, "no external scripts or stylesheets");
  assert.ok(!html.includes("/*PAGE_JS*/") && !html.includes("/*UI_SDK_CSS*/"), "placeholders replaced");
  assert.match(html, /<ui-segmented-control/);
});
