# Idfon docs index

**Read this first, then open only the files you need.** Do not bulk-read `docs/`
— most questions touch one or two entries below.

Status legend: **current** = describes the code as it is; **as-built** = a plan
kept because it records decisions; **design** = not all implemented.

## Orientation

| Doc | Covers | Read when |
| --- | --- | --- |
| `../AGENTS.md` | repository working rules and the iOS shipping checklist (gated IPA, credentials, build numbers, upload verification, TestFlight push smoke test) | shipping an iOS update or checking agent workflow rules |

## CLI and distribution

| Doc | Covers |
| --- | --- |
| `cli-architecture.md` | CLI shape + npm packaging; contact vCards |
| `cli-data.md` | `put`/`get`/`send-data`, live streaming |
| `npm-distribution.md` | npm packages and the publish gate |

## Media

| Doc | Covers |
| --- | --- |
| `live-radio.md` | the one-way talk-radio contact (`voice_route.mode = broadcast`, `stream` backend) for walk-around call-quality soaks: programming (slots + `schedule.json`), programme submissions from clips or pitches, filler segments from sourced facts (`brief`/`segment`), per-topic stations (`roster.json` + `radio-serve.sh`), the OPML menu contact (`opml` + `show=` selection, intro `kind`/`default`/`audio` layout, per-caller on-demand shows, continuation from the live-station directory search when a stream ends; `radio-stations-opml.mjs` curates and searches a liveness-checked live-station menu), the radio-world agent unit (`agents/radio-world` + `pnpm agent`: json-render station menu via `radio-menu`, programming via `radio-program`, directory search via `radio-stations`, hang-up = off), the public page (`files.idfon.net/<handle>/`), iPhone pairing, the JSONL record, soak modes, limits | planning a mobility soak, programming the radio contact, adding a topic station, changing the broadcast/stream/schedule path, or the radio-world agent |

## Artifacts and agents

| Doc | Covers |
| --- | --- |
| `idfon-artifacts.md` | artifact model, references, remote view, voice/spoken projection, accessibility |
| `idfon-artifacts-testing.md` | manual artifact/reference test runbook |
| `idfon-eve.md` | idfon as an Eve ingress channel (implemented) |
| `live-voice.md` | the voice agent built on the Eve channel; transport + front-end decision |
| `voice-agent.md` | voice agents that speak/listen for other agents (wrapped / injected); the governing architecture: one process by default (the iPhone shape), seams over processes (voice pipe / cascade / wrap), the voice-agent layer boundary (optimizations, cue caches and greeting policy belong to the agent, not the app shell), route-owned effective speech defaults in Contact Detail, bridge→library refactor (design) |
| `voice-agent-modes.md` | on-device vs passive-remote vs proxy, where the latency goes, and how per-contact speech defaults follow client/server route ownership |
| `voice-prompt-lab.md` | iterate voice-agent prompts headlessly (`scripts/voice-prompt-lab.sh`, scoring + variants) |
| `voice-multimodal.md` | the voice turn contract: text + `IDFON-*/1` envelopes + attachment; references, camera input (per-contact capability), client-facing tools (`speak`/`point`/`show`) via the outbox, output split, full-duplex gap (design) |
| `voice-side-channel.md` | shared voice service: requirements, STT/TTS candidates, plan (P0–P7 implemented; A1 client cascade integrated on iOS/macOS — iOS speech seams, call cues, greet-on-connect, `· spoken` chat turns); on-device engines are streaming-only since 2026-11-01 (Kokoro and Parakeet Redux removed): Phonon TTS default (the duplex loop's mouth), Nemotron streaming ASR (English bundle by default; `IDFON_ASR_MULTILINGUAL=1` selects the licence-matched 3.5 `latin/560ms` tier), Apple as the fallback; the text-only `llm` agent and per-run model selection for agency contacts |
| `voice-agent-prompting.md` | source notes on voice-agent prompt engineering: prompt structure, output rules, guardrails, tools, voice realism, testing; idfon use-case implications; frontier-lab voice prompting (research for a future skill) |

| `../corpus/voice-interactions/` | replayable voice-interaction corpus (Dessa demos + tyger demo): clean timed speaker-attributed transcripts + `-tygerdemo` replay scripts, no audio (recreated from timings — copyright-safe); the regression hedge for voice-agent changes, HF dataset planned |
| `voice-agent-harness.md` | research notes on the agent loop/harness: requirements (on-device + remote, subagents, decision models as tools), pi-agent-core shape, open-source candidates (yoagent, io-harness, fx, AFM harnesses); decision (adopt yoagent) + open items |
| `voice-agent-loop-contract.md` | the portable loop contract: prompt sections, tool schemas (canonical JSON), and the trace record shared by the on-device and remote tiers (design) |
| `agent-agency.md` | the `agency` go-between that introduces registered agents (A2A card relay, `IDFON-INVITE/1`; cards are target-signed, only the reply credential is agency-signed) |
| `session-context.md` | session as the medium for a shared virtual context edited by user, voice agent, and target agents; no chat-as-context, no recall (design) |

## Operations

| Doc | Covers |
| --- | --- |
| `benchmarks.md` | repeatable latency measurement as a pure-TS vertical: the bench Eve agent (`agents/bench`) + `scripts/relay-bench.sh` (relay A/B over the daemon stack, JSONL history) + `scripts/voice-latency-report.sh` (voice-turn stages) |