//! Web Push to paired phones: "Claude is waiting on you", and nothing
//! else.
//!
//! **Content-free, so payload-free.** A push carries no data at all: no
//! title, no repo, not even which review. The phone's service worker shows
//! a fixed notification naming this machine, and a tap opens the inbox.
//! With no payload there is nothing to encrypt (RFC 8291 does not apply),
//! and the push service learns only that a push happened. All it takes is
//! a VAPID signature (RFC 8292) on an empty POST.
//!
//! **When.** A new review, a reply from Claude, a revision: something the
//! agent said that the reviewer is waiting on. Never Threshold's own
//! bookkeeping (pins, diagnostics), never the reviewer's own notes or
//! marks. Once per review per burst: a review is announced after its log
//! has sat still for a few seconds, and what has been announced is
//! remembered on disk, so a restart does not announce it again.

use p256::ecdsa::{signature::Signer, Signature, SigningKey};
use p256::elliptic_curve::sec1::ToEncodedPoint;
use p256::pkcs8::{DecodePrivateKey, EncodePrivateKey};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use threshold_core::{Author, Body, Event};

use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64URL;
use base64::Engine;

/// One phone's push subscription. The endpoint is a capability URL at the
/// phone's push service; whoever holds it can wake the phone, so it is
/// kept beside the device list and dropped when the device is revoked.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Subscription {
    pub device: String,
    pub endpoint: String,
    /// The phone's ECDH public key (`p256dh`) and auth secret, base64url.
    /// Needed to encrypt a payload to it; a subscription saved before
    /// payloads existed has neither and gets an empty push.
    #[serde(default)]
    pub p256dh: Option<String>,
    #[serde(default)]
    pub auth: Option<String>,
}

/// What a push says: carried encrypted end to end (RFC 8291), so the push
/// service relays it without being able to read it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Notice {
    /// The review's title.
    pub title: String,
    /// Who spoke and the start of what they said.
    pub body: String,
    /// The review file, for opening it from the notification.
    pub review: String,
    /// The thread Claude spoke in, by its root's seq, so the notification
    /// opens on the conversation rather than the brief. None for a new
    /// review or a revision.
    pub thread: Option<u64>,
}

/// The first message of the thread `seq` belongs to, following replies up
/// the chain: Claude often answers the latest message, not the first.
fn root_of(events: &[Event], seq: u64) -> Option<u64> {
    let parent = |s: u64| {
        events.iter().find(|e| e.seq == s).and_then(|e| match &e.body {
            Body::Note { in_reply_to, .. } | Body::Reply { in_reply_to, .. } => Some(*in_reply_to),
            _ => None,
        })
    };
    let mut cur = seq;
    // Bounded by the log: a reply always points back, never round.
    for _ in 0..events.len() {
        match parent(cur)? {
            Some(p) => cur = p,
            None => return Some(cur),
        }
    }
    None
}

/// Encrypt `plaintext` for a subscription (RFC 8291, aes128gcm per RFC
/// 8188), with the sender's ephemeral key and salt given, so the RFC's
/// own example can check it. One record: the payload, then the 0x02
/// delimiter that marks the last record, with no padding.
pub fn encrypt_with(plaintext: &[u8], ua_public: &[u8], auth_secret: &[u8], as_secret: &p256::SecretKey, salt: &[u8; 16]) -> Result<Vec<u8>, String> {
    use aes_gcm::aead::{Aead, KeyInit};
    use hkdf::Hkdf;
    use sha2::Sha256;
    let ua = p256::PublicKey::from_sec1_bytes(ua_public).map_err(|_| "not a P-256 public key")?;
    let as_public = as_secret.public_key().to_encoded_point(false);
    let shared = p256::ecdh::diffie_hellman(as_secret.to_nonzero_scalar(), ua.as_affine());

    // IKM = HKDF(auth_secret, ecdh_secret, "WebPush: info" 0x00 ua_public as_public)
    let mut key_info = b"WebPush: info\0".to_vec();
    key_info.extend_from_slice(ua_public);
    key_info.extend_from_slice(as_public.as_bytes());
    let mut ikm = [0u8; 32];
    Hkdf::<Sha256>::new(Some(auth_secret), shared.raw_secret_bytes()).expand(&key_info, &mut ikm).map_err(|e| e.to_string())?;

    let prk = Hkdf::<Sha256>::new(Some(salt), &ikm);
    let mut cek = [0u8; 16];
    let mut nonce = [0u8; 12];
    prk.expand(b"Content-Encoding: aes128gcm\0", &mut cek).map_err(|e| e.to_string())?;
    prk.expand(b"Content-Encoding: nonce\0", &mut nonce).map_err(|e| e.to_string())?;

    let mut record = plaintext.to_vec();
    record.push(0x02);
    let cipher = aes_gcm::Aes128Gcm::new_from_slice(&cek).map_err(|e| e.to_string())?;
    let sealed = cipher.encrypt(aes_gcm::Nonce::from_slice(&nonce), record.as_ref()).map_err(|e| e.to_string())?;

    // Header: salt, record size, key id length, key id (the sender's key).
    let mut out = salt.to_vec();
    out.extend_from_slice(&4096u32.to_be_bytes());
    out.push(as_public.as_bytes().len() as u8);
    out.extend_from_slice(as_public.as_bytes());
    out.extend_from_slice(&sealed);
    Ok(out)
}

