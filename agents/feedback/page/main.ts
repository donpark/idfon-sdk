// The feedback form: UI SDK web components, bundled into one HTML file
// (page/build.mjs) that the feedback agent sends as an html artifact.
//
// The app shows it in its sandboxed web view. A page from a built-in contact
// gets `window.idfon` (idfon-ios `SandboxedArtifactWebView`): `reply(text)`
// sends the text to that contact as a message from the user, and `ready()`
// marks the moment the form can take input (the WKWebView timing check).
// Anywhere else the form still renders, but cannot send.

import "@ui-sdk/web/button"
import "@ui-sdk/web/field"
import "@ui-sdk/web/rating"
import "@ui-sdk/web/segmented-control"
import "@ui-sdk/web/textarea"

import { formatFeedback, type Category } from "./message"

interface IdfonPage {
  reply(text: string): void
  ready(): void
}

declare global {
  interface Window {
    idfon?: IdfonPage
  }
}

const PROMPTS: Record<Category, string> = {
  bug: "What happened, and what did you expect?",
  idea: "What would make idfon better for you?",
  other: "Tell us anything.",
}

const form = document.querySelector<HTMLFormElement>("#feedback")!
const category = document.querySelector<HTMLElement & { value: string | null }>("#category")!
const rating = document.querySelector<HTMLElement & { value: number | null }>("#rating")!
const text = document.querySelector<HTMLTextAreaElement>("#text")!
const send = document.querySelector<HTMLElement>("#send")!
const sendButton = send.querySelector("button")!
const status = document.querySelector<HTMLElement>("#status")!

function currentCategory(): Category {
  const value = category.value
  return value === "idea" || value === "other" ? value : "bug"
}

function sync() {
  text.placeholder = PROMPTS[currentCategory()]
  sendButton.disabled = text.value.trim() === ""
}

category.addEventListener("change", sync)
text.addEventListener("input", sync)

send.addEventListener("action", () => {
  const body = text.value.trim()
  if (!body) return
  if (!window.idfon) {
    status.textContent = "This form can only send from the idfon app."
    return
  }
  window.idfon.reply(
    formatFeedback({ category: currentCategory(), rating: rating.value ?? undefined, text: body }),
  )
  form.hidden = true
  status.textContent = "Sent. Thank you!"
})

form.addEventListener("submit", (event) => event.preventDefault())
sync()

// Ready once the elements are upgraded and laid out: the next frame.
requestAnimationFrame(() => window.idfon?.ready())
