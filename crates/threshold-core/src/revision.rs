//! Validating a revision spec before it reaches `git`.
//!
//! Every other argument this tool passes to `git` is either a literal it
//! chose itself or a path the user picked from a folder dialog. A revision
//! spec is different: it is free text, typed into a field or handed over on
//! a command line by an agent, and it lands in `git diff <spec>`.
//!
//! The arguments are passed as argv, never through a shell, so there is no
//! quoting or injection concern. The real hazard is narrower and specific
//! to git: a spec beginning with `-` is read as an *option*, not a
//! revision. `--output=/tmp/x` would make git write there. Callers also
//! separate the spec from paths with `--`, so the two rules together mean
//! a spec can only ever be understood as a revision.
//!
//! Beyond that this deliberately does not try to know what git accepts.
//! `main...HEAD`, `HEAD~3`, `v1.2^{commit}`, `release/2.0` and a raw oid
//! are all legitimate and the grammar is large; git itself is the correct
//! validator, via `rev-parse --verify`, and it reports failures better
//! than a reimplementation would.

use std::fmt;

/// Longer than any real revision spec, short enough to keep a pathological
/// value out of an error message and a log entry.
pub const MAX_REVISION_LEN: usize = 256;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RevisionError {
    Empty,
    LeadingDash,
    ControlChar,
    TooLong,
}

impl fmt::Display for RevisionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => write!(f, "revision is empty"),
            Self::LeadingDash => write!(
                f,
                "a revision may not begin with '-': git would read it as an option rather than \
                 a revision (try 'HEAD~1' or 'main...HEAD')"
            ),
            Self::ControlChar => write!(f, "a revision may not contain control characters"),
            Self::TooLong => write!(f, "a revision may not be longer than {MAX_REVISION_LEN} characters"),
        }
    }
}

impl std::error::Error for RevisionError {}

/// Check `spec` is safe to pass to `git` as a revision, returning it
/// trimmed. Says nothing about whether the revision exists — that is
/// `git rev-parse --verify`'s job, at the point of use.
pub fn validate_revision(spec: &str) -> Result<&str, RevisionError> {
    let s = spec.trim();
    if s.is_empty() {
        return Err(RevisionError::Empty);
    }
    if s.chars().count() > MAX_REVISION_LEN {
        return Err(RevisionError::TooLong);
    }
    if s.starts_with('-') {
        return Err(RevisionError::LeadingDash);
    }
    if s.chars().any(char::is_control) {
        return Err(RevisionError::ControlChar);
    }
    Ok(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_revision_shapes_people_actually_type_are_accepted() {
        for spec in [
            "HEAD",
            "HEAD~1",
            "HEAD^",
            "main...HEAD",
            "main..HEAD",
            "origin/main",
            "release/2.0",
            "v1.2.3",
            "v1.2^{commit}",
            "3d73c7b",
            "46564a3f0e1c2b3a4d5e6f708192a3b4c5d6e7f8",
        ] {
            assert_eq!(validate_revision(spec), Ok(spec), "should accept {spec:?}");
        }
    }

    #[test]
    fn surrounding_whitespace_is_trimmed_rather_than_rejected() {
        // A value pasted into a text field routinely carries it.
        assert_eq!(validate_revision("  main...HEAD \n"), Ok("main...HEAD"));
    }

    #[test]
    fn a_leading_dash_is_rejected_because_git_would_read_it_as_an_option() {
        // The one that matters: git diff --output=... writes a file.
        assert_eq!(
            validate_revision("--output=/tmp/pwned"),
            Err(RevisionError::LeadingDash)
        );
        assert_eq!(validate_revision("-n"), Err(RevisionError::LeadingDash));
        // Still rejected after trimming, not smuggled through with padding.
        assert_eq!(validate_revision("   --exec=sh"), Err(RevisionError::LeadingDash));
    }

    #[test]
    fn empty_and_whitespace_only_specs_are_rejected() {
        assert_eq!(validate_revision(""), Err(RevisionError::Empty));
        assert_eq!(validate_revision("   \t\n "), Err(RevisionError::Empty));
    }

    #[test]
    fn control_characters_are_rejected() {
        // A newline would let a value span lines in the decision log and
        // in any message quoting it.
        assert_eq!(validate_revision("main\nHEAD"), Err(RevisionError::ControlChar));
        assert_eq!(validate_revision("main\0HEAD"), Err(RevisionError::ControlChar));
    }

    #[test]
    fn an_absurdly_long_spec_is_rejected_by_character_count_not_byte_count() {
        let long: String = "é".repeat(MAX_REVISION_LEN + 1);
        assert_eq!(validate_revision(&long), Err(RevisionError::TooLong));
        // Exactly at the limit is fine, and is 2x that in bytes — so a
        // byte-length check here would wrongly reject it.
        let at_limit: String = "é".repeat(MAX_REVISION_LEN);
        assert!(validate_revision(&at_limit).is_ok());
    }
}