/// Encrypt for a subscription with a fresh key and salt, as every real
/// push must be.
pub fn encrypt(plaintext: &[u8], ua_public: &[u8], auth_secret: &[u8]) -> Result<Vec<u8>, String> {
    let mut seed = [0u8; 32];
    let secret = loop {
        getrandom::fill(&mut seed).map_err(|e| e.to_string())?;
        if let Ok(k) = p256::SecretKey::from_slice(&seed) {
            break k;
        }
    };
    let mut salt = [0u8; 16];
    getrandom::fill(&mut salt).map_err(|e| e.to_string())?;
    encrypt_with(plaintext, ua_public, auth_secret, &secret, &salt)
}

/// The start of `text` on one line, cut at a word near `max` characters.
pub fn excerpt(text: &str, max: usize) -> String {
    let flat: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= max {
        return flat;
    }
    let cut: String = flat.chars().take(max).collect();
    let at = cut.rfind(' ').filter(|&i| i > max / 2).unwrap_or(cut.len());
    format!("{}…", cut[..at].trim_end_matches([',', '.', ';', ':']))
}

#[derive(Serialize, Deserialize)]
struct Keys {
    /// PKCS#8 DER, base64url.
    private: String,
}

pub struct Push {
    dir: PathBuf,
    key: SigningKey,
    lock: Mutex<()>,
}

fn write_atomically(path: &Path, text: &str) -> std::io::Result<()> {
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, text)?;
    std::fs::rename(tmp, path)
}

impl Push {
    /// Load the VAPID key from `dir`, making one the first time.
    pub fn load(dir: &Path) -> Self {
        let path = dir.join("vapid.json");
        let existing = std::fs::read_to_string(&path)
            .ok()
            .and_then(|t| serde_json::from_str::<Keys>(&t).ok())
            .and_then(|k| B64URL.decode(k.private).ok())
            .and_then(|der| SigningKey::from_pkcs8_der(&der).ok());
        let key = existing.unwrap_or_else(|| {
            let mut seed = [0u8; 32];
            // A zero or out-of-range scalar is astronomically unlikely;
            // try again rather than reason about it.
            loop {
                getrandom::fill(&mut seed).expect("the OS random source is unavailable");
                if let Ok(k) = SigningKey::from_slice(&seed) {
                    break k;
                }
            }
        });
        if let Ok(der) = key.to_pkcs8_der() {
            let keys = Keys { private: B64URL.encode(der.as_bytes()) };
            let _ = write_atomically(&path, &serde_json::to_string_pretty(&keys).unwrap_or_default());
        }
        Push { dir: dir.to_path_buf(), key, lock: Mutex::new(()) }
    }

    /// The application server key a phone subscribes with: the public key,
    /// uncompressed, base64url.
    pub fn public_key(&self) -> String {
        B64URL.encode(self.key.verifying_key().to_encoded_point(false).as_bytes())
    }

    fn subs_path(&self) -> PathBuf {
        self.dir.join("push-subscriptions.json")
    }

    pub fn subscriptions(&self) -> Vec<Subscription> {
        std::fs::read_to_string(self.subs_path()).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
    }

