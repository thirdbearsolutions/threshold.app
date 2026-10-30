// The phone's side of the wire: its credential, which machine it belongs
// to, and every call to the Mac.
//
// Everything is per origin, and each Mac is its own origin
// (https://<name>.<tailnet>.ts.net), so two Macs never share a credential,
// a machine record or a cache. Nothing here can talk to a Mac other than
// the one that served this page.

import type { BundlePayload, ChapterState, DiffPayload, LineAnchor, Mark, SessionEvent } from "../src/api";

export interface Machine { id: string; name: string; color: string }

export interface ReviewRow {
  name: string;
  title: string;
  project: string | null;
  revision: string | null;
  decided: boolean;
  archived: boolean;
  last_at_ms: number;
}

export interface ChapterRow extends ChapterState { title: string }

export type { BundlePayload, DiffPayload, LineAnchor, Mark, SessionEvent };

const CREDENTIAL = "threshold.credential";
const MACHINE = "threshold.machine";

function read(key: string): string | null {
  try { return localStorage.getItem(key); } catch { return null; }
}
function write(key: string, value: string | null): void {
  try {
    if (value === null) localStorage.removeItem(key);
    else localStorage.setItem(key, value);
  } catch { /* private mode: pairing will not survive a reload, which it says */ }
}

export function credential(): string | null { return read(CREDENTIAL); }

/** The machine this page was paired with, as last confirmed. */
export function savedMachine(): Machine | null {
  try { return JSON.parse(read(MACHINE) ?? "null"); } catch { return null; }
}

export function forget(): void {
  write(CREDENTIAL, null);
}

/** Why a call failed, in the terms the screen needs. */
export class ApiError extends Error {
  constructor(public status: number, message: string) { super(message); }
  /** The Mac is not answering: Threshold is closed, or the phone is off
   *  the tailnet. Not an error in anything the reviewer did. */
  get unreachable(): boolean { return this.status === 0 || this.status === 502 || this.status === 503 || this.status === 504; }
}

async function call<T>(method: "GET" | "POST", path: string, body?: unknown): Promise<T> {
  const headers: Record<string, string> = {};
  const c = credential();
  if (c) headers.authorization = `Bearer ${c}`;
  // Every write names the machine it is meant for. If this address has
  // come to mean another Mac, the server refuses rather than obeys.
  const m = savedMachine();
  if (method === "POST" && m) headers["x-threshold-machine"] = m.id;
  if (body !== undefined) headers["content-type"] = "application/json";
  let res: Response;
  const send = () => fetch(path, { method, headers, body: body === undefined ? undefined : JSON.stringify(body), cache: "no-store" });
  try {
    res = await send();
  } catch {
    // A phone waking from sleep often has no network for a moment. One
    // retry, shortly after, tells that blip from the Mac being away.
    await new Promise((r) => setTimeout(r, 800));
    try {
      res = await send();
    } catch {
      throw new ApiError(0, "unreachable");
    }
  }
  if (!res.ok) {
    let message = res.statusText;
    try { message = (await res.json()).error ?? message; } catch { /* bare 401 */ }
    throw new ApiError(res.status, message);
  }
  return res.json() as Promise<T>;
}

export const machine = () => call<Machine>("GET", "/api/machine");

export async function pair(token: string, name: string): Promise<Machine> {
  const r = await call<{ credential: string; machine: Machine }>("POST", "/api/pair", { token, name });
  write(CREDENTIAL, r.credential);
  write(MACHINE, JSON.stringify(r.machine));
  return r.machine;
}

export function remember(m: Machine): void {
  write(MACHINE, JSON.stringify(m));
}

const enc = encodeURIComponent;
export const reviews = () => call<ReviewRow[]>("GET", "/api/reviews");
export const events = (name: string) => call<SessionEvent[]>("GET", `/api/reviews/${enc(name)}`);
export const diff = (name: string) => call<DiffPayload>("GET", `/api/reviews/${enc(name)}/diff`);
export const bundle = (name: string) => call<BundlePayload | null>("GET", `/api/reviews/${enc(name)}/bundle`);
export const chapters = (name: string) => call<ChapterRow[]>("GET", `/api/reviews/${enc(name)}/chapters`);
export const anchors = (name: string) => call<LineAnchor[]>("GET", `/api/reviews/${enc(name)}/anchors`);

export interface NoteAt { file?: string; line?: number; line_content?: string; chapter?: string; in_reply_to?: number }
export const note = (name: string, text: string, at: NoteAt = {}) =>
  call<SessionEvent[]>("POST", `/api/reviews/${enc(name)}/notes`, { text, ...at });
export const mark = (name: string, chapter: string, m: Mark) =>
  call<SessionEvent[]>("POST", `/api/reviews/${enc(name)}/marks`, { chapter, mark: m });
export const reject = (name: string, reason: string, digest: string) =>
  call<SessionEvent[]>("POST", `/api/reviews/${enc(name)}/verdict`, { verdict: "rejected", reason, digest });
