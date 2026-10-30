//! The threshold-review skill: shipped inside the app, and copied into
//! Claude Code's skills folder when the person asks.
//!
//! The one place Threshold writes outside its own folders, so it is
//! careful there: a copy that matches is left alone, and a different one
//! is only replaced when the person has said so.

use std::path::{Path, PathBuf};

use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum SkillStatus {
    /// Nothing at the destination.
    Missing,
    /// The destination's SKILL.md is the one this app ships.
    Installed,
    /// Something else is there: an older or edited copy.
    Different,
}

/// Where Claude Code looks for this skill.
pub(crate) fn destination() -> PathBuf {
    let home = std::env::var("HOME").map(PathBuf::from).unwrap_or_else(|_| PathBuf::from("."));
    home.join(".claude/skills/threshold-review")
}

pub(crate) fn status(bundled: &str, dest: &Path) -> SkillStatus {
    // Read through a symlink: a link to a checkout of this skill is an
    // install like any other.
    match std::fs::read_to_string(dest.join("SKILL.md")) {
        Ok(text) if text == bundled => SkillStatus::Installed,
        Ok(_) => SkillStatus::Different,
        Err(_) if dest.symlink_metadata().is_ok() => SkillStatus::Different,
        Err(_) => SkillStatus::Missing,
    }
}

/// Copy the bundled skill to `dest`. Refuses to replace a different copy
/// unless `replace` is set.
pub(crate) fn install(bundled: &str, dest: &Path, replace: bool) -> Result<SkillStatus, String> {
    match status(bundled, dest) {
        SkillStatus::Installed => return Ok(SkillStatus::Installed),
        SkillStatus::Different if !replace => return Err("a different copy of the skill is already installed".into()),
        _ => {}
    }
    // A symlink is replaced, not written through: writing through would
    // edit whatever it points at, which is somebody's checkout.
    if dest.symlink_metadata().is_ok_and(|m| m.file_type().is_symlink()) {
        std::fs::remove_file(dest).map_err(|e| format!("removing the old link at {}: {e}", dest.display()))?;
    }
    std::fs::create_dir_all(dest).map_err(|e| format!("creating {}: {e}", dest.display()))?;
    std::fs::write(dest.join("SKILL.md"), bundled).map_err(|e| format!("writing the skill: {e}"))?;
    Ok(status(bundled, dest))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SKILL: &str = "---\nname: threshold-review\n---\nbody\n";

    #[test]
    fn installs_into_an_empty_folder_and_then_reports_installed() {
        let tmp = tempfile::tempdir().unwrap();
        let dest = tmp.path().join("skills/threshold-review");
        assert_eq!(status(SKILL, &dest), SkillStatus::Missing);
        assert_eq!(install(SKILL, &dest, false).unwrap(), SkillStatus::Installed);
        assert_eq!(install(SKILL, &dest, false).unwrap(), SkillStatus::Installed);
    }

    #[test]
    fn a_different_copy_is_kept_unless_replacing_is_asked_for() {
        let tmp = tempfile::tempdir().unwrap();
        let dest = tmp.path().join("threshold-review");
        std::fs::create_dir_all(&dest).unwrap();
        std::fs::write(dest.join("SKILL.md"), "edited by hand").unwrap();
        assert_eq!(status(SKILL, &dest), SkillStatus::Different);
        assert!(install(SKILL, &dest, false).is_err());
        assert_eq!(std::fs::read_to_string(dest.join("SKILL.md")).unwrap(), "edited by hand");
        assert_eq!(install(SKILL, &dest, true).unwrap(), SkillStatus::Installed);
    }

    #[cfg(unix)]
    #[test]
    fn a_link_to_a_checkout_counts_and_is_never_written_through() {
        let tmp = tempfile::tempdir().unwrap();
        let checkout = tmp.path().join("checkout");
        std::fs::create_dir_all(&checkout).unwrap();
        std::fs::write(checkout.join("SKILL.md"), SKILL).unwrap();
        let dest = tmp.path().join("threshold-review");
        std::os::unix::fs::symlink(&checkout, &dest).unwrap();
        assert_eq!(status(SKILL, &dest), SkillStatus::Installed);

        // An older checkout behind the link: replacing swaps the link for
        // a copy and leaves the checkout as it was.
        std::fs::write(checkout.join("SKILL.md"), "older").unwrap();
        assert_eq!(status(SKILL, &dest), SkillStatus::Different);
        assert_eq!(install(SKILL, &dest, true).unwrap(), SkillStatus::Installed);
        assert_eq!(std::fs::read_to_string(checkout.join("SKILL.md")).unwrap(), "older");
        assert!(!dest.symlink_metadata().unwrap().file_type().is_symlink());
    }
}
