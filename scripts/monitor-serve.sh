#!/bin/bash
# Serve the diag monitor (docs/diag.md) with the shared serve script; its
# defaults (mock model, plain eve-idfon holder) live in agents/monitor/serve.env.
# Reports land in ~/.idfon/<instance>/diag/<device>.jsonl.
export EVE_AGENT="${EVE_AGENT:-monitor}"
. "$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)/live-voice-serve.sh" "$@"
