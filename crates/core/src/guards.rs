//! Destination guards (DESIGN.md §7.2). An agent-chosen path is the largest attack
//! surface, so these block. They run at plan time and again right before each write.
//!
//! Semantics:
//! - Writes directly into `/` or the home directory are refused (e.g. `~/.zshrc`);
//!   subdirectories of home are fine.
//! - Everything under shared temp dirs, system dirs and the listed home prefixes is refused.
//! - Path components that hold credentials, hooks or agent configuration are refused
//!   anywhere in the path. `.claude` and `.codex` are allowed only when followed by
//!   `worktrees`, so a `.env` inside a worktree checkout is still writable.
//! - Comparisons ignore ASCII case, because macOS file systems usually do.

use std::ffi::OsStr;
use std::fs;
use std::io;
use std::os::unix::fs::MetadataExt;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use thiserror::Error;

use crate::sinks::keychain::{KeychainBackend, NativeKeychain};
use crate::sinks::tool::Tools;

const SHARED_TEMP: [&str; 4] = ["/tmp", "/var/tmp", "/private/tmp", "/private/var/tmp"];
const SYSTEM: [&str; 16] = [
    "/etc", "/private/etc", "/usr", "/bin", "/sbin", "/System", "/Library", "/Applications", "/opt", "/cores",
    "/dev", "/proc", "/sys", "/boot", "/root", "/var/root",
];
const HOME_PREFIXES: [&str; 9] = [
    ".ssh", ".aws", ".kube", ".gnupg", ".docker", ".config/gh", ".local", "Library/Keychains", "Library/LaunchAgents",
];
const PROTECTED_COMPONENTS: [&str; 12] = [
    ".git", ".github", ".husky", ".ssh", ".aws", ".kube", ".gnupg", ".vscode", ".idea", ".cursor", ".gemini", ".agents",
];
const WORKTREE_PARENTS: [&str; 2] = [".claude", ".codex"];

/// Where passdrop runs, and what it talks to. Tests pass a fake home, fake `gh`/`vercel`
/// executables and an in-memory keychain, so nothing real is touched.
#[derive(Debug, Clone)]
pub struct Env {
    pub home: PathBuf,
    pub cwd: PathBuf,
    pub uid: u32,
    pub tools: Tools,
    pub keychain: Arc<dyn KeychainBackend>,
}

impl Env {
    /// Canonicalizes home and cwd when they exist, so comparisons see real paths.
    pub fn new(home: PathBuf, cwd: PathBuf, uid: u32) -> Env {
        let home = fs::canonicalize(&home).unwrap_or(home);
        let cwd = fs::canonicalize(&cwd).unwrap_or(cwd);
        Env { home, cwd, uid, tools: Tools::default(), keychain: Arc::new(NativeKeychain) }
    }
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum GuardError {
    #[error("the path is empty or malformed")]
    Malformed,
    #[error("paths starting with ~user are not supported")]
    TildeUser,
    #[error("'..' is not allowed in the part of the path that doesn't exist yet")]
    DotDot,
    #[error("refused: {0}")]
    Refused(String),
    #[error("the destination exists and is not a regular file")]
    NotRegularFile,
    #[error("the destination is owned by another user")]
    WrongOwner,
    #[error("the destination changed since the request was planned")]
    Changed,
    #[error("can't inspect the path: {0}")]
    Io(String),
}

impl GuardError {
    pub fn code(&self) -> &'static str {
        match self {
            GuardError::Malformed => "malformed_path",
            GuardError::TildeUser => "malformed_path",
            GuardError::DotDot => "malformed_path",
            GuardError::Refused(_) => "destination_refused",
            GuardError::NotRegularFile => "not_regular_file",
            GuardError::WrongOwner => "wrong_owner",
            GuardError::Changed => "destination_changed",
            GuardError::Io(_) => "io",
        }
    }
}

fn io_err(e: io::Error) -> GuardError {
    GuardError::Io(e.to_string())
}

