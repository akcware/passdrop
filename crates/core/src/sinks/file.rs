//! `file` sink: write bytes (a dropped file or a text value) to a path with an owner-only mode.

use std::fs;
use std::io;
use std::path::Path;

use zeroize::Zeroizing;

use super::{SinkError, WriteOutcome, atomic_write, ensure_parent};

pub const DEFAULT_MODE: u32 = 0o600;

/// Octal mode with owner bits only and owner read set, e.g. "600", "0400".
pub fn parse_mode(s: &str) -> Option<u32> {
    if s.is_empty() || s.len() > 4 || !s.chars().all(|c| ('0'..='7').contains(&c)) {
        return None;
    }
    let m = u32::from_str_radix(s, 8).ok()?;
    if m & 0o077 != 0 || m & 0o400 == 0 || m > 0o700 {
        return None;
    }
    Some(m)
}

pub fn exists(path: &Path) -> bool {
    path.is_file()
}

/// Without `overwrite`, an existing file with different contents is left alone (`exists`).
pub fn write(path: &Path, bytes: &[u8], mode: u32, overwrite: bool) -> Result<WriteOutcome, SinkError> {
    let existed = match fs::read(path) {
        Ok(old) => {
            let old = Zeroizing::new(old);
            if old.as_slice() == bytes {
                return Ok(WriteOutcome::Unchanged);
            }
            if !overwrite {
                return Err(SinkError::Exists);
            }
            true
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => false,
        Err(e) => return Err(SinkError::io(path, e)),
    };
    ensure_parent(path)?;
    atomic_write(path, bytes, mode)?;
    let back = Zeroizing::new(fs::read(path).map_err(|e| SinkError::io(path, e))?);
    if back.as_slice() != bytes {
        return Err(SinkError::VerifyFailed);
    }
    Ok(if existed { WriteOutcome::Updated } else { WriteOutcome::Created })
}

/// Read a dropped file, refusing anything over `limit` bytes.
pub fn read_limited(path: &Path, limit: u64) -> Result<Zeroizing<Vec<u8>>, SinkError> {
    let meta = fs::metadata(path).map_err(|e| SinkError::io(path, e))?;
    if !meta.is_file() {
        return Err(SinkError::Io { path: path.display().to_string(), message: "not a regular file".into() });
    }
    if meta.len() > limit {
        return Err(SinkError::TooLarge { limit });
    }
    let bytes = Zeroizing::new(fs::read(path).map_err(|e| SinkError::io(path, e))?);
    if bytes.len() as u64 > limit {
        return Err(SinkError::TooLarge { limit });
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modes() {
        assert_eq!(parse_mode("600"), Some(0o600));
        assert_eq!(parse_mode("0400"), Some(0o400));
        assert_eq!(parse_mode("700"), Some(0o700));
        assert_eq!(parse_mode("644"), None);
        assert_eq!(parse_mode("200"), None);
        assert_eq!(parse_mode("4600"), None);
        assert_eq!(parse_mode("rw"), None);
        assert_eq!(parse_mode(""), None);
    }
}
