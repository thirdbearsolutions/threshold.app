---
name: threshold-review
description: Hand a committed change to the human for review in Threshold, a desktop review inbox, and stay with it until they decide — a brief and a narrative of the diff, flags on the lines that matter, answers to their notes, revisions, then acting on their verdict. Use before finalizing anything destructive or wide-reaching — a mass rename, a schema migration, a bulk delete, a dependency bump across many files, a force-push, a large refactor — and whenever the user asks to "review this in Threshold", "let me approve this first", or "show me the diff before you continue". Shows them the real diff instead of asking for permission in a line of chat.
---

# Reviewing a risky change in Threshold

Asking "shall I proceed?" in chat asks the user to approve a change they
cannot see. Threshold shows them the actual diff — every file, every hunk
— and writes their verdict to a file this session waits on.

## When to use this

Reach for it when a change is hard to reverse or too large to judge from a
sentence:

- a mass rename, a codemod, a bulk delete
- a schema migration, especially one dropping or rewriting columns
- a dependency bump touching many files
- deleting or rewriting files the user did not explicitly name
- anything the user has said they want to see before it lands

Do **not** use it for ordinary edits. A two-line fix reviewed through a
desktop app is friction, not safety. And it is not a substitute for the
normal confirmation rules: it replaces "approve this blind", not "ask
before doing something destructive".

## The flow

A review is a **conversation**, not a prompt. The transport is one
append-only JSONL file — the session log — that both you and Threshold
append to and watch. A verdict is the terminal event, not the only thing
the human can say.

**Commit to a branch first and hand over a two-commit range.** Threshold
renders a review by running `git diff` at the moment the human looks at
it, so a `revision` naming only one commit — `HEAD~3`, or nothing at all
— is compared against the working tree and moves every time anything on
disk does. A range like `main...HEAD` names two commits, and Threshold
**pins** the review to them as soon as it lands in the inbox — a few
seconds after your last write, whether or not anyone has opened it: from
then on it diffs those object ids, so the diff cannot change under the
reviewer however the branch moves afterwards.

A review it cannot pin still opens, with a warning across the diff and a
`review_unpinned` diagnostic in the log addressed to you. Fix it by
committing and pushing a corrected `revision`, not by asking the human to
read faster.

### 1. Write the brief and push it into the inbox

The brief is the point. The human is about to read a diff you already
understand; tell them what it is, what it implies, and what you are unsure
about. A review with no brief is the old two-button prompt again.

Threshold is an app the user leaves open. You do **not** launch it and do
not own its lifetime — you write a session log into the inbox and it
appears in their list:

```bash
SESSION=~/.threshold/reviews/$(date +%Y-%m-%d-%H%M%S)-$(basename "$PWD").jsonl
mkdir -p ~/.threshold/reviews
echo "$SESSION"     # remember it; every later step needs this literal path

cat > "$SESSION" <<JSON
{"seq":1,"at_ms":$(date +%s000),"from":"agent","revision":"main...HEAD","kind":"brief","repo":"/absolute/path/to/repo","title":"Move book covers into storage","summary":"Covers move from a folder on the server into a storage service, and a one-off command moves the existing files. Pages show a placeholder when a cover is missing.","implications":["Irreversible: the command deletes each local cover once it is uploaded","Pages now depend on storage being reachable","I have not run the command against a full set of covers, which is worth a second look"]}
JSON
```

`repo` is required and must be absolute. The log has to be
self-describing: the user may open this review tomorrow, in an app you
never launched, long after your session has ended.

Name the file so it reads well in a list — a date and the repo, as above.
It is a heading, not an id.

**One review, one new file, never a path you have used before.** Several
Claude sessions run on one machine, and a shared path cross-wires them:
both sessions wake on whichever event lands first, so one reads the
human's decision about the other's diff as approval of its own. That is a
fabricated approval and neither side can detect it. The timestamp in the
name is what keeps them apart, so let it be the current time rather than
copying a path from an earlier review or from this page.

`$SESSION` lives only inside the command that sets it — later steps run
as separate commands, where it is empty. Echo it, and carry the literal
path forward. Every snippet below shows one, and yours will differ.

Then **say in chat that a review is waiting**, and name it. The app shows
a count, but they may not be looking at it. They open it from the list,
so the name you gave the file is how they find it.

If Threshold is not running, say so rather than launching it. It is their
app, and a window appearing unbidden is worse than a sentence:

```bash
pgrep -f 'Threshold.app' >/dev/null || echo "Threshold isn't running — the review is waiting in ~/.threshold/reviews"
```

### 1b. Rebundle the diff into a narrative

For anything beyond a few hunks, follow the brief with a `bundle`:
chapters of hunks in the order that explains the change, rather than the
path order git happens to emit. The reviewer toggles between your reading
and the plain file view.

