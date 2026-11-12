//! macOS and Linux.

use std::fs::{self, DirBuilder, Metadata, Permissions};
use std::io;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};

/// Who a file belongs to. On Unix, the effective uid.
pub type Owner = u32;

/// The owner of the files this process creates.
pub fn current_owner() -> Owner {
    // SAFETY: geteuid has no preconditions and can't fail.
    unsafe { libc::geteuid() }
}

pub fn owner_of(meta: &Metadata) -> Owner {
    meta.uid()
}

/// Permission bits of an existing file, so a rewrite can keep them.
pub fn mode_of(meta: &Metadata) -> u32 {
    meta.permissions().mode() & 0o777
}

/// Apply a mode the caller has already validated (owner bits only for new files).
pub fn set_mode(path: &Path, mode: u32) -> io::Result<()> {
    fs::set_permissions(path, Permissions::from_mode(mode))
}

/// Create a directory and its missing parents, each accessible to the owner only.
pub fn create_private_dir_all(path: &Path) -> io::Result<()> {
    DirBuilder::new().recursive(true).mode(0o700).create(path)
}

/// The environment variable `home_dir` reads, for error messages.
pub const HOME_VAR: &str = "HOME";

/// The user's home directory. `None` when the variable is unset or empty.
pub fn home_dir() -> Option<PathBuf> {
    std::env::var_os(HOME_VAR).filter(|h| !h.is_empty()).map(PathBuf::from)
}

/// Shared temp dirs: any user can create files there.
pub const SHARED_TEMP: &[&str] = &["/tmp", "/var/tmp", "/private/tmp", "/private/var/tmp"];

/// System directories, refused with everything under them.
pub const SYSTEM: &[&str] = &[
    "/etc", "/private/etc", "/usr", "/bin", "/sbin", "/System", "/Library", "/Applications", "/opt", "/cores",
    "/dev", "/proc", "/sys", "/boot", "/root", "/var/root",
];

/// Credential and autostart locations relative to home, refused with everything under them.
pub const HOME_PREFIXES: &[&str] = &[
    ".ssh", ".aws", ".kube", ".gnupg", ".docker", ".config/gh", ".local", "Library/Keychains", "Library/LaunchAgents",
];
