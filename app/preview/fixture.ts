// A made-up review for previewing the UI in a browser: one repo, one
// change, a brief, a narrative, flags and a thread. Nothing here is read
// from disk. The change is the tour's sample (src-tauri/src/sample.rs).

const DIFF = `diff --git a/stacks/commands/move_covers.py b/stacks/commands/move_covers.py
new file mode 100644
index 0000000..b698c80
--- /dev/null
+++ b/stacks/commands/move_covers.py
@@ -0,0 +1,12 @@
+"""Copy every cover into storage, then delete the local file."""
+
+import os
+
+from stacks import settings, storage
+
+
+def run():
+    for name in sorted(os.listdir(settings.COVERS_DIR)):
+        path = os.path.join(settings.COVERS_DIR, name)
+        storage.put(f"covers/{name}", path)
+        os.remove(path)
diff --git a/stacks/covers.py b/stacks/covers.py
index 534e645..5346d21 100644
--- a/stacks/covers.py
+++ b/stacks/covers.py
@@ -1,11 +1,9 @@
-import os
+from stacks import storage
 
-from stacks import settings
 
-
-def cover_path(book):
-    return os.path.join(settings.COVERS_DIR, f"{book.isbn}.jpg")
+def cover_key(book):
+    return f"covers/{book.isbn}.jpg"
 
 
 def cover_url(book):
-    return f"/covers/{book.isbn}.jpg"
+    return storage.url(cover_key(book))
diff --git a/stacks/settings.py b/stacks/settings.py
index 8c09fbe..5c1bde6 100644
--- a/stacks/settings.py
+++ b/stacks/settings.py
@@ -1 +1,2 @@
 COVERS_DIR = "/var/stacks/covers"
+COVERS_BUCKET = "stacks-covers"
diff --git a/stacks/views.py b/stacks/views.py
index a766029..cb61ec3 100644
--- a/stacks/views.py
+++ b/stacks/views.py
@@ -1,5 +1,7 @@
-from stacks import covers
+from stacks import covers, storage
 
 
 def book_card(book):
-    return {"title": book.title, "cover": covers.cover_url(book)}
+    key = covers.cover_key(book)
+    cover = covers.cover_url(book) if storage.exists(key) else "/static/no-cover.svg"
+    return {"title": book.title, "cover": cover}
diff --git a/tests/test_covers.py b/tests/test_covers.py
index 845d6d4..7ee131f 100644
--- a/tests/test_covers.py
+++ b/tests/test_covers.py
@@ -1,5 +1,9 @@
-from stacks import covers
+from stacks import covers, views
 
 
 def test_cover_url_uses_the_isbn(book):
     assert covers.cover_url(book).endswith(f"{book.isbn}.jpg")
+
+
+def test_a_missing_cover_shows_the_placeholder(book_without_cover):
+    assert views.book_card(book_without_cover)["cover"] == "/static/no-cover.svg"
`;

type Line = { kind: "context" | "added" | "removed"; content: string };
type File = { old_path: string | null; new_path: string | null; status: string; hunks: { header: string; lines: Line[] }[]; additions: number; deletions: number };

export function parse(diff: string): File[] {
  const files: File[] = [];
  let f: File | null = null;
  for (const raw of diff.split("\n")) {
    if (raw.startsWith("diff --git")) {
      f = { old_path: null, new_path: null, status: "modified", hunks: [], additions: 0, deletions: 0 };
      files.push(f);
    } else if (!f) continue;
    else if (raw.startsWith("new file")) f.status = "added";
    else if (raw.startsWith("--- ")) f.old_path = raw === "--- /dev/null" ? null : raw.slice(6);
    else if (raw.startsWith("+++ ")) f.new_path = raw.slice(6);
    else if (raw.startsWith("@@")) f.hunks.push({ header: raw.replace(/^(@@[^@]*@@).*$/, "$1"), lines: [] });
    else if (f.hunks.length && raw !== "" || (f.hunks.length && raw === "")) {
      const h = f.hunks[f.hunks.length - 1];
      if (raw.startsWith("+")) { h.lines.push({ kind: "added", content: raw.slice(1) }); f.additions++; }
      else if (raw.startsWith("-")) { h.lines.push({ kind: "removed", content: raw.slice(1) }); f.deletions++; }
      else if (raw.startsWith(" ")) h.lines.push({ kind: "context", content: raw.slice(1) });
    }
  }
  return files;
}