```bash
echo '{"seq":'$(( $(wc -l < ~/.threshold/reviews/2026-09-10-143005-shop.jsonl) + 1 ))',"at_ms":'$(date +%s000)',"from":"agent","revision":"main...HEAD","kind":"bundle","chapters":[{"title":"The new column and its constraint","summary":"The actual idea. Everything else follows from this shape.","hunks":[{"file":"migrations/003.sql","header":"@@ -0,0 +1,40 @@"}]},{"title":"Mechanical: 22 call sites","summary":"Rename only. Skim or skip.","hunks":[{"file":"src/db.rs","header":"@@ -14,7 +14,7 @@"}]}]}' >> ~/.threshold/reviews/2026-09-10-143005-shop.jsonl
```

Get the hunk headers from `git diff <rev>` — the `@@ …` lines, verbatim,
paired with the file each belongs to.

**Binary files are welcome, and images are shown.** If a picture explains
the change better than prose (a rendered diagram, a screenshot, a
before/after), commit it and put it in the review. Threshold shows images
(PNG, JPEG, GIF, WebP, AVIF, SVG, BMP, ICO) from the review's own commits:
an added one on its own, and a changed one as before and after, side by
side. A Markdown file's relative images (`![overview](overview.png)`)
render inline, so a design doc and its figures read together. Other
binaries are listed but not shown.

A binary file has no `@@` lines. Its one hunk is the line git prints for
it instead, verbatim:

```json
{"file":"docs/design/overview.png","header":"Binary files /dev/null and b/docs/design/overview.png differ"}
```

It counts toward coverage like any other hunk, so put each image in the
chapter it illustrates.

**Every hunk must be in a chapter.** Threshold audits this and shows the
reviewer "10 of 13 hunks in a chapter" in red, lists whatever you left
out as its own section, and refuses to open on your narrative at all when
it is incomplete. That is deliberate: a tidy story about nine hunks that
wins approval for ten is the worst thing this tool could do. Account for
everything, including the boring parts — a chapter that says "mechanical,
skim this" is a real chapter and does that job honestly.

Chapters may overlap when a hunk genuinely serves two purposes.

A bundle is tied to its revision. **After a `revised` event, send a new
one**, or the reviewer is reading a narrative about code that has
changed.

### 1c. Mark the lines that actually matter

Out of four hundred changed lines there are usually three that could
bite. You know which. Flag them when you push, so they are marked before
the reviewer has read anything — otherwise the only way to say it is
prose they read before they have seen any code.

```bash
echo '{"seq":'$(( $(wc -l < ~/.threshold/reviews/2026-09-10-143005-shop.jsonl) + 1 ))',"at_ms":'$(date +%s000)',"from":"agent","revision":"main...HEAD","kind":"note","file":"src/db.rs","line":142,"line_content":"    conn.execute(\"DROP TABLE ...\")?;","flag":"caution","text":"This is the irreversible one. Everything else in the change is additive."}' >> ~/.threshold/reviews/2026-09-10-143005-shop.jsonl
```

`file` and `line` are the new-file path and line number, and
`line_content` is that line's text — the same anchoring as any note.

Copy `line_content` **verbatim** from the diff. Threshold compares it
against the line the flag lands on and drops the flag when they differ,
because a mark pointing confidently at code you never looked at is worse
than no mark. Get it wrong and your flag silently does not appear.

Three kinds, and no more:

| flag | means | use it for |
| --- | --- | --- |
| `attention` | the load-bearing line | the decision everything else follows from |
| `caution` | the risky one | irreversible, or the thing most likely to be wrong |
| `context` | looks odd, is not | the line that will otherwise cost a question |

**`context` is the one to reach for most.** It spends a sentence to save
the reviewer asking, and unlike the other two it does not cry wolf. Use
`caution` sparingly — flag five things as risky and none of them reads as
risky.

A flag is a pointer, not a question, so nothing is waiting on you after
you write one. The reviewer can reply to it, and it becomes an ordinary
thread.

### 1d. Read what Threshold says about it

Threshold checks a review once its log has been still for a few seconds,
without waiting for anyone to open it, and writes what it finds back
into the log as `"from": "threshold"`. Give it a moment, then read those
lines:

```bash
sleep 6; grep '"from": *"threshold"' ~/.threshold/reviews/2026-09-10-143005-shop.jsonl
```

A `pinned` line is the review settling onto its commits and needs
nothing from you. A `diagnostic` does: fix it now, while the reviewer
may not have started, rather than after they have read a narrative that
leaves hunks out. Nothing at all means Threshold is not running.

Threshold's lines take `seq` numbers too, which is why every snippet
here counts lines in the file rather than assuming the next number is
yours.

### 2. Wait for them to say something

Keep a **cursor**: the last line of the log you have actually read. Not
the length of the file when you start waiting — the reviewer can write
while you are reading, replying or revising, and a wait that starts from
"how long the file is now" swallows whatever landed in that gap. The
thread is in the log, nothing wakes you for it, and it sits there as
"waiting for Claude…" until the review ends.

After your opening writes the cursor is the line count once 1d has read
Threshold's lines. From then on it only moves when you read (step 3),
never when you write.

