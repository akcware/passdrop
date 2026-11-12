//! `passdrop init`: install the skill for Claude Code and Codex, or remove it (`--undo`).
//! Permission rules are printed, never written: the user's settings are the user's.

use std::fs;
use std::path::{Path, PathBuf};

/// The skill shipped with this binary (the same file the Claude Code plugin uses).
pub const SKILL: &str = include_str!("../../../skills/passdrop/SKILL.md");

/// Marks a SKILL.md as ours, so `init` never overwrites and `--undo` never deletes
/// someone else's file.
const MARKER: &str = "\nname: passdrop\n";

const PERMISSIONS_NOTE: &str = "\
Recommended next steps (passdrop does not change your settings):

  Claude Code — keep agents from reading written values back; add to
  ~/.claude/settings.json or the project's .claude/settings.json:
    \"permissions\": { \"deny\": [\"Read(**/.env)\", \"Read(**/.env.*)\"] }
  If you use Claude Code's sandbox, also add \"passdrop *\" to sandbox.excludedCommands.

  Codex — passdrop must run outside the sandbox. Approve it when asked, or allow it
  permanently in ~/.codex/rules/default.rules:
    prefix_rule(pattern=[\"passdrop\"], decision=\"allow\")
  (With that rule, passdrop's own window is the only approval step.)";

pub struct Targets {
    pub claude: bool,
    pub codex: bool,
}

fn skill_dirs(home: &Path, t: &Targets) -> Vec<(&'static str, PathBuf)> {
    let mut v = Vec::new();
    if t.claude {
        v.push(("Claude Code", home.join(".claude/skills/passdrop")));
    }
    if t.codex {
        v.push(("Codex", home.join(".agents/skills/passdrop")));
    }
    v
}

fn is_ours(p: &Path) -> bool {
    fs::read_to_string(p).map(|s| s.contains(MARKER)).unwrap_or(false)
}

/// Returns the lines to print, or an error message.
pub fn install(home: &Path, t: &Targets) -> Result<Vec<String>, String> {
    let mut out = Vec::new();
    for (agent, dir) in skill_dirs(home, t) {
        let file = dir.join("SKILL.md");
        if file.exists() && !is_ours(&file) {
            return Err(format!("{} exists and is not passdrop's skill; leaving it alone", file.display()));
        }
        fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        let unchanged = fs::read_to_string(&file).map(|s| s == SKILL).unwrap_or(false);
        if !unchanged {
            fs::write(&file, SKILL).map_err(|e| format!("{}: {e}", file.display()))?;
        }
        let verb = if unchanged { "already up to date" } else { "installed" };
        out.push(format!("{agent}: skill {verb} at {}", file.display()));
    }
    out.push(String::new());
    out.push(PERMISSIONS_NOTE.to_string());
    Ok(out)
}

pub fn uninstall(home: &Path, t: &Targets) -> Result<Vec<String>, String> {
    let mut out = Vec::new();
    for (agent, dir) in skill_dirs(home, t) {
        let file = dir.join("SKILL.md");
        if !file.exists() {
            out.push(format!("{agent}: nothing to remove"));
            continue;
        }
        if !is_ours(&file) {
            return Err(format!("{} is not passdrop's skill; leaving it alone", file.display()));
        }
        fs::remove_file(&file).map_err(|e| format!("{}: {e}", file.display()))?;
        // Remove the folder only if nothing else is in it.
        let _ = fs::remove_dir(&dir);
        out.push(format!("{agent}: removed {}", file.display()));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn home() -> tempfile::TempDir {
        let base = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/tmp");
        fs::create_dir_all(&base).unwrap();
        tempfile::Builder::new().prefix("init-").tempdir_in(base).unwrap()
    }

    const BOTH: Targets = Targets { claude: true, codex: true };

    #[test]
    fn the_embedded_skill_is_marked() {
        assert!(SKILL.starts_with("---\nname: passdrop\n"));
    }

    #[test]
    fn install_is_idempotent_and_undo_removes_only_ours() {
        let h = home();
        let lines = install(h.path(), &BOTH).unwrap();
        assert!(lines[0].contains("installed") && lines[1].contains("installed"));
        let claude = h.path().join(".claude/skills/passdrop/SKILL.md");
        let codex = h.path().join(".agents/skills/passdrop/SKILL.md");
        assert_eq!(fs::read_to_string(&claude).unwrap(), SKILL);
        assert_eq!(fs::read_to_string(&codex).unwrap(), SKILL);
        assert!(install(h.path(), &BOTH).unwrap()[0].contains("already up to date"));

        // A file the user put there in the folder survives --undo.
        fs::write(h.path().join(".agents/skills/passdrop/notes.txt"), "mine").unwrap();
        uninstall(h.path(), &BOTH).unwrap();
        assert!(!claude.exists() && !h.path().join(".claude/skills/passdrop").exists());
        assert!(!codex.exists() && h.path().join(".agents/skills/passdrop/notes.txt").exists());
    }

    #[test]
    fn someone_elses_skill_is_never_touched() {
        let h = home();
        let dir = h.path().join(".claude/skills/passdrop");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("SKILL.md"), "---\nname: other\n---\n").unwrap();
        assert!(install(h.path(), &Targets { claude: true, codex: false }).is_err());
        assert!(uninstall(h.path(), &Targets { claude: true, codex: false }).is_err());
        assert_eq!(fs::read_to_string(dir.join("SKILL.md")).unwrap(), "---\nname: other\n---\n");
    }
}