export const REPO = "/Users/you/code/stacks";
export const REVIEW = "2026-10-28-101500-stacks.jsonl";
export const files = parse(DIFF);
const path = (f: File) => f.new_path ?? f.old_path ?? "";
const hunk = (file: string, i = 0) => ({ file, header: files.find((f) => path(f) === file)!.hunks[i].header });

export const chapters = [
  { title: "Covers come from storage", summary: "The idea: a cover is a key in storage, not a file on this server.", hunks: [hunk("stacks/covers.py"), hunk("stacks/settings.py")] },
  { title: "Pages cope with a missing cover", summary: "A placeholder instead of a broken image, and a test for it.", hunks: [hunk("stacks/views.py"), hunk("tests/test_covers.py")] },
  { title: "Moving the files", summary: "A one-off command, and the irreversible part.", hunks: [hunk("stacks/commands/move_covers.py")] },
];

const rev = "main...HEAD";
let t = Date.parse("2026-10-28T10:15:00Z");
const at = () => (t += 60_000);
export const events: any[] = [
  { seq: 1, at_ms: at(), from: "agent", revision: rev, kind: "brief", repo: REPO, title: "Move book covers into storage",
    summary: "Book covers move from a folder on the server into a storage service. Pages show a placeholder when a cover is missing, and a one-off command moves the existing files.",
    implications: ["Irreversible: the command deletes each local cover once it is uploaded", "Pages now depend on storage being reachable", "I have not run the command against a full set of covers, which is worth a second look"] },
  { seq: 2, at_ms: at(), from: "agent", revision: rev, kind: "bundle", chapters },
  { seq: 3, at_ms: at(), from: "threshold", revision: rev, kind: "pinned", base: "3f9e1c2b7a", head: "a71d0b4e19", digest: "9c1d…" },
  { seq: 4, at_ms: at(), from: "agent", revision: rev, kind: "note", file: "stacks/commands/move_covers.py", line: 12, line_content: "        os.remove(path)", flag: "caution", text: "This is the irreversible one: each local cover is deleted once it is uploaded." },
  { seq: 5, at_ms: at(), from: "agent", revision: rev, kind: "note", file: "stacks/covers.py", line: 9, line_content: "    return storage.url(cover_key(book))", flag: "attention", text: "Every cover URL now comes from here." },
  { seq: 6, at_ms: at(), from: "agent", revision: rev, kind: "note", file: "stacks/settings.py", line: 2, line_content: "COVERS_BUCKET = \"stacks-covers\"", flag: "context", text: "Read once at startup, like COVERS_DIR above it." },
  { seq: 7, at_ms: at(), from: "human", revision: rev, kind: "note", file: "stacks/views.py", line: 6, line_content: "    cover = covers.cover_url(book) if storage.exists(key) else \"/static/no-cover.svg\"", text: "Doesn't this ask storage about every book on every page?" },
  { seq: 8, at_ms: at(), from: "agent", revision: rev, kind: "reply", in_reply_to: 7, text: "Once per book card, yes. A page shows twenty at most and `storage.exists` is a cached lookup, so it costs a few milliseconds." },
  { seq: 9, at_ms: at(), from: "human", revision: rev, kind: "note", text: "What happens to a cover uploaded while the command runs?" },
];

export const reviews = [
  { path: REVIEW, title: "Move book covers into storage", repo: REPO, revision: rev, decided: false, last_at_ms: t, events: 9, spoken: 8, archived: false },
  { path: "2026-10-28-091200-stacks.jsonl", title: "Rename Member.email_address to email", repo: REPO, revision: rev, decided: false, last_at_ms: t - 3_600_000, events: 4, spoken: 3, archived: false },
  { path: "2026-10-27-162000-docs.jsonl", title: "Rewrite the onboarding guide", repo: "/Users/you/code/docs", revision: rev, decided: false, last_at_ms: t - 70_000_000, events: 6, spoken: 6, archived: false },
  { path: "2026-10-27-110500-stacks.jsonl", title: "Bump Django to 5.2", repo: REPO, revision: rev, decided: true, last_at_ms: t - 90_000_000, events: 5, spoken: 4, archived: false },
];
