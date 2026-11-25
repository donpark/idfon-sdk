# feedback

The contact that collects feedback from inside the app. It is deterministic:
a mock model runs `agent/lib/respond.ts`, so it needs no model and no API key.

| The user sends | The agent |
| --- | --- |
| anything | sends the form: `page/dist/feedback.html` as an html artifact (`metadata.form = "feedback/1"`) |
| a form submission (`Feedback · …`) | saves it, then thanks them |

```sh
pnpm agent start feedback     # builds the page, then serves (serve.env: plain runner, mock model)
pnpm --filter idfon-eve-feedback test
```

## The form

`page/` is a UI SDK page (`@ui-sdk/web` custom elements: segmented control,
rating, textarea, button). `page/build.mjs` bundles it with UI SDK's
stylesheet into one self-contained HTML file, because the app's artifact
sandbox blocks every network load. The serve script runs the build before
`eve build` (any agent with a `page/build.mjs`).

The app shows the page in its sandboxed web view. A page from a built-in
contact gets `window.idfon`:

- `reply(text)` sends `text` to this contact as a message from the user. It
  goes through the chat's normal send path, so the destination is fixed by
  the app, not the page.
- `ready()` marks when the form can take input (the app logs the time from tap).

Elsewhere (a browser, an untrusted contact) the form renders but cannot send.

## Messages

A submission is a plain message the user can read in the thread
(`page/message.ts`):

```
Feedback · Bug · 4/5
The call dropped when I switched to AirPods.
```

## Where feedback goes

Each submission is one JSON line in `IDFON_FEEDBACK_FILE` (default
`~/.idfon/feedback/submissions.jsonl`, mode 0600): `received_at`, `peer_id`,
`category`, `rating`, `text` and the raw `message`. A later step may forward
them elsewhere (email, an issue tracker).
