//! Is a destination tracked, ignored or neither? (DESIGN.md §7.2)
//! An ignore rule does nothing for a file that's already committed, so tracked files
//! need the human's explicit confirmation. Indeterminate answers never crash.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GitStatus {
    NotRepo,
    Tracked,
    Ignored,
    Untracked,
    Unknown,
}

pub fn status(path: &Path) -> GitStatus {
    status_with(path, &[])
}

/// `envs` lets tests isolate git from the user's global config.
pub fn status_with(path: &Path, envs: &[(&str, &str)]) -> GitStatus {
    let Some((dir, rel)) = split_existing(path) else {
        return GitStatus::Unknown;
    };
    let Some(top) = git(&dir, &["rev-parse", "--show-toplevel"], envs) else {
        return GitStatus::Unknown;
    };
    if !top.status.success() {
        let err = String::from_utf8_lossy(&top.stderr).to_ascii_lowercase();
        return if err.contains("not a git repository") { GitStatus::NotRepo } else { GitStatus::Unknown };
    }
    let rel_s = rel.to_string_lossy().to_string();
    match git(&dir, &["ls-files", "--error-unmatch", "--", &rel_s], envs) {
        Some(o) if o.status.success() => return GitStatus::Tracked,
        Some(_) => {}
        None => return GitStatus::Unknown,
    }
    match git(&dir, &["check-ignore", "-q", "--", &rel_s], envs).and_then(|o| o.status.code()) {
        Some(0) => GitStatus::Ignored,
        Some(1) => GitStatus::Untracked,
        _ => GitStatus::Unknown,
    }
}

/// Append `/<path relative to the repo root>` to the repo's top-level `.gitignore`.
pub fn add_to_gitignore(path: &Path) -> Result<(), String> {
    add_to_gitignore_with(path, &[])
}

pub fn add_to_gitignore_with(path: &Path, envs: &[(&str, &str)]) -> Result<(), String> {
    let (dir, _) = split_existing(path).ok_or("no existing parent directory")?;
    let out = git(&dir, &["rev-parse", "--show-toplevel"], envs).ok_or("git is not available")?;
    if !out.status.success() {
        return Err("not inside a git repository".to_string());
    }
    let top = PathBuf::from(String::from_utf8_lossy(&out.stdout).trim().to_string());
    let top = fs::canonicalize(&top).map_err(|e| e.to_string())?;
    let rel = path.strip_prefix(&top).map_err(|_| "destination is outside the repository".to_string())?;
    let line = format!("/{}", rel.to_string_lossy());
    let gitignore = top.join(".gitignore");
    let existing = match fs::read_to_string(&gitignore) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(e.to_string()),
    };
    if existing.lines().any(|l| l.trim() == line) {
        return Ok(());
    }
    let mut f = fs::OpenOptions::new().create(true).append(true).open(&gitignore).map_err(|e| e.to_string())?;
    let mut add = String::new();
    if !existing.is_empty() && !existing.ends_with('\n') {
        add.push('\n');
    }
    add.push_str(&line);
    add.push('\n');
    f.write_all(add.as_bytes()).map_err(|e| e.to_string())
}

/// The deepest existing directory above `path`, and `path` relative to it.
fn split_existing(path: &Path) -> Option<(PathBuf, PathBuf)> {
    let mut dir = path.parent()?.to_path_buf();
    while !dir.is_dir() {
        dir = dir.parent()?.to_path_buf();
    }
    let rel = path.strip_prefix(&dir).ok()?.to_path_buf();
    Some((dir, rel))
}

fn git(dir: &Path, args: &[&str], envs: &[(&str, &str)]) -> Option<Output> {
    let mut cmd = Command::new("git");
    // Messages are matched below, so they must not be localized (git printed German here).
    cmd.arg("-C").arg(dir).args(args).stdin(Stdio::null()).env("LC_ALL", "C").env("LANGUAGE", "C");
    for (k, v) in envs {
        cmd.env(k, v);
    }
    cmd.output().ok()
}
