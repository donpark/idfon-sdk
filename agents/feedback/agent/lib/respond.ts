// The feedback agent's whole policy, as the mock model's turn function.
// One tool call per turn, then a short text once its result is in:
//
//   a form submission ("Feedback · …")  → feedback-save → thanks
//   anything else                       → feedback-form → a pointer to the form

import { HEADER } from "../../page/message";

export interface Turn {
  lastUserMessage: string | null;
  /** The whole session so far; eve's `toolResults` spans every turn. */
  messages: { role: string; text: string }[];
}

export type Reply = string | { toolCalls: { name: string; input: Record<string, unknown> }[] };

/** True once this turn's tool has answered: a tool message after the last user message. */
function toolAnswered(messages: Turn["messages"]): boolean {
  const lastUser = messages.findLastIndex((message) => message.role === "user");
  return messages.slice(lastUser + 1).some((message) => message.role === "tool");
}

export function respond({ lastUserMessage, messages }: Turn): Reply {
  const submission = (lastUserMessage ?? "").startsWith(HEADER);
  if (!toolAnswered(messages)) {
    return submission
      ? { toolCalls: [{ name: "feedback-save", input: { message: lastUserMessage } }] }
      : { toolCalls: [{ name: "feedback-form", input: {} }] };
  }
  return submission
    ? "Thanks, we got it. Send more any time."
    : "Tap the form to tell us about a bug or an idea.";
}