    fn save(&self, subs: &[Subscription]) -> std::io::Result<()> {
        write_atomically(&self.subs_path(), &serde_json::to_string_pretty(subs).unwrap_or_default())
    }

    /// Record a device's subscription, replacing any it had: a phone has
    /// one push endpoint at a time.
    pub fn subscribe(&self, device: &str, endpoint: &str, p256dh: Option<&str>, auth: Option<&str>) -> Result<(), String> {
        if !endpoint.starts_with("https://") || endpoint.len() > 2048 {
            return Err("not a push endpoint".into());
        }
        let key = |v: Option<&str>, len: usize| -> Result<Option<String>, String> {
            match v {
                None => Ok(None),
                Some(k) if B64URL.decode(k.trim_end_matches('=')).map(|b| b.len() == len).unwrap_or(false) => Ok(Some(k.trim_end_matches('=').to_string())),
                Some(_) => Err("not a subscription key".into()),
            }
        };
        let (p256dh, auth) = (key(p256dh, 65)?, key(auth, 16)?);
        let _g = self.lock.lock().unwrap();
        let mut subs = self.subscriptions();
        subs.retain(|s| s.device != device && s.endpoint != endpoint);
        subs.push(Subscription { device: device.into(), endpoint: endpoint.into(), p256dh, auth });
        self.save(&subs).map_err(|e| e.to_string())
    }

    pub fn unsubscribe_device(&self, device: &str) {
        let _g = self.lock.lock().unwrap();
        let mut subs = self.subscriptions();
        let before = subs.len();
        subs.retain(|s| s.device != device);
        if subs.len() != before {
            let _ = self.save(&subs);
        }
    }

    fn drop_endpoint(&self, endpoint: &str) {
        let _g = self.lock.lock().unwrap();
        let mut subs = self.subscriptions();
        subs.retain(|s| s.endpoint != endpoint);
        let _ = self.save(&subs);
    }

    /// The VAPID `Authorization` header for a push to `endpoint`.
    pub fn authorization(&self, endpoint: &str, subject: &str, now_s: u64) -> Result<String, String> {
        let url = reqwest::Url::parse(endpoint).map_err(|e| e.to_string())?;
        let aud = url.origin().ascii_serialization();
        let header = B64URL.encode(br#"{"typ":"JWT","alg":"ES256"}"#);
        // Twelve hours: the longest a push service accepts is 24.
        let claims = serde_json::json!({ "aud": aud, "exp": now_s + 12 * 3600, "sub": subject });
        let body = B64URL.encode(claims.to_string());
        let signing_input = format!("{header}.{body}");
        let sig: Signature = self.key.sign(signing_input.as_bytes());
        Ok(format!("vapid t={signing_input}.{}, k={}", B64URL.encode(sig.to_bytes()), self.public_key()))
    }

    /// Tell every subscribed phone. The notice is encrypted to each phone;
    /// a subscription saved without keys gets an empty push, which the
    /// phone shows as a generic notification. Endpoints the push service
    /// says are gone (404, 410) are dropped. Returns how many were
    /// delivered.
    pub async fn send_all(&self, subject: &str, notice: &Notice) -> usize {
        let client = reqwest::Client::new();
        let now = threshold_core::now_ms() / 1000;
        let json = serde_json::to_vec(notice).unwrap_or_default();
        let mut delivered = 0;
        for sub in self.subscriptions() {
            let Ok(auth) = self.authorization(&sub.endpoint, subject, now) else { continue };
            let body = match (&sub.p256dh, &sub.auth) {
                (Some(k), Some(a)) => match (B64URL.decode(k), B64URL.decode(a)) {
                    (Ok(k), Ok(a)) => encrypt(&json, &k, &a).ok(),
                    _ => None,
                },
                _ => None,
            };
            let mut req = client.post(&sub.endpoint).header("Authorization", auth).header("TTL", "3600").header("Urgency", "high");
            req = match body {
                Some(b) => req.header("Content-Encoding", "aes128gcm").header("Content-Type", "application/octet-stream").body(b),
                None => req.header("Content-Length", "0"),
            };
            let res = req.send().await;
            match res {
                Ok(r) if r.status().is_success() => delivered += 1,
                Ok(r) if r.status() == 404 || r.status() == 410 => self.drop_endpoint(&sub.endpoint),
                Ok(r) => eprintln!("threshold: push to {} refused: {}", r.url().host_str().unwrap_or("?"), r.status()),
                Err(e) => eprintln!("threshold: push failed: {e}"),
            }
        }
        delivered
    }
}

/// Whether `e` is the agent saying something the reviewer is waiting on.
pub fn worth_announcing(e: &Event) -> bool {
    e.from == Author::Agent && matches!(e.body, Body::Brief { .. } | Body::Reply { .. } | Body::Revised { .. } | Body::Note { .. })
}

/// What has been announced, by review file name: the highest seq covered.
/// Kept on disk so a restart does not announce the same thing again.
pub struct Announced {
    path: PathBuf,
    seen: HashMap<String, u64>,
}

impl Announced {
    /// Loaded from `dir`. The first time ever, everything already in the
    /// inbox counts as announced: turning notifications on must not
    /// deliver a history lesson.
    pub fn load(dir: &Path, inbox: &Path) -> Self {
        let path = dir.join("push-announced.json");
        let seen = match std::fs::read_to_string(&path).ok().and_then(|t| serde_json::from_str(&t).ok()) {
            Some(s) => s,
            None => {
                let mut s = HashMap::new();
                for r in threshold_core::list_reviews(inbox) {
                    if let Some(name) = r.path.file_name() {
                        s.insert(name.to_string_lossy().into_owned(), threshold_core::last_seq(&r.path));
                    }
                }
                s
            }
        };
        let a = Announced { path, seen };
        a.save();
        a
    }

