# CLI eval tasks

Each group runs in one fresh agent against its own fixture
(`fixture.sh up /tmp/idfe-<n>`). The agent is "me"; `bob` is already a
contact who can message you and auto-replies `pong: <text>`. Files are in
`$FIX/work`. `check.sh` grades each task.

## Group 1
- **t1** Carol sent you her contact card, `carol.vcf`. Add her as a contact, then show her details.
- **t2** Send bob the message `hello` and wait for his reply. Report the reply text.
- **t9** Are you allowed to send messages to bob? Answer yes or no.

## Group 2
- **t3** Send the file `notes.txt` to bob.
- **t4** Bob will send you a file when you message him `send file`. Ask him, then save the file he sends to `received.txt`.
- **t11** What messages have you received from bob? List their texts.

## Group 3
- **t5** Store `notes.txt` so that anyone with a ticket can download it. Report the ticket.
- **t6** `ticket.txt` holds a download ticket. Download its content into `out.bin`.
- **t10** Broadcast `song.wav` as live audio on a loop. Report its ticket, then list running broadcasts, then stop it.

## Group 4
- **t7** Create a second identity named `work`, switch to it, then switch back to your original identity.
- **t8** Make a ticket that lets bob message you, valid until 2026-12-31. Save it to `ticket-for-bob.txt`.
- **t12** Create a room named `team` with bob in it, and send `standup` to the room.
