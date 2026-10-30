//! Phone access: which machine this is, which phones may talk to it, and
//! how a phone gets permission.
//!
//! Kept beside the decision log in app data, never in the session logs:
//! those are the conversation between agent and human, and a device
//! credential is neither.
//!
//! **Which machine.** Threshold may run on several Macs, each driving a
//! different org's agents, and one phone may drive them all. The phone
//! must never be unsure which one it is talking to, so a machine carries a
//! name and a colour the person chose, shown on every phone screen, and a
//! random id every write must quote back. A request carrying another
//! machine's id is refused rather than applied to the wrong Mac.
//!
//! **Which phones.** Pairing is a bootstrap, not a secret. The QR carries
//! a one-time token that expires in a minute; presenting it yields a
//! device credential the phone keeps. Only a hash of the credential is
//! stored, so the file on disk cannot be replayed, and revoking a device
//! deletes its hash.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Where the app listens unless configured otherwise. Loopback only:
/// `tailscale serve` is what puts it on the tailnet, with HTTPS.
pub const DEFAULT_PORT: u16 = 7457;

/// How long a pairing QR stays good.
pub const PAIRING_TTL: Duration = Duration::from_secs(60);

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Machine {
    /// Random, generated once. Quoted on every write from a phone.
    pub id: String,
    /// What the person calls this Mac, and its tailnet host name.
    pub name: String,
    /// A CSS colour, e.g. `#2f6fdf`.
    pub color: String,
}

/// `mobile.json`: written by `scripts/threshold-mobile-setup.sh`, read at
/// launch.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Config {
    /// Off unless setup has been run. Nothing listens otherwise.
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub port: Option<u16>,
    #[serde(default)]
    pub machine: Option<Machine>,
    /// The HTTPS address a phone reaches this machine at, from
    /// `tailscale serve`. What the pairing QR points to.
    #[serde(default)]
    pub url: Option<String>,
}