```bash
until awk -v c=9 'NR>c && !/"from": *"agent"/{f=1} END{exit !f}' ~/.threshold/reviews/2026-09-10-143005-shop.jsonl; do sleep 2; done
```

Backgrounded, with your own path and your cursor for `c`. It wakes on
any line past the cursor that is not yours — usually the reviewer,
sometimes Threshold reporting on a revision you pushed — including lines
that arrived before the loop started, in which case it returns at once.
Your own replies past the cursor do not wake it.

### 3. Read what arrived, and respond in kind

Read everything past the cursor, numbered:

```bash
awk -v c=9 'NR>c{print NR": "$0}' ~/.threshold/reviews/2026-09-10-143005-shop.jsonl
```

The last number printed is your new cursor. Take it from this output,
not from a fresh `wc -l`: a line written after this read is one you have
not seen, and it has to stay past the cursor so the next wait catches it.

Deal with **every** line in what you read, not just the newest. A
reviewer working through the diff often leaves several notes in one
burst, and each is its own thread.

- **`kind: "note"`** — a question or an annotation. `file`, `line` and
  `line_content` are set when it is attached to a line; `line_content` is
  how you find the code, since line numbers shift once you revise.
- **`kind: "verdict"`** — the review is over.

Answer a note by appending a reply, quoting its `seq`:

```bash
echo '{"seq":'$(( $(wc -l < ~/.threshold/reviews/2026-09-10-143005-shop.jsonl) + 1 ))',"at_ms":'$(date +%s000)',"from":"agent","kind":"reply","in_reply_to":2,"text":"..."}' >> ~/.threshold/reviews/2026-09-10-143005-shop.jsonl
```

If the right answer is a code change, **make it** — amend the commit, then
announce the new revision so the diff reloads under them. The `revised`
event is what retires the pin, so send one even when the spec string is
unchanged: `main...HEAD` still reads `main...HEAD` after an amend, and
without the event Threshold goes on rendering the commit you replaced.

```bash
echo '{"seq":'$(( $(wc -l < ~/.threshold/reviews/2026-09-10-143005-shop.jsonl) + 1 ))',"at_ms":'$(date +%s000)',"from":"agent","kind":"revised","to":"main...HEAD","note":"dropped the cascade"}' >> ~/.threshold/reviews/2026-09-10-143005-shop.jsonl
```

`seq` must be one greater than the highest already in the file, and the
file is not only yours — hence counting its lines at the moment you
write. Each line must be a single complete line of JSON — that is what
makes concurrent appends safe, and what makes the line count equal the
highest `seq`.

Then wait again from your cursor. Loop until a verdict arrives.

### 4. The verdict

On **approved**, proceed, saying what you are proceeding with. On
**rejected**, stop and treat the reason as the instruction it is. Never
re-submit the same diff hoping for a different answer, and never work
around a rejection.

**Check the verdict's `digest` before treating it as authoritative.** It
is the SHA-256 of the diff the human actually had on screen. Checking
`revision` instead proves nothing — it is the same `main...HEAD` you sent,
whatever it now resolves to:

```bash
git diff main...HEAD | shasum -a 256   # must equal the verdict's digest
```

A mismatch means the code moved between the render they judged and now,
so the approval is not about what is in the tree. Re-push rather than
proceed. A verdict with no `digest` at all came from a review that could
not be pinned; treat it as covering only what the log says was reviewed.

## Gotchas

- **The app must be installed.** If
  `/Applications/Threshold.app` is missing or out of date
  (`scripts/install.sh --check` in the Threshold repo), install it with
  `scripts/install.sh`. This builds from HEAD and
  replaces the copy in `/Applications`; `npm run app:build` alone only
  writes to the workspace's `target/`.
- **Closing the window does not cancel a review.** The list is built from
  the inbox directory, so a review outlives both the window and you. If
  your session ends mid-review the user can still open it, read the whole
  conversation and decide — they simply get no further replies from you.
- **Answer promptly.** An unanswered annotation shows as "waiting for
  Claude…" in the UI. If you are going to be a while, say so in a reply.
- **A narrative is not a substitute for the brief.** The brief says what
  the change is and what it implies; chapters say how to read it.
- **A rejection is still logged.** Every decision is appended to
  Threshold's permanent log with its `source` and `digest`.
- **Threshold talks back in the log.** Besides `bundle_incomplete` and
  `bundle_phantom_hunks`, watch for `review_unpinned` (the diff can move
  while it is read — hand over a two-commit range), `pin_failed` (git
  could not resolve the spec), `pin_unreachable` (the pinned commits are
  gone, usually an amend without a `revised` event) and `diff_drifted`
  (an unpinned diff changed mid-read).
- **Empty diff, no review.** If the revision has no changes there is
  nothing to approve; check the diff is non-empty before you push, so the
  user is not handed an empty review. Threshold says so too, as
  `diff_empty`, but by then the review is already in their list.
