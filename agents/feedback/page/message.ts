// The wire format of one submission: a plain message the user can read in
// their thread, and the agent parses back (agent/tools/feedback-save.ts).
//
//   Feedback · Bug · 4/5
//   The call dropped when I switched to AirPods.

export type Category = "bug" | "idea" | "other"

export interface Feedback {
  category: Category
  rating?: number
  text: string
}

const LABELS: Record<Category, string> = { bug: "Bug", idea: "Idea", other: "Other" }

export const HEADER = "Feedback · "

export function formatFeedback({ category, rating, text }: Feedback): string {
  const parts = [LABELS[category]]
  if (rating !== undefined && rating > 0) parts.push(`${rating}/5`)
  return `${HEADER}${parts.join(" · ")}\n${text}`
}

/** The fields of a submission, or undefined when `message` is not one. */
export function parseFeedback(message: string): Feedback | undefined {
  if (!message.startsWith(HEADER)) return undefined
  const newline = message.indexOf("\n")
  const header = (newline === -1 ? message : message.slice(0, newline)).slice(HEADER.length)
  const text = newline === -1 ? "" : message.slice(newline + 1).trim()
  const [label, score] = header.split(" · ")
  const category = (Object.keys(LABELS) as Category[]).find((key) => LABELS[key] === label)
  if (!category) return undefined
  const rating = score?.match(/^(\d)\/5$/)?.[1]
  return { category, rating: rating ? Number(rating) : undefined, text }
}