/// Resolve a destination file path and run every rule on it.
pub fn resolve(env: &Env, raw: &str) -> Result<PathBuf, GuardError> {
    let out = resolve_path(env, raw)?;
    check_path(env, &out)?;
    check_existing(env, &out)?;
    Ok(out)
}

/// Expand `~`, resolve relative paths against cwd, canonicalize the existing part
/// (following symlinks), and append the not-yet-existing rest. No rules are applied;
/// use this for directories and run `check_path` on the final file path.
pub fn resolve_path(env: &Env, raw: &str) -> Result<PathBuf, GuardError> {
    if raw.trim().is_empty() || raw.contains('\0') {
        return Err(GuardError::Malformed);
    }
    let expanded = if raw == "~" {
        env.home.clone()
    } else if let Some(rest) = raw.strip_prefix("~/") {
        env.home.join(rest)
    } else if raw.starts_with('~') {
        return Err(GuardError::TildeUser);
    } else if Path::new(raw).is_absolute() {
        PathBuf::from(raw)
    } else {
        env.cwd.join(raw)
    };

    let comps: Vec<Component> = expanded.components().collect();
    let mut k = comps.len();
    while k > 0 {
        let p: PathBuf = comps[..k].iter().collect();
        if fs::symlink_metadata(&p).is_ok() {
            break;
        }
        k -= 1;
    }
    if k == 0 {
        return Err(GuardError::Malformed);
    }
    let base: PathBuf = comps[..k].iter().collect();
    let mut out = fs::canonicalize(&base).map_err(io_err)?;
    for c in &comps[k..] {
        match c {
            Component::Normal(n) => out.push(n),
            Component::CurDir => {}
            Component::ParentDir => return Err(GuardError::DotDot),
            _ => return Err(GuardError::Malformed),
        }
    }
    Ok(out)
}

/// Rules on a resolved (canonical) path.
pub fn check_path(env: &Env, path: &Path) -> Result<(), GuardError> {
    let parent = path.parent().ok_or(GuardError::Malformed)?;
    if parent == Path::new("/") || same_ci(parent, &env.home) {
        return Err(GuardError::Refused(
            "writing directly into / or the home directory is not allowed".to_string(),
        ));
    }
    for t in SHARED_TEMP {
        if starts_with_ci(path, Path::new(t)) {
            return Err(GuardError::Refused(format!("{t} is a shared temp directory")));
        }
    }
    for s in SYSTEM {
        if starts_with_ci(path, Path::new(s)) {
            return Err(GuardError::Refused(format!("{s} is a system directory")));
        }
    }
    for rel in HOME_PREFIXES {
        if starts_with_ci(path, &env.home.join(rel)) {
            return Err(GuardError::Refused(format!("~/{rel} is protected")));
        }
    }
    let names: Vec<String> = path
        .components()
        .filter_map(|c| match c {
            Component::Normal(n) => Some(lower(n)),
            _ => None,
        })
        .collect();
    for (i, n) in names.iter().enumerate() {
        if PROTECTED_COMPONENTS.contains(&n.as_str()) {
            return Err(GuardError::Refused(format!("{n} directories are protected")));
        }
        if WORKTREE_PARENTS.contains(&n.as_str()) && names.get(i + 1).map(String::as_str) != Some("worktrees") {
            return Err(GuardError::Refused(format!("{n} directories are protected (worktrees excepted)")));
        }
    }
    Ok(())
}

/// If the target exists it must be a regular file owned by us.
pub fn check_existing(env: &Env, path: &Path) -> Result<(), GuardError> {
    match fs::metadata(path) {
        Ok(m) => {
            if !m.is_file() {
                return Err(GuardError::NotRegularFile);
            }
            if m.uid() != env.uid {
                return Err(GuardError::WrongOwner);
            }
            Ok(())
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(io_err(e)),
    }
}

/// A plain file name: no separators, not `.` or `..`, no control characters.
pub fn is_valid_file_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 255
        && name != "."
        && name != ".."
        && !name.contains('/')
        && !name.contains('\\')
        && !name.chars().any(char::is_control)
}

