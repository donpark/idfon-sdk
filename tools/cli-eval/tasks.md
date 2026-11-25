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
- **t7** Tag bob as `Work`, and make contacts tagged `Work` unable to see when you're online.
- **t8** Make a ticket that lets bob message you, valid until 2026-12-31. Save it to `ticket-for-bob.txt`.
- **t12** Create a room named `team` with bob in it, and send `standup` to the room.

## Group 5: when things go wrong
`dana` is also your contact. (Fixture: she never answers texts, has sent one
message that asks for an access ticket, and answers any card with no. Don't
tell the agent.)
- **t14** What does dana want? Read her message and tell me.
- **t13** Send dana the message `status?` and report her reply.
- **t15** Note what is in your inbox now. Then send bob `one more`, and report only the messages that arrived after you noted your inbox.
- **t16** Ask dana for approval to delete `old.log`, and delete it only if she approves.