    fn save(&self) {
        let _ = write_atomically(&self.path, &serde_json::to_string(&self.seen).unwrap_or_default());
    }

    /// Reviews with something new worth announcing, marking them
    /// announced. Only logs that have sat still for `settle`: an agent
    /// pushes a brief, a bundle and its flags as a burst, which is one
    /// announcement, not five.
    pub fn due(&mut self, inbox: &Path, settle: std::time::Duration) -> Vec<Notice> {
        let mut due = Vec::new();
        for r in threshold_core::list_reviews(inbox) {
            if r.decided {
                continue;
            }
            let Some(name) = r.path.file_name().map(|n| n.to_string_lossy().into_owned()) else { continue };
            let last = threshold_core::last_seq(&r.path);
            let seen = self.seen.get(&name).copied().unwrap_or(0);
            if last <= seen {
                continue;
            }
            let settled = std::fs::metadata(&r.path).and_then(|m| m.modified()).map(|m| m.elapsed().unwrap_or_default() >= settle).unwrap_or(true);
            if !settled {
                continue;
            }
            let events = threshold_core::read_session(&r.path);
            // The newest thing Claude said since last time is what the
            // notification quotes.
            if let Some(e) = events.iter().filter(|e| e.seq > seen && worth_announcing(e)).last() {
                let body = match &e.body {
                    Body::Brief { title, summary, .. } => format!("New review: {}", excerpt(if summary.is_empty() { title } else { summary }, 140)),
                    Body::Revised { note, to } => format!("Revised: {}", excerpt(note.as_deref().unwrap_or(to), 140)),
                    Body::Note { text, .. } | Body::Reply { text, .. } => format!("Claude: {}", excerpt(text, 140)),
                    _ => "Claude is waiting on you.".into(),
                };
                let thread = match &e.body {
                    Body::Note { .. } | Body::Reply { .. } => root_of(&events, e.seq),
                    _ => None,
                };
                due.push(Notice { title: r.title.clone(), body, review: name.clone(), thread });
            }
            self.seen.insert(name, last);
        }
        self.save();
        due
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use p256::ecdsa::{signature::Verifier, VerifyingKey};
    use std::time::Duration;

    /// Google's push service, sent a push for a subscription that does
    /// not exist, answers 401/403 if it rejects the VAPID signature and
    /// 404/410 if it accepts it and only the subscription is unknown.
    ///
    /// `cargo test -p threshold-desktop a_real_push_service -- --ignored --nocapture`
    #[tokio::test]
    #[ignore]
    async fn a_real_push_service_accepts_the_signature() {
        let dir = tempfile::tempdir().unwrap();
        let push = Push::load(dir.path());
        let endpoint = "https://fcm.googleapis.com/fcm/send/not-a-real-subscription-0000";
        let auth = push.authorization(endpoint, "mailto:threshold@localhost", threshold_core::now_ms() / 1000).unwrap();
        let res = reqwest::Client::new()
            .post(endpoint)
            .header("Authorization", auth)
            .header("TTL", "60")
            .header("Content-Length", "0")
            .send()
            .await
            .unwrap();
        let status = res.status().as_u16();
        println!("status {status}: {}", res.text().await.unwrap_or_default().chars().take(300).collect::<String>());
        assert!(![401, 403].contains(&status), "the push service rejected the VAPID signature");
    }

    fn b64(s: &str) -> Vec<u8> {
        B64URL.decode(s).unwrap()
    }

    #[test]
    fn a_payload_is_encrypted_exactly_as_rfc_8291_appendix_a() {
        // The RFC's worked example, byte for byte.
        let as_secret = p256::SecretKey::from_slice(&b64("yfWPiYE-n46HLnH0KqZOF1fJJU3MYrct3AELtAQ-oRw")).unwrap();
        assert_eq!(
            B64URL.encode(as_secret.public_key().to_encoded_point(false).as_bytes()),
            "BP4z9KsN6nGRTbVYI_c7VJSPQTBtkgcy27mlmlMoZIIgDll6e3vCYLocInmYWAmS6TlzAC8wEqKK6PBru3jl7A8"
        );
        let ua_public = b64("BCVxsr7N_eNgVRqvHtD0zTZsEc6-VV-JvLexhqUzORcxaOzi6-AYWXvTBHm4bjyPjs7Vd8pZGH6SRpkNtoIAiw4");
        let auth = b64("BTBZMqHH6r4Tts7J_aSIgg");
        let salt: [u8; 16] = b64("DGv6ra1nlYgDCS1FRnbzlw").try_into().unwrap();
        let body = encrypt_with(b"When I grow up, I want to be a watermelon", &ua_public, &auth, &as_secret, &salt).unwrap();
        assert_eq!(
            B64URL.encode(body),
            "DGv6ra1nlYgDCS1FRnbzlwAAEABBBP4z9KsN6nGRTbVYI_c7VJSPQTBtkgcy27mlmlMoZIIgDll6e3vCYLocInmYWAmS6TlzAC8wEqKK6PBru3jl7A_yl95bQpu6cVPTpK4Mqgkf1CXztLVBSt2Ks3oZwbuwXPXLWyouBWLVWGNWQexSgSxsj_Qulcy4a-fN"
        );
    }

    #[test]
    fn an_excerpt_is_one_line_cut_at_a_word() {
        assert_eq!(excerpt("short\n\n  text", 140), "short text");
        let long = "Yes, id is submissions.id. It doesn't write the outbox row itself. enqueue_sync_mailchimp only queues an HTTP POST";
        let e = excerpt(long, 60);
        assert!(e.ends_with('…') && e.chars().count() <= 61, "{e}");
        assert!(e.starts_with("Yes, id is submissions.id. It doesn't write the"), "{e}");
    }

    #[test]
    fn the_key_is_made_once_and_kept() {
        let dir = tempfile::tempdir().unwrap();
        let a = Push::load(dir.path()).public_key();
        assert_eq!(Push::load(dir.path()).public_key(), a, "phones subscribed with this key; a new one strands them");
        assert_eq!(B64URL.decode(&a).unwrap().len(), 65, "uncompressed P-256 point");
    }

    #[test]
    fn the_vapid_header_is_a_jwt_the_public_key_verifies() {
        let dir = tempfile::tempdir().unwrap();
        let push = Push::load(dir.path());
        let h = push.authorization("https://fcm.googleapis.com/fcm/send/abc", "mailto:t@example.com", 1_000).unwrap();
        let (t, k) = h.strip_prefix("vapid t=").unwrap().split_once(", k=").unwrap();
        assert_eq!(k, push.public_key());
        let parts: Vec<&str> = t.split('.').collect();
        assert_eq!(parts.len(), 3);
        let claims: serde_json::Value = serde_json::from_slice(&B64URL.decode(parts[1]).unwrap()).unwrap();
        assert_eq!(claims["aud"], "https://fcm.googleapis.com", "the audience is the push service's origin, not the endpoint");
        assert_eq!(claims["exp"], 1_000 + 12 * 3600);
        let key = VerifyingKey::from_sec1_bytes(&B64URL.decode(k).unwrap()).unwrap();
        let sig = Signature::from_slice(&B64URL.decode(parts[2]).unwrap()).unwrap();
        key.verify(format!("{}.{}", parts[0], parts[1]).as_bytes(), &sig).unwrap();
    }

    #[test]
    fn a_device_has_one_subscription_and_loses_it_when_revoked() {
        let dir = tempfile::tempdir().unwrap();
        let push = Push::load(dir.path());
        push.subscribe("d1", "https://push.example/a", None, None).unwrap();
        push.subscribe("d1", "https://push.example/b", None, None).unwrap();
        push.subscribe("d2", "https://push.example/c", None, None).unwrap();
        assert_eq!(push.subscriptions().len(), 2);
        assert!(push.subscribe("d3", "http://insecure.example/x", None, None).is_err());
        assert!(push.subscribe("d3", "https://push.example/d", Some("short"), Some("x")).is_err(), "keys are checked");
        push.unsubscribe_device("d1");
        assert_eq!(push.subscriptions(), vec![Subscription { device: "d2".into(), endpoint: "https://push.example/c".into(), p256dh: None, auth: None }]);
    }

    fn brief() -> Body {
        Body::Brief { title: "t".into(), summary: "s".into(), implications: vec![], repo: Some("/r".into()) }
    }

    #[test]
    fn what_was_already_in_the_inbox_is_not_announced() {
        let inbox = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        threshold_core::append_event(&inbox.path().join("old.jsonl"), Author::Agent, None, brief()).unwrap();
        let mut a = Announced::load(data.path(), inbox.path());
        assert_eq!(a.due(inbox.path(), Duration::ZERO).len(), 0);
    }

    #[test]
    fn claude_speaking_is_announced_once_and_nothing_else_is() {
        let inbox = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        let mut a = Announced::load(data.path(), inbox.path());
        let p = inbox.path().join("new.jsonl");
        threshold_core::append_event(&p, Author::Agent, None, brief()).unwrap();
        let n = a.due(inbox.path(), Duration::ZERO);
        assert_eq!(n.len(), 1, "a new review");
        assert_eq!((n[0].title.as_str(), n[0].body.as_str(), n[0].review.as_str()), ("t", "New review: s", "new.jsonl"));
        assert_eq!(a.due(inbox.path(), Duration::ZERO).len(), 0, "and only once");

        threshold_core::diagnose(&p, None, "review_unpinned", "m".into(), vec![]).unwrap();
        let note = Body::Note { text: "why?".into(), file: None, line: None, line_content: None, chapter: None, in_reply_to: None, flag: None };
        threshold_core::append_event(&p, Author::Human, None, note).unwrap();
        assert_eq!(a.due(inbox.path(), Duration::ZERO).len(), 0, "Threshold's bookkeeping and the reviewer's own words are not news");

        threshold_core::append_event(&p, Author::Agent, None, Body::Reply { text: "because".into(), in_reply_to: Some(3) }).unwrap();
        let n = Announced::load(data.path(), inbox.path()).due(inbox.path(), Duration::ZERO);
        assert_eq!(n.len(), 1, "a reply is, and what was announced survives a restart");
        assert_eq!(n[0].body, "Claude: because", "quoting what Claude said");
        assert_eq!(n[0].thread, Some(3), "and opening on that thread");

        let more = Body::Note { text: "and?".into(), file: None, line: None, line_content: None, chapter: None, in_reply_to: Some(4), flag: None };
        threshold_core::append_event(&p, Author::Human, None, more).unwrap();
        threshold_core::append_event(&p, Author::Agent, None, Body::Reply { text: "and this".into(), in_reply_to: Some(5) }).unwrap();
        let n = Announced::load(data.path(), inbox.path()).due(inbox.path(), Duration::ZERO);
        assert_eq!(n[0].thread, Some(3), "an answer to a reply is still that thread");
    }

    #[test]
    fn a_review_still_being_written_waits() {
        let inbox = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        let mut a = Announced::load(data.path(), inbox.path());
        threshold_core::append_event(&inbox.path().join("new.jsonl"), Author::Agent, None, brief()).unwrap();
        assert_eq!(a.due(inbox.path(), Duration::from_secs(3600)).len(), 0);
        assert_eq!(a.due(inbox.path(), Duration::ZERO).len(), 1, "announced once it settles, not lost");
    }
}
