# Threshold

A desktop inbox where a coding agent hands a human a change to review, and
the two of them talk it through before it lands.

An agent that has finished something risky or wide-reaching usually asks
"shall I proceed?" in a line of chat, which asks the human to approve a
change they cannot see. With Threshold the agent commits the work and
writes a review into an inbox: a brief saying what the change is and what
it implies, the diff arranged into chapters in the order that explains it,
and flags on the few lines that matter. The human reads the real diff,
asks questions on any line, and gets answers and revisions back in the
same place. The review ends with an approval or a rejection, which names
the exact diff it is about.

The same review works for a change the agent didn't write. Point it at a
colleague's pull request and it reads the branch and hands it to you the
same way: a brief, chapters, flags, and what it couldn't work out from
the code. You start the PR with a map instead of a file list.

## How a review works

A review is one append-only JSONL file in `~/.threshold/reviews/`, the
**session log**. The agent writes to it, Threshold writes to it, and both
watch it. There is no socket, server or API between them: an agent that
can append a line to a file and read one back can use Threshold. The app
lists every log in the directory as its inbox, so a review outlives the
window and the agent's session alike.

Every line is one event:

```json
{"seq":1,"at_ms":1757255400000,"from":"agent","revision":"main...HEAD","kind":"brief","repo":"/path/to/repo","title":"Move book covers into storage","summary":"…","implications":["Irreversible: each local cover is deleted once it is uploaded","…"]}
```

`seq` is one greater than the highest in the file. `from` is `agent`,
`human` or `threshold`. `revision` records which version of the change
the event was written against. The kinds:

| kind | from | what it is |
| --- | --- | --- |
| `brief` | agent | What the change is and what it implies. Opens the review and names the repo. |
| `bundle` | agent | The narrative: chapters of hunks in the order that explains the change. |
| `note` | either | A comment, on a line, a chapter or the whole review. An agent's note with a `flag` marks a line before anyone has read it. |
| `reply` | agent | An answer to a note. |
| `revised` | agent | The code changed; the diff under review moves. |
| `pinned` | threshold | The review's revision resolved to two commits. |
| `diagnostic` | threshold | A problem with what the agent sent. |
| `chaptermark` | human | The reviewer has been through a chapter. Progress, not approval. |
| `verdict` | human | `approved` or `rejected`, with a reason and the digest of the diff on screen. The last event. |

The agent's side of all this, with every command it needs, is the
[`threshold-review` skill](../skills/threshold-review/SKILL.md). Install
it for Claude Code, or give its contents to any agent that can run shell
commands.

## Reading a review

Open a review from the inbox. The agent's brief comes first, then the
diff:

- **Narrative or files.** The agent's chapters, in its order, or the
  plain per-file view git would give you. Toggle between them. Threshold
  checks that every hunk is in a chapter and shows the count in red when
  one is missing. It will not open on a narrative that leaves something
  out, so you can't approve hunks you weren't shown.
- **Flags.** The agent marks the lines that matter as `attention` (the
  load-bearing line), `caution` (the risky one) or `context` (looks odd,
  is not). A flag whose quoted text no longer matches its line is
  dropped rather than pointed at code the agent never looked at.
- **Notes and threads.** Comment on any line, on a chapter, or on the
  review as a whole. The agent replies in a thread, and notes made
  against older code say so rather than silently re-anchoring. A note
  follows its line through an amend when the line is still there.
- **Markdown and images.** A changed Markdown file is rendered, with the
  changed blocks marked, and its relative images inline. Changed images
  are shown, before and after.
- **The facts.** File and line counts, whether anything is deleted, and
  whether a test file is touched.
- **Mark chapters** as you go, then **Approve** or **Reject**. A
  rejection needs a reason, and the agent treats it as an instruction.

Reviews can be archived. **History** shows every past verdict and what
it was about.

## Someone else's pull request

Ask an agent to read a colleague's PR and hand it to you in Threshold:

```
Read PR 214 (branch feature/cover-storage) and hand it to me in Threshold.
Chapters in the order that explains it, flags on what I should look at,
and tell me what you couldn't work out from the code.
```