impl Config {
    pub fn port(&self) -> u16 {
        self.port.unwrap_or(DEFAULT_PORT)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Device {
    pub id: String,
    /// What the phone called itself when it paired.
    pub name: String,
    pub paired_at_ms: u64,
    pub last_seen_ms: u64,
    /// SHA-256 of the credential, hex. The credential itself is only ever
    /// on the phone.
    credential_sha256: String,
}

/// A paired phone as shown to the person: everything but the hash.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct DeviceView {
    pub id: String,
    pub name: String,
    pub paired_at_ms: u64,
    pub last_seen_ms: u64,
}

pub struct Mobile {
    dir: PathBuf,
    config: Config,
    /// Hashes of pairing tokens not yet used, with when each expires.
    /// In memory only: a restart invalidates every QR on screen, which is
    /// the right direction to fail in.
    pairing: Mutex<Vec<(String, Instant)>>,
    devices: Mutex<()>,
}

pub fn random_hex(bytes: usize) -> String {
    let mut buf = vec![0u8; bytes];
    getrandom::fill(&mut buf).expect("the OS random source is unavailable");
    hex(&buf)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn sha256_hex(s: &str) -> String {
    hex(&Sha256::digest(s.as_bytes()))
}

/// Compare without stopping at the first difference, so response timing
/// says nothing about how much of a guess was right.
fn same(a: &str, b: &str) -> bool {
    a.len() == b.len() && a.bytes().zip(b.bytes()).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

fn write_atomically(path: &Path, text: &str) -> std::io::Result<()> {
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, text)?;
    std::fs::rename(tmp, path)
}

impl Mobile {
    /// Load from `dir` (the app data directory), giving the machine an id
    /// the first time it has a name.
    pub fn load(dir: &Path) -> Self {
        let _ = std::fs::create_dir_all(dir);
        let path = dir.join("mobile.json");
        let mut config: Config = std::fs::read_to_string(&path).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
        if let Some(m) = config.machine.as_mut() {
            if m.id.is_empty() {
                m.id = random_hex(16);
                let _ = write_atomically(&path, &serde_json::to_string_pretty(&config).unwrap_or_default());
            }
        }
        Mobile { dir: dir.to_path_buf(), config, pairing: Mutex::new(Vec::new()), devices: Mutex::new(()) }
    }

    pub fn config(&self) -> &Config {
        &self.config
    }

    /// Serve only once the machine has been named. A phone that cannot be
    /// told which Mac it is on is the thing this module exists to prevent.
    pub fn machine(&self) -> Option<&Machine> {
        self.config.machine.as_ref().filter(|m| !m.name.trim().is_empty() && !m.id.is_empty())
    }

    fn devices_path(&self) -> PathBuf {
        self.dir.join("devices.json")
    }

    fn read_devices(&self) -> Vec<Device> {
        std::fs::read_to_string(self.devices_path()).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
    }

    fn write_devices(&self, devices: &[Device]) -> std::io::Result<()> {
        write_atomically(&self.devices_path(), &serde_json::to_string_pretty(devices).unwrap_or_default())
    }

    pub fn devices(&self) -> Vec<DeviceView> {
        let _g = self.devices.lock().unwrap();
        self.read_devices()
            .into_iter()
            .map(|d| DeviceView { id: d.id, name: d.name, paired_at_ms: d.paired_at_ms, last_seen_ms: d.last_seen_ms })
            .collect()
    }

    /// A fresh one-time pairing token, for a QR code.
    pub fn new_pairing_token(&self) -> String {
        self.new_pairing_token_at(Instant::now())
    }

    fn new_pairing_token_at(&self, now: Instant) -> String {
        let token = random_hex(32);
        let mut p = self.pairing.lock().unwrap();
        p.retain(|(_, exp)| *exp > now);
        p.push((sha256_hex(&token), now + PAIRING_TTL));
        token
    }

    /// Trade a pairing token for a device credential. The token is spent
    /// whether or not anything else goes wrong.
    pub fn pair(&self, token: &str, name: &str) -> Option<(DeviceView, String)> {
        self.pair_at(token, name, Instant::now())
    }

    fn pair_at(&self, token: &str, name: &str, now: Instant) -> Option<(DeviceView, String)> {
        let hash = sha256_hex(token);
        {
            let mut p = self.pairing.lock().unwrap();
            let i = p.iter().position(|(h, _)| same(h, &hash))?;
            let (_, expires) = p.remove(i);
            if expires <= now {
                return None;
            }
        }
        let credential = random_hex(32);
        let at = threshold_core::now_ms();
        let name: String = name.trim().chars().take(60).collect();
        let device = Device {
            id: random_hex(8),
            name: if name.is_empty() { "phone".into() } else { name },
            paired_at_ms: at,
            last_seen_ms: at,
            credential_sha256: sha256_hex(&credential),
        };
        let _g = self.devices.lock().unwrap();
        let mut all = self.read_devices();
        all.push(device.clone());
        self.write_devices(&all).ok()?;
        let view = DeviceView { id: device.id, name: device.name, paired_at_ms: at, last_seen_ms: at };
        Some((view, credential))
    }

    /// The device a credential belongs to, noting that it was seen.
    pub fn authenticate(&self, credential: &str) -> Option<String> {
        let hash = sha256_hex(credential);
        let _g = self.devices.lock().unwrap();
        let mut all = self.read_devices();
        let d = all.iter_mut().find(|d| same(&d.credential_sha256, &hash))?;
        let id = d.id.clone();
        let now = threshold_core::now_ms();
        // Written at most once a minute per device: last-seen is for a
        // person glancing at a list, not an audit trail.
        if now.saturating_sub(d.last_seen_ms) > 60_000 {
            d.last_seen_ms = now;
            let _ = self.write_devices(&all);
        }
        Some(id)
    }

    /// Forget a device. Its next request is refused.
    pub fn revoke(&self, id: &str) -> std::io::Result<bool> {
        let _g = self.devices.lock().unwrap();
        let mut all = self.read_devices();
        let before = all.len();
        all.retain(|d| d.id != id);
        if all.len() == before {
            return Ok(false);
        }
        self.write_devices(&all)?;
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn named(dir: &Path) -> Mobile {
        let config = Config { enabled: true, port: None, machine: Some(Machine { id: String::new(), name: "studio".into(), color: "#2f6fdf".into() }), url: None };
        std::fs::write(dir.join("mobile.json"), serde_json::to_string(&config).unwrap()).unwrap();
        Mobile::load(dir)
    }

    #[test]
    fn a_machine_is_given_an_id_once_and_keeps_it() {
        let dir = tempfile::tempdir().unwrap();
        let first = named(dir.path()).machine().unwrap().id.clone();
        assert_eq!(first.len(), 32);
        assert_eq!(Mobile::load(dir.path()).machine().unwrap().id, first, "the id is what a phone quotes back, so it must survive a restart");
    }

    #[test]
    fn an_unnamed_machine_has_no_identity_to_serve_under() {
        let dir = tempfile::tempdir().unwrap();
        assert!(Mobile::load(dir.path()).machine().is_none());
    }

    #[test]
    fn a_pairing_token_works_once_and_its_credential_authenticates() {
        let dir = tempfile::tempdir().unwrap();
        let m = named(dir.path());
        let token = m.new_pairing_token();
        let (device, credential) = m.pair(&token, "My phone").unwrap();
        assert_eq!(m.authenticate(&credential), Some(device.id.clone()));
        assert!(m.pair(&token, "again").is_none(), "a photo of the QR must not pair a second phone");
        assert!(!std::fs::read_to_string(dir.path().join("devices.json")).unwrap().contains(&credential), "only the hash is stored");
    }

    #[test]
    fn a_pairing_token_expires_after_a_minute() {
        let dir = tempfile::tempdir().unwrap();
        let m = named(dir.path());
        let now = Instant::now();
        let token = m.new_pairing_token_at(now);
        assert!(m.pair_at(&token, "late", now + PAIRING_TTL + Duration::from_secs(1)).is_none());
    }

    #[test]
    fn a_made_up_token_or_credential_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let m = named(dir.path());
        m.new_pairing_token();
        assert!(m.pair(&random_hex(32), "guess").is_none());
        assert!(m.authenticate(&random_hex(32)).is_none());
        assert!(m.authenticate("").is_none());
    }

    #[test]
    fn a_revoked_device_is_refused_at_once() {
        let dir = tempfile::tempdir().unwrap();
        let m = named(dir.path());
        let (device, credential) = m.pair(&m.new_pairing_token(), "p").unwrap();
        assert!(m.revoke(&device.id).unwrap());
        assert!(m.authenticate(&credential).is_none());
        assert!(m.devices().is_empty());
    }
}
