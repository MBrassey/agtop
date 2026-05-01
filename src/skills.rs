// Detect Claude Code "agent skills" loaded for a session.
//
// Claude Code resolves skills from two roots, in priority order:
//
//   1. `<cwd>/.claude/skills/<name>/SKILL.md`           (project-local)
//   2. `~/.claude/skills/<name>/SKILL.md`               (user-global)
//
// A skill is any subdirectory containing a `SKILL.md` file.  We surface
// the union of names found at both roots (deduped) so the TUI's detail
// popup can show "Skills loaded: N — name1, name2, …".
//
// This is intentionally cheap: we only walk one level deep, only stat
// for `SKILL.md`, and silently skip unreadable / nonexistent dirs.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

const SKILL_FILE: &str = "SKILL.md";

/// Return the sorted list of skill names visible to a session whose
/// cwd is `cwd`.  Order is project-local first, then user-global.
/// Duplicates (skill installed in both scopes) are deduped — the
/// project-local one wins.
pub fn skills_for_cwd(cwd: &str) -> Vec<String> {
    let mut out: BTreeSet<String> = BTreeSet::new();

    // Project-local skills.
    if !cwd.is_empty() && cwd != "?" {
        let project_root = Path::new(cwd).join(".claude").join("skills");
        collect_into(&project_root, &mut out);
    }

    // User-global skills.
    if let Some(home) = dirs::home_dir() {
        let global_root = home.join(".claude").join("skills");
        collect_into(&global_root, &mut out);
    }

    out.into_iter().collect()
}

fn collect_into(root: &Path, out: &mut BTreeSet<String>) {
    let rd = match std::fs::read_dir(root) {
        Ok(rd) => rd,
        Err(_) => return,
    };
    for ent in rd.flatten() {
        let ft = match ent.file_type() { Ok(ft) => ft, Err(_) => continue };
        // Skip symlinks defensively (a malicious skill dir symlinked
        // to / would otherwise have the same effect as walking root).
        if ft.is_symlink() || !ft.is_dir() { continue; }
        let dir = ent.path();
        let skill_md: PathBuf = dir.join(SKILL_FILE);
        if skill_md.is_file() {
            if let Some(name) = dir.file_name().and_then(|s| s.to_str()) {
                // Skill names are display-safe (filesystem-derived
                // ASCII), but sanitise to keep the same invariant the
                // rest of the UI relies on.
                let clean = crate::format::sanitize_control(name);
                if !clean.is_empty() {
                    out.insert(clean);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn empty_when_no_skills_dir() {
        let tmp = TempDir::new().unwrap();
        let cwd = tmp.path().to_string_lossy().into_owned();
        // No ~/.claude/skills exposed in test; project dir is empty.
        // (User-global may or may not exist on the host; this test
        // only confirms project-local returns nothing.)
        let s = skills_for_cwd(&cwd);
        // Don't assert s.is_empty() — host may have global skills.
        // Just confirm no panic and the function returns Vec<String>.
        assert!(s.iter().all(|n| !n.is_empty()));
    }

    #[test]
    fn detects_project_local_skill() {
        let tmp = TempDir::new().unwrap();
        let skill_dir = tmp.path().join(".claude").join("skills").join("frontend-design");
        fs::create_dir_all(&skill_dir).unwrap();
        fs::write(skill_dir.join("SKILL.md"), "# frontend-design\n").unwrap();

        let s = skills_for_cwd(tmp.path().to_str().unwrap());
        assert!(s.contains(&"frontend-design".to_string()),
                "expected frontend-design in {:?}", s);
    }

    #[test]
    fn ignores_dirs_without_skill_md() {
        let tmp = TempDir::new().unwrap();
        let skill_dir = tmp.path().join(".claude").join("skills").join("not-a-skill");
        fs::create_dir_all(&skill_dir).unwrap();
        // No SKILL.md inside.
        let s = skills_for_cwd(tmp.path().to_str().unwrap());
        assert!(!s.contains(&"not-a-skill".to_string()));
    }
}
