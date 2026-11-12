//! Destinations (DESIGN.md §7). Local sinks write in process, atomically, and read back.
//! Remote sinks (keychain, gh-secret, vercel-env) live on the `remote-sinks` branch.

pub mod dotenv;
pub mod file;

use std::io::{self, Write};
use std::path::Path;

use thiserror::Error;

use crate::guards::GuardError;
use crate::platform;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteOutcome {
    Created,
    Updated,
    Unchanged,
}

/// Messages carry paths, key names and line numbers only, never values.
#[derive(Debug, Error)]
pub enum SinkError {
    #[error(transparent)]
    Guard(#[from] GuardError),
    #[error(
        "this value can't be written to a dotenv file without changing it (it contains ' together with one of \" $ ` \\); use a file or keychain sink"
    )]
    Unquotable,
    #[error("{path} looks encrypted (DOTENV_PUBLIC_KEY); refusing to write plaintext into it")]
    Encrypted { path: String },
    #[error("{path}: can't tell where the entry on line {line} ends (unterminated quote); refusing to edit")]
    Unparseable { path: String, line: usize },
    #[error("{path} is not valid UTF-8")]
    NotUtf8 { path: String },
    #[error("a dropped file must be UTF-8 text to go into a dotenv file")]
    BinaryForDotenv,
    #[error("the destination is tracked by git and the human did not confirm writing to it")]
    TrackedNotConfirmed,
    #[error("the destination already holds a different value and Replace was not chosen; run the request again to choose Keep or Replace")]
    Exists,
    #[error("reading the destination back did not match what was written")]
    VerifyFailed,
    #[error("the file is larger than the {limit}-byte limit")]
    TooLarge { limit: u64 },
    #[error("{path}: {message}")]
    Io { path: String, message: String },
    #[error("{0} sinks are not implemented yet")]
    NotImplemented(&'static str),
}

impl SinkError {
    pub fn code(&self) -> &'static str {
        match self {
            SinkError::Guard(g) => g.code(),
            SinkError::Unquotable => "unquotable",
            SinkError::Encrypted { .. } => "encrypted_dotenv",
            SinkError::Unparseable { .. } => "unparseable",
            SinkError::NotUtf8 { .. } => "not_utf8",
            SinkError::BinaryForDotenv => "binary_for_dotenv",
            SinkError::TrackedNotConfirmed => "tracked_file",
            SinkError::Exists => "exists",
            SinkError::VerifyFailed => "verify_failed",
            SinkError::TooLarge { .. } => "too_large",
            SinkError::Io { .. } => "io",
            SinkError::NotImplemented(_) => "not_implemented",
        }
    }

    pub(crate) fn io(path: &Path, e: io::Error) -> SinkError {
        SinkError::Io { path: path.display().to_string(), message: e.to_string() }
    }
}

/// Create missing parent directories with mode 0700.
pub(crate) fn ensure_parent(path: &Path) -> Result<(), SinkError> {
    let parent = path.parent().ok_or_else(|| SinkError::Io {
        path: path.display().to_string(),
        message: "no parent directory".to_string(),
    })?;
    if parent.is_dir() {
        return Ok(());
    }
    platform::create_private_dir_all(parent).map_err(|e| SinkError::io(parent, e))
}

/// Write via a temp file in the destination's directory, fsync, then rename over it.
pub(crate) fn atomic_write(path: &Path, bytes: &[u8], mode: u32) -> Result<(), SinkError> {
    let dir = path.parent().ok_or(SinkError::Io { path: path.display().to_string(), message: "no parent".into() })?;
    let mut tmp = tempfile::Builder::new()
        .prefix(".passdrop-")
        .tempfile_in(dir)
        .map_err(|e| SinkError::io(dir, e))?;
    platform::set_mode(tmp.path(), mode).map_err(|e| SinkError::io(path, e))?;
    tmp.write_all(bytes).map_err(|e| SinkError::io(path, e))?;
    tmp.as_file().sync_all().map_err(|e| SinkError::io(path, e))?;
    tmp.persist(path).map_err(|e| SinkError::io(path, e.error))?;
    Ok(())
}
