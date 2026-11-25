import { defineAgent } from "eve";
import { mockModel } from "eve/evals";

import { respond } from "./lib/respond";

// feedback: the built-in contact that collects feedback from inside the app.
// Deterministic on purpose: no model, no API key, the same answer every time.
// Any message gets the form (a UI SDK page sent as an html artifact,
// tools/feedback-form.ts); a submission from that form is saved and thanked
// (tools/feedback-save.ts). See README.md.
export default defineAgent({
  model: mockModel(respond),
  modelContextWindowTokens: 32_768,
});
