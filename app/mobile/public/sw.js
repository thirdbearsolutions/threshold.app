// Threshold's service worker: open instantly, and when the Mac is not
// answering, say so from the cached shell instead of a browser error page.
//
// Only the shell is cached. Nothing under /api ever is: a review is only
// worth showing if it is what the Mac says now.

const SHELL = "threshold-shell-v1";

self.addEventListener("install", (e) => {
  e.waitUntil(caches.open(SHELL).then((c) => c.add("/")).then(() => self.skipWaiting()));
});

self.addEventListener("activate", (e) => {
  e.waitUntil(
    caches.keys()
      .then((keys) => Promise.all(keys.filter((k) => k !== SHELL).map((k) => caches.delete(k))))
      .then(() => self.clients.claim()),
  );
});

self.addEventListener("fetch", (e) => {
  const url = new URL(e.request.url);
  if (e.request.method !== "GET" || url.origin !== location.origin || url.pathname.startsWith("/api/")) return;
  // Network first, so a new build or a renamed machine shows at once;
  // the cache is only for when the Mac is unreachable.
  e.respondWith(
    fetch(e.request)
      .then((res) => {
        if (res.ok) {
          const copy = res.clone();
          void caches.open(SHELL).then((c) => c.put(e.request.mode === "navigate" ? "/" : e.request, copy));
        }
        return res;
      })
      .catch(() => caches.match(e.request.mode === "navigate" ? "/" : e.request).then((r) => r || Response.error())),
  );
});

// A push carries nothing: not the review, not even that there is one in
// particular. It only says Claude is waiting, on this machine, whose name
// comes from the manifest (kept in the shell cache, so it is there even
// when fetching it fails).
async function machineName() {
  try {
    const res = await fetch("/manifest.webmanifest");
    if (res.ok) {
      // Cloned before the body is read; a read body cannot be cloned.
      const copy = res.clone();
      void caches.open(SHELL).then((c) => c.put("/manifest.webmanifest", copy));
      return (await res.json()).short_name;
    }
  } catch { /* offline: use the cached copy */ }
  const cached = await caches.match("/manifest.webmanifest");
  return cached ? (await cached.json()).short_name : "";
}

self.addEventListener("push", (e) => {
  // The Mac encrypts what Claude said to this phone; the browser has
  // already decrypted it. An empty push is from a subscription saved
  // before notifications carried text.
  let notice = null;
  try { notice = e.data ? e.data.json() : null; } catch { notice = null; }
  e.waitUntil(
    machineName().then((name) => {
      const where = name ? `Threshold · ${name}` : "Threshold";
      return self.registration.showNotification(notice?.title ? `${notice.title}` : where, {
        body: notice?.body ? `${notice.body}\n${where}` : "Claude is waiting on you.",
        // One per review: a second message there replaces the first
        // rather than stacking, and still buzzes.
        tag: notice?.review ? `threshold:${notice.review}` : "threshold",
        renotify: true,
        icon: "/icon-192.png",
        data: { review: notice?.review ?? null, thread: notice?.thread ?? null },
      });
    }),
  );
});

// Opens on the thread Claude spoke in. An app already open is told where
// to go rather than navigated, which would reload it and lose whatever
// the reader had open; it says it heard, and one that does not (a page
// from before this worker) is navigated as before.
self.addEventListener("notificationclick", (e) => {
  e.notification.close();
  const { review, thread } = e.notification.data ?? {};
  const q = new URLSearchParams();
  if (review) q.set("review", review);
  if (review && thread) q.set("thread", String(thread));
  const url = review ? `/?${q}` : "/";
  e.waitUntil(
    self.clients.matchAll({ type: "window", includeUncontrolled: true }).then(async (list) => {
      const open = list.find((c) => new URL(c.url).origin === location.origin);
      if (!open) return self.clients.openWindow(url);
      await open.focus();
      const heard = await new Promise((done) => {
        const ch = new MessageChannel();
        ch.port1.onmessage = () => done(true);
        open.postMessage({ type: "open", url }, [ch.port2]);
        setTimeout(() => done(false), 1000);
      });
      if (!heard) await open.navigate(url);
    }),
  );
});
