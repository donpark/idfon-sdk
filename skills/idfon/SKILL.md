---
name: idfon
description: Use the idfon CLI to message contacts, send and receive files, share download tickets, run live audio and calls, and manage who may reach you, over idfon's peer-to-peer network.
---

# idfon CLI

`idfon` talks to a local daemon, which it starts automatically. You act as
one **identity** (one you); the people and agents you talk to are **peers**. Name a
peer by its name, alias or id anywhere a command takes `PEER`.
Commands print short human text; add `--json` for machine output.

## Everyday commands
```
idfon peer add carol.vcf            # add a contact: .vcf card, idfon://add/... link, or MCP ticket
idfon peer list | show PEER | remove PEER

idfon send bob "hello"              # message a peer
idfon send bob "hello" --wait       # ...and print the reply as `bob: <text>` (--timeout 60s)
idfon inbox [--from bob] [-f]       # messages you received; -f keeps following

idfon send bob --file notes.txt     # send a file (waits for delivery)
idfon recv -o copy.txt [--from bob] # save a file sent to you (takes one from the
                                    #   last 2 minutes, else waits for the next)

idfon put notes.txt                 # store a file; prints a ticket anyone can download
idfon get TICKET -o copy.txt        # download a ticket

idfon live publish song.wav --loop  # broadcast audio; prints its ticket and returns
                                    #   (the daemon keeps broadcasting)
idfon live list | stop ID           # running broadcasts: id, kind, ticket
idfon live watch TICKET -o rec.wav  # record someone's broadcast
idfon call bob --file song.wav      # 1:1 call; blocks until hangup
idfon answer -o call.wav            # take the next incoming call

idfon room create team --member bob # group with members
idfon room send team "standup"

idfon access check bob [CAPABILITY] # may you message bob? (default message.send)
idfon access grant bob              # allow sending to and receiving from bob
idfon access ticket bob --expires 2026-12-31   # a ticket that lets bob message you
idfon identity rename "Ann Lee"     # your name (there is one identity)
idfon tag add bob Work              # tag a contact; filters and what they see
idfon tag set Work --hide-presence  # Work contacts can't see when you're online
idfon visibility --photo off        # hide your photo from everyone by default
```

## Notes
- Durations: `30s`, `5m`, `2h`, `30d`. Dates: `2026-12-31`.
- Files you sent show in a peer's `inbox` as `[file, N bytes]`; they save it with `idfon recv`.
- `idfon rpc METHOD '{json}'` calls any daemon method directly.
- `idfon config show | set KEY VALUE | unset KEY` manage settings (env var wins over `~/.idfon/config.toml`); `--profile NAME` runs a separate daemon.
- `idfon <command> --help` lists every flag.
