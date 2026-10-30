//! Every approve/reject decision, with the facts of the diff it was about:
//! what History shows.

use crate::facts::DiffFacts;
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Verdict {
    Approved,
    Rejected,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Decision {
    pub summary: DiffFacts,
    pub verdict: Verdict,
    pub reason: Option<String>,
    pub decided_at_ms: u64,
    /// What was reviewed: `None` for the working tree, `Some("--cached")`
    /// for the index, or the revision spec the diff was taken against.
    ///
    /// Without this a log entry says a change was rejected but not which
    /// change, and once the same repo can be reviewed at several
    /// revisions the entries stop being distinguishable — which would
    /// defeat the whole point of keeping the log (seeing a pattern like
    /// "always rejects renames in this directory").
    ///
    /// `serde(default)` so logs written before this field existed still
    /// load, as `None`, rather than failing the whole file and silently
    /// presenting an empty history.
    #[serde(default)]
    pub source: Option<String>,
    /// SHA-256 of the diff that was actually read.
    ///
    /// `source` says which question produced the diff; this says which
    /// answer. Two entries a month apart both reading `main...HEAD` are
    /// otherwise indistinguishable in the history, which defeats the
    /// point of keeping one.
    ///
    /// `None` for entries logged before pinning existed, and for a
    /// review that could not be pinned at all.
    #[serde(default)]
    pub digest: Option<String>,
}

pub fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

/// Load the log from `path`; a missing file is an empty log, not an error
/// (there simply haven't been any decisions yet).
pub fn load(path: &Path) -> Vec<Decision> {
    std::fs::read_to_string(path).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
}

pub fn save(path: &Path, decisions: &[Decision]) -> std::io::Result<()> {
    let json = serde_json::to_string_pretty(decisions).unwrap_or_default();
    std::fs::write(path, json)
}

pub fn append(path: &Path, decision: Decision) -> std::io::Result<Vec<Decision>> {
    let mut decisions = load(path);
    decisions.push(decision);
    save(path, &decisions)?;
    Ok(decisions)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_summary() -> DiffFacts {
        DiffFacts { files_changed: 3, lines_added: 10, lines_removed: 2, has_deletions: false, deleted_files: vec![], touches_tests: false, test_files: vec![] }
    }

    #[test]
    fn loading_a_missing_file_yields_an_empty_log() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("log.json");
        assert!(load(&path).is_empty());
    }

    #[test]
    fn appending_then_loading_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("log.json");
        let decision = Decision { summary: sample_summary(), verdict: Verdict::Approved, reason: None, decided_at_ms: 12345, source: None, digest: None };
        append(&path, decision.clone()).unwrap();
        assert_eq!(load(&path), vec![decision]);
    }

    #[test]
    fn appending_twice_preserves_order_and_both_entries() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("log.json");
        let a = Decision { summary: sample_summary(), verdict: Verdict::Approved, reason: None, decided_at_ms: 1, source: None, digest: None };
        let b = Decision { summary: sample_summary(), verdict: Verdict::Rejected, reason: Some("too risky".into()), decided_at_ms: 2, source: Some("--cached".into()), digest: None };
        append(&path, a.clone()).unwrap();
        let after = append(&path, b.clone()).unwrap();
        assert_eq!(after, vec![a, b]);
    }

    #[test]
    fn a_rejection_keeps_its_reason() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("log.json");
        let decision = Decision { summary: sample_summary(), verdict: Verdict::Rejected, reason: Some("breaks the public API".into()), decided_at_ms: 99, source: Some("main...HEAD".into()), digest: Some("abc".into()) };
        append(&path, decision).unwrap();
        let loaded = load(&path);
        assert_eq!(loaded[0].reason.as_deref(), Some("breaks the public API"));
    }
    #[test]
    fn a_log_written_before_the_source_field_existed_still_loads() {
        // Real shape of an on-disk log from before revisions were
        // supported. Without serde(default) this fails to parse, and
        // because load() swallows parse errors into an empty Vec, the
        // whole history would silently vanish rather than error.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("decisions.json");
        std::fs::write(
            &path,
            r#"[{"summary":{"files_changed":1,"lines_added":2,"lines_removed":3,
                 "has_deletions":false,"deleted_files":[],"touches_tests":false,
                 "test_files":[]},
                "verdict":"rejected","reason":"no","decided_at_ms":7}]"#,
        )
        .unwrap();
        let loaded = load(&path);
        assert_eq!(loaded.len(), 1, "the pre-existing entry must survive");
        assert_eq!(loaded[0].reason.as_deref(), Some("no"));
        assert_eq!(loaded[0].source, None);
        assert_eq!(loaded[0].digest, None);
    }

}
