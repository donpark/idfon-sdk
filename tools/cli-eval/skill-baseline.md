---
name: idfon-cli
description: Use the idfon CLI to manage identities and contacts, message peers, move files, and run live audio over idfon's peer-to-peer network.
---

# idfon CLI

`idfon` talks to a local daemon (`idfond`), which it starts automatically.
You act as one **identity**; other people and agents are **peers** (contacts).
Add `--json` to any command for the full response envelope.

## Concepts
- **Peer ref**: a peer's name, alias, id, or endpoint id.
- **Grants** gate your own side: `message.send` lets you send to a peer,
  `message.receive` lets you receive from them.
- **Tickets** are bearer strings: a blob ticket downloads bytes, an
  `iroh-live:` ticket plays a live stream, a capability ticket grants access.
- Sends are async: `send --text` returns an operation id.

## Commands
```
idfon status                              # daemon + active identity
idfon identity list|create NAME|use NAME|delete NAME
idfon peer list|show REF|add REF ...|update REF ...|remove REF
idfon contact import FILE.vcf             # add peers from a vCard
idfon contact export [--peer REF] [--out F]

idfon send PEER --text "hi"               # chat message
idfon send PEER --file FILE               # signaled file transfer
idfon recv --out FILE [--from PEER]       # wait for a signaled file
idfon events [--type message.received] [--follow]
idfon wait --type message.received [--after CURSOR] [--timeout-ms N]
idfon operation get|wait|cancel OP_ID

idfon put --file FILE                     # store; prints a blob ticket
idfon get TICKET [--out FILE]             # download a blob ticket
idfon publish FILE                        # durable public URL (edge)

idfon send --stream --file F.wav [--loop] # live broadcast; prints ticket
idfon send --stream --list | --stop ID    # list / stop broadcasts
idfon send PEER --stream --file F.wav     # 1:1 call (blocks until hangup)
idfon recv --stream --out F.wav           # answer a 1:1 call

idfon access check --subject ID --capability CAP
idfon access allow --subject ID --capability CAP
idfon access ticket --subject ID [--capability CAP] [--expires-at T]
idfon room create --name N [--member REF]... | list | send ROOM --text T | leave ROOM
```

## Tips
- `recv` only sees data that arrives after it starts: start it first (in the
  background) and then trigger the send.
- Use `idfon <command> --help` for every flag.