/// Short form for refs and the window: relative to cwd, or `~/…`, or absolute.
pub fn display_path(env: &Env, p: &Path) -> String {
    if let Ok(rel) = p.strip_prefix(&env.cwd)
        && !rel.as_os_str().is_empty() {
            return rel.display().to_string();
        }
    if let Ok(rel) = p.strip_prefix(&env.home) {
        return format!("~/{}", rel.display());
    }
    p.display().to_string()
}

fn lower(s: &OsStr) -> String {
    s.to_string_lossy().to_ascii_lowercase()
}

fn normal_lower(p: &Path) -> Vec<String> {
    p.components()
        .filter_map(|c| match c {
            Component::Normal(n) => Some(lower(n)),
            _ => None,
        })
        .collect()
}

fn starts_with_ci(path: &Path, prefix: &Path) -> bool {
    let a = normal_lower(path);
    let b = normal_lower(prefix);
    a.len() >= b.len() && a[..b.len()] == b[..]
}

fn same_ci(a: &Path, b: &Path) -> bool {
    normal_lower(a) == normal_lower(b)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env() -> Env {
        Env::new(PathBuf::from("/Users/me"), PathBuf::from("/Users/me/proj"), 501)
    }

    fn refused(p: &str) -> bool {
        matches!(check_path(&env(), Path::new(p)), Err(GuardError::Refused(_)))
    }

    #[test]
    fn direct_children_of_root_and_home_are_refused() {
        assert!(refused("/Users/me/.zshrc"));
        assert!(refused("/Users/me/.npmrc"));
        assert!(refused("/x"));
        assert!(!refused("/Users/me/proj/.env"));
    }

    #[test]
    fn system_temp_and_home_prefixes() {
        assert!(refused("/tmp/proj/.env"));
        assert!(refused("/private/tmp/x/.env"));
        assert!(refused("/etc/x/hosts"));
        assert!(refused("/Users/me/.ssh/authorized_keys"));
        assert!(refused("/Users/me/.config/gh/hosts.yml"));
        assert!(refused("/Users/me/Library/LaunchAgents/x.plist"));
        assert!(!refused("/Users/me/.appstoreconnect/private_keys/AuthKey_X.p8"));
        assert!(!refused("/Users/me/.config/myapp/key"));
    }

    #[test]
    fn protected_components_anywhere() {
        assert!(refused("/Users/me/proj/.git/hooks/pre-commit"));
        assert!(refused("/Users/me/proj/.github/workflows/x.yml"));
        assert!(refused("/Users/me/proj/.vscode/settings.json"));
        assert!(refused("/Users/me/proj/.agents/skills/x/SKILL.md"));
        assert!(refused("/Users/me/proj/.claude/settings.json"));
        assert!(refused("/Users/me/.claude/settings.json"));
        assert!(refused("/Users/me/.codex/config.toml"));
    }

    #[test]
    fn worktrees_are_allowed_but_config_inside_them_is_not() {
        assert!(!refused("/Users/me/proj/.claude/worktrees/feature-1a2b/.env"));
        assert!(!refused("/Users/me/.codex/worktrees/abc/repo/.env.local"));
        assert!(refused("/Users/me/proj/.claude/worktrees/feature/.claude/settings.json"));
        assert!(refused("/Users/me/proj/.claude/worktrees/feature/.git/config"));
    }

    #[test]
    fn case_insensitive() {
        assert!(refused("/Users/me/proj/.GIT/config"));
        assert!(refused("/users/ME/.SSH/x/y"));
        assert!(refused("/Users/me/proj/.Claude/settings.json"));
    }

    #[test]
    fn file_names() {
        assert!(is_valid_file_name("AuthKey_ABC.p8"));
        assert!(!is_valid_file_name("../x"));
        assert!(!is_valid_file_name("a/b"));
        assert!(!is_valid_file_name(".."));
        assert!(!is_valid_file_name(""));
        assert!(!is_valid_file_name("a\nb"));
    }

    #[test]
    fn tilde_user_is_rejected() {
        assert_eq!(resolve(&env(), "~root/x"), Err(GuardError::TildeUser));
        assert_eq!(resolve(&env(), ""), Err(GuardError::Malformed));
    }
}