The agent fetches the branch and writes the review for a range such as
`main...origin/feature/cover-storage`, which pins to those commits like
any other. You read it, ask about anything, and take what you've learned
back to the PR. The agent didn't write the change, so it can explain it
but not revise it, and its brief is a reading rather than the author's
intent: where it couldn't work out why, that's a question for the author.

Threshold also works without an agent. **File ▸ Open Folder… (⌘O)**
opens any repository as a plain diff of its working tree, its staged
changes, or a revision such as `main...HEAD`. There's no brief, narrative
or conversation there, and a verdict only goes into History.

## The first run

The first time Threshold opens, it asks for a theme and then offers a
tour. **Help ▸ Take the Tour** runs it again. The tour builds a small
sample repo in `~/.threshold/sample/stacks` and a review of it in the
inbox, then points at each part of a real review in turn. Done or
skipped, the sample review is archived, and its verdict never goes into
History.

Until a real review has arrived, the main pane says how to get one.
**Install for Claude Code** copies the skill this build ships into
`~/.claude/skills/threshold-review/`. A matching copy is left alone, and
a different one is only replaced when you say so.

## Pinning: the diff does not move while it is read

A revision spec is not a thing, it is a question. `main...HEAD` resolves
differently once the agent commits again or somebody advances `main`, so
a review rendered by re-running `git diff` would show a different change
every time the human came back to it, silently, with the narrative still
reading as authoritative, notes re-anchored to whatever now occupied
their line, and the verdict recorded against the spec rather than the
change.

So Threshold resolves the spec once, when the review lands in the inbox,
to the two commits git would compare (the merge base and the head, for a
`...` range), records them in the log as a `pinned` event, and diffs
*those* from then on. Two object ids name exactly one diff forever. The
verdict carries the SHA-256 of the diff the reviewer had on screen, so
the agent can prove the approval is about what it is about to ship:

```sh
git diff main...HEAD | shasum -a 256   # must equal the verdict's digest
```

Landing, not opening: the commits a review is about are the ones the
agent handed over, not whatever the branch points at by the time someone
gets to it. The same background pass runs the rest of the checks (an
incomplete bundle, hunks the bundle names that do not exist, an empty
diff, a spec git cannot resolve) and writes them into the log as
diagnostics, for the agent that is already watching it. It waits until a
log has been still for a few seconds, so it never reports on a review
that is still being written.

Not every spec can be pinned, and the ones that cannot are said out loud.
`HEAD~3` and a bare `git diff` compare against the **working tree**, so
half of the diff is whatever is on disk when it is rendered. Those reviews
still open, but carry a warning, a `review_unpinned` diagnostic for the
agent, and a background check that says so when the diff moves mid-read.

An amended commit comes in through a `revised` event, which retires the
pin. The spec string is the same across an amend, so the event is the
only thing separating the old commits from the new.

A revision spec is validated before it reaches git. A leading `-` is
rejected, because git would read it as an option (`--output=` writes a
file), and a `--` separator follows it so it cannot be read as a path.
Whether the revision exists is left to git, whose error is passed
straight back.

## Staying current

Once per clone, run `scripts/install-hooks.sh`. After that, every commit,
merge, rebase or checkout in the main checkout rebuilds Threshold in the
background, once `scripts/install.sh` has installed it, if its sources
changed. If Threshold is closed, the new build
is installed straight away. If it's open, the build waits, and the header
shows **Update ready · Restart**. Click it and Threshold comes back on the
new build, where you left it. Quitting Threshold installs the waiting build
too.

`scripts/install.sh --check` says what the installed app was built from.
The log is `~/Library/Logs/threshold-auto-install.log`.

## Your phone

Threshold can serve its reviews to your phone over Tailscale, so an agent
waiting on a question isn't stuck until you're back at a desk. From the
phone you can read, ask, mark chapters, and approve or reject, and it
notifies you when an agent has said something you're waiting on.

On each Mac, from the repo root:

