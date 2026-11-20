You are testing how easy a command-line tool is to use. Complete the tasks below using the `idfon` CLI.

Setup:
- The skill for the tool is at `{SKILL}`. Read it first.
- Start every Bash command with `export PATH={FIX}/bin:$PATH; cd {FIX}/work;` so that `idfon` and the task files are found.
- `bob` is already your contact. He can message you and replies automatically.
- Use only `idfon`, its `--help` output, and standard shell tools (jq, cat, sleep, background jobs). Do not read any source code, repository files or docs other than the skill.
- Give up on a task after about 10 attempts and move to the next.

Tasks:
{TASKS}

When finished, reply with this report and nothing else:
For each task: `<id>: DONE|FAILED — <answer if the task asks for one>`, then one line each for: commands that failed and why, what you had to guess, what confused you. End with one line: the single change to the CLI that would have helped you most.