export const approve = (name: string, note: string | null, digest: string) =>
  call<SessionEvent[]>("POST", `/api/reviews/${enc(name)}/verdict`, { verdict: "approved", reason: note, digest });

export type Pushed = { type: "inbox-changed" } | { type: "session-changed"; review: string };

/** Listen for changes until `signal` aborts, reconnecting with backoff.
 *
 *  A fetch stream rather than EventSource, which cannot send the
 *  credential header. `onState` hears whether the Mac is answering, so the
 *  screen can say so instead of showing stale data as if it were live. */
/** Whether the Mac answers at all, within a few seconds. */
export async function reachable(): Promise<boolean> {
  const ctl = new AbortController();
  const t = setTimeout(() => ctl.abort(), 4000);
  try {
    return (await fetch("/api/machine", { cache: "no-store", signal: ctl.signal })).ok;
  } catch {
    return false;
  } finally {
    clearTimeout(t);
  }
}

/** Longest the stream may go silent. The Mac sends a keep-alive every 15
 *  seconds, so this long without one means the connection died without
 *  saying so, which is what a phone going to sleep does to it. */
const SILENT_MS = 35_000;

export async function listen(onEvent: (p: Pushed) => void, onState: (live: boolean) => void, signal: AbortSignal): Promise<void> {
  let delay = 1000;
  while (!signal.aborted) {
    // One connection's lifetime: aborted with the whole listener, or by
    // the watchdog when the stream has gone quiet.
    const conn = new AbortController();
    const stop = () => conn.abort();
    signal.addEventListener("abort", stop);
    let watchdog = setTimeout(stop, SILENT_MS);
    const pet = () => { clearTimeout(watchdog); watchdog = setTimeout(stop, SILENT_MS); };
    try {
      const res = await fetch("/api/events", { headers: { authorization: `Bearer ${credential() ?? ""}` }, signal: conn.signal, cache: "no-store" });
      // Answering, and refusing this phone: reachable, and not paired.
      if (res.status === 401) { onState(true); throw new ApiError(401, "unpaired"); }
      if (!res.ok || !res.body) throw new ApiError(res.status, res.statusText);
      onState(true);
      delay = 1000;
      // Anything missed while disconnected is unknowable, so reconnecting
      // counts as everything having changed.
      onEvent({ type: "inbox-changed" });
      const reader = res.body.pipeThrough(new TextDecoderStream()).getReader();
      let buf = "";
      for (;;) {
        const { value, done } = await reader.read();
        if (done) break;
        pet();
        buf += value;
        let i;
        while ((i = buf.indexOf("\n\n")) >= 0) {
          const block = buf.slice(0, i);
          buf = buf.slice(i + 2);
          const data = block.split("\n").filter((l) => l.startsWith("data:")).map((l) => l.slice(5).trim()).join("");
          if (data) { try { onEvent(JSON.parse(data)); } catch { /* keep-alive or garbage */ } }
        }
      }
    } catch (e) {
      if (signal.aborted) return;
      if (e instanceof ApiError && e.status === 401) throw e;
    } finally {
      clearTimeout(watchdog);
      signal.removeEventListener("abort", stop);
    }
    if (signal.aborted) return;
    // A dropped stream is not the Mac being away. Only say so when it
    // does not answer when asked directly; otherwise reconnect at once.
    if (await reachable()) {
      delay = 1000;
      continue;
    }
    onState(false);
    await new Promise((r) => setTimeout(r, delay));
    delay = Math.min(delay * 2, 5_000);
  }
}

export interface PushState { key: string; subscribed: boolean }
export const pushKey = () => call<PushState>("GET", "/api/push/key");
/** A subscription as PushSubscription.toJSON() gives it: where to push,
 *  and the keys the Mac encrypts what it says to. */
export const pushSubscribe = (sub: PushSubscriptionJSON) =>
  call<PushState>("POST", "/api/push/subscribe", { endpoint: sub.endpoint, keys: sub.keys });
export const pushUnsubscribe = () => call<PushState>("POST", "/api/push/unsubscribe", {});

/** A file's new side, from the review's commits. */
export const fileText = (name: string, path: string) =>
  call<string[]>("GET", `/api/reviews/${enc(name)}/file?path=${enc(path)}`);
/** An image from one side of a review, as a data: URL. */
export const fileImage = (name: string, path: string, old: boolean) =>
  call<string>("GET", `/api/reviews/${enc(name)}/image?path=${enc(path)}&old=${old}`);

export interface Message { seq: number; from: "agent" | "human" | "threshold"; text: string; at_ms: number }
export interface ThreadCard {
  review: string;
  title: string;
  project: string | null;
  root: number;
  file: string | null;
  line: number | null;
  code: string | null;
  chapter: string | null;
  flag: "attention" | "caution" | "context" | null;
  messages: Message[];
}
export interface FreshReview { review: string; title: string; project: string | null; summary: string; flags: number; at_ms: number }
export const threads = () => call<{ threads: ThreadCard[]; fresh: FreshReview[] }>("GET", "/api/threads");