```sh
scripts/install.sh                       # a build that includes the phone app
scripts/threshold-mobile-setup.sh        # Tailscale, a name, a colour
```

The setup script installs Tailscale if needed. It then asks what to call
this Mac. That one name becomes its tailnet address
(`https://<name>.<tailnet>.ts.net`), its home-screen label, and the band
across the top of every phone screen, in the colour you pick. Give each
Mac a different name and colour. Each gets its own home-screen app, with
its own pairing, and nothing is shared between them.

Then open Threshold → **Phone** on the Mac and scan the code with the
phone's camera. Phones paired there can be revoked there.

How it stays safe:

- Threshold listens on 127.0.0.1 only. `tailscale serve` puts it on your
  tailnet over HTTPS, and never on the public internet.
- A verdict from the phone names the diff it read, and is refused if that
  diff has changed since.
- Requests need a device credential from pairing. The QR code works once,
  for one minute.
- A review is named by its file name in the inbox, never by a path, so a
  request can't choose where git runs.
- Every write names the machine it's meant for. If a phone's page has
  ended up talking to a different Mac, the write is refused.
- What the phone writes goes through Threshold's own process, so the
  phone and the desk can't race on a log.
- Notifications are encrypted end to end to the phone (RFC 8291), so the
  push service relays them without being able to read them.

The phone works only while Threshold is running on that Mac, and it says
so when it isn't. `scripts/threshold-mobile-setup.sh --off` turns it off.

## Design notes

- **A file, not a protocol.** The session log is the whole interface
  between agent and human. It is inspectable with `cat`, survives
  restarts on both sides, and any agent that can run a shell can speak
  it. Each event is one complete line, which is what makes concurrent
  appends safe.
- **The agent's claims and the ground truth look different.** Chapters
  and flags are the agent's reading of the diff. Threshold audits them
  against the real diff (coverage, quoted line text) and says so when
  they disagree, instead of trusting them.
- **Threshold reports, and does not refuse.** A review that cannot be
  pinned, or whose bundle is incomplete, still opens. The problem is
  shown to the reviewer and sent to the agent as a diagnostic, so the
  agent can fix it while the reviewer reads.
- **The diff comes from real `git diff`**, parsed by a unified-diff
  parser (`crates/threshold-core`) that handles the shapes git actually
  produces: modified, added, deleted, renamed (including a pure rename),
  and binary.
- **A permanent decision log**, alongside the per-review logs: every
  verdict, with the revision it was about and its digest. A plain,
  append-only JSON array.

## What's tested, and how

`crates/threshold-core` has unit tests for the diff parser (every file
shape git produces, including that a pure rename has no hunks), the diff
facts (including that the test-file rule catches `tests/`, `__tests__/`,
`test_*.py` and `*.test.ts` but not `latest_report.rs` or
`contest_winner.rs`), the session log and decision log round trips,
revision-spec validation (accepting `main...HEAD`, `HEAD~1`,
`v1.2^{commit}`, `release/2.0` and raw object ids, rejecting a leading
dash, control characters and an over-long spec), bundle coverage, pinning
(that `...` is not mistaken for `..`, that only a full object id counts as
one, that a `revised` event retires the pin even when the spec string is
unchanged), and line anchors against hand-built diffs (insertions,
deletions, a changed line, a rename, the text fallback for an unpinned
review).

The Tauri shell is tested against real repositories. A pinned review
renders the same diff across a new commit on `main`, and in the same
test the unpinned render of the same spec does not, so the test cannot
pass by measuring nothing. A review nobody has opened is pinned and
checked in the background exactly once, while one still being written,
an empty log and a decided review are left alone. A line note follows
its code through a real `git commit --amend`. The phone server is tested
for pairing, token expiry and revocation, refusing an unpaired caller, a
review name that escapes the inbox and a write meant for another machine,
verdicts that name the diff that was read, and push encryption against
the RFC 8291 test vector.

The UI's pure logic (the inbox, threads, drafts, Markdown rendering, the
outline, and the scroll invariant: a region's position is a function of
its key, and rendering never changes it) is tested with `npm test`.

## Status

In daily use on macOS.
