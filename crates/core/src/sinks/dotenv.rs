//! `dotenv` sink (DESIGN.md §7.3).
//!
//! Quoting grammar, chosen by running awkward values through `sh`, `zsh`, Node 25
//! (`process.loadEnvFile`), Bun 1.4 and Deno 2.2 on 2026-11-11:
//! 1. Unquoted when the value is non-empty and only uses `A-Za-z0-9_./:@+=-`.
//! 2. Otherwise single quotes when the value has no `'`. All five loaders read single
//!    quotes literally, including newlines, `\`, `"`, `#` and backticks. Exception: Bun
//!    expands `$VAR` even inside single quotes, so values with `$` get a
//!    `loader_dependent` warning (validate.rs).
//! 3. Otherwise double quotes when the value has none of `"`, `$`, backtick, `\`.
//! 4. Otherwise refuse (`unquotable`).
//!
//! Double quotes with backslash escapes were rejected: Node keeps `\\` and `\"` literally.
//!
//! Editing touches only the target key's lines; every other byte stays as it was. If an
//! entry's end can't be found (unterminated quote), the file is not edited.

use std::fs;
use std::io;
use std::path::Path;
use std::sync::LazyLock;

use regex::Regex;
use zeroize::{Zeroize, Zeroizing};

use super::{SinkError, WriteOutcome, atomic_write, ensure_parent};

static KEY_LINE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\s*(export\s+)?([A-Za-z_][A-Za-z0-9_.-]*)\s*=(.*)$").expect("static regex")
});

pub const NEW_FILE_MODE: u32 = 0o600;

fn is_plain(c: char) -> bool {
    c.is_ascii_alphanumeric() || "_./:@+=-".contains(c)
}

/// Encode a value per the grammar above.
pub fn encode(value: &str) -> Result<Zeroizing<String>, SinkError> {
    if !value.is_empty() && value.chars().all(is_plain) {
        return Ok(Zeroizing::new(value.to_string()));
    }
    if !value.contains('\'') {
        return Ok(Zeroizing::new(format!("'{value}'")));
    }
    if !value.chars().any(|c| matches!(c, '"' | '$' | '`' | '\\')) {
        return Ok(Zeroizing::new(format!("\"{value}\"")));
    }
    Err(SinkError::Unquotable)
}

/// One `KEY=value` entry, possibly spanning lines `start..end`.
struct Entry {
    key: String,
    start: usize,
    end: usize,
    export: bool,
    value: Zeroizing<String>,
}

/// Find every entry. Err(line number, 1-based) if a quoted value never closes.
fn scan(lines: &[&str]) -> Result<Vec<Entry>, usize> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let Some(c) = KEY_LINE.captures(lines[i]) else {
            i += 1;
            continue;
        };
        let export = c.get(1).is_some();
        let key = c[2].to_string();
        let rest = c.get(3).map_or("", |m| m.as_str()).trim_start();
        let (value, end) = match rest.chars().next() {
            Some(q @ ('\'' | '"' | '`')) => read_quoted(lines, i, &rest[1..], q).ok_or(i + 1)?,
            _ => (Zeroizing::new(unquoted(rest).to_string()), i + 1),
        };
        out.push(Entry { key, start: i, end, export, value });
        i = end;
    }
    Ok(out)
}

fn read_quoted(lines: &[&str], start: usize, first: &str, q: char) -> Option<(Zeroizing<String>, usize)> {
    let mut acc = Zeroizing::new(String::new());
    let mut seg = first;
    let mut j = start;
    loop {
        if let Some(pos) = find_close(seg, q) {
            acc.push_str(&seg[..pos]);
            let v = if q == '"' { decode_dq(&acc) } else { acc };
            return Some((v, j + 1));
        }
        acc.push_str(seg);
        acc.push('\n');
        j += 1;
        if j >= lines.len() {
            return None;
        }
        seg = lines[j];
    }
}

/// Position of the closing quote. In double quotes, a quote preceded by an odd number of
/// backslashes is escaped.
fn find_close(s: &str, q: char) -> Option<usize> {
    if q != '"' {
        return s.find(q);
    }
    let bytes = s.as_bytes();
    for (i, &b) in bytes.iter().enumerate() {
        if b == b'"' {
            let backslashes = bytes[..i].iter().rev().take_while(|&&c| c == b'\\').count();
            if backslashes % 2 == 0 {
                return Some(i);
            }
        }
    }
    None
}

/// Common dotenv escapes inside double quotes. Our own writes never contain backslashes
/// in double quotes, so this only matters for comparing values written by other tools.
fn decode_dq(s: &str) -> Zeroizing<String> {
    let mut out = Zeroizing::new(String::with_capacity(s.len()));
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n') => out.push('\n'),
            Some('r') => out.push('\r'),
            Some('t') => out.push('\t'),
            Some('"') => out.push('"'),
            Some('\\') => out.push('\\'),
            Some(other) => {
                out.push('\\');
                out.push(other);
            }
            None => out.push('\\'),
        }
    }
    out
}

/// Unquoted values end at an inline comment (` #`) and lose trailing whitespace.
fn unquoted(rest: &str) -> &str {
    let cut = rest.find(" #").into_iter().chain(rest.find("\t#")).min().unwrap_or(rest.len());
    rest[..cut].trim_end()
}

fn strip_eol(line: &str) -> &str {
    let l = line.strip_suffix('\n').unwrap_or(line);
    l.strip_suffix('\r').unwrap_or(l)
}

fn line_terminator(line: &str) -> &'static str {
    if line.ends_with("\r\n") {
        "\r\n"
    } else if line.ends_with('\n') {
        "\n"
    } else {
        ""
    }
}

fn read_text(path: &Path) -> Result<Option<Zeroizing<String>>, SinkError> {
    let bytes = match fs::read(path) {
        Ok(b) => b,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(SinkError::io(path, e)),
    };
    match String::from_utf8(bytes) {
        Ok(s) => Ok(Some(Zeroizing::new(s))),
        Err(e) => {
            e.into_bytes().zeroize();
            Err(SinkError::NotUtf8 { path: path.display().to_string() })
        }
    }
}

/// The current value of `key` (last definition wins, as in `sh` and Node).
pub fn read(path: &Path, key: &str) -> Result<Option<Zeroizing<String>>, SinkError> {
    let Some(text) = read_text(path)? else {
        return Ok(None);
    };
    let lines: Vec<&str> = text.split_inclusive('\n').map(strip_eol).collect();
    let entries = scan(&lines).map_err(|line| SinkError::Unparseable { path: path.display().to_string(), line })?;
    Ok(entries.into_iter().rev().find(|e| e.key == key).map(|e| e.value))
}

pub fn is_set(path: &Path, key: &str) -> Result<bool, SinkError> {
    Ok(read(path, key)?.is_some_and(|v| !v.is_empty()))
}

/// Set `key` to `value`: replace the first definition in place, drop duplicates, or
/// append. Keeps `export `, line endings and the file mode. Reads back to verify.
///
/// `Unchanged` (no write) only when the existing line is exactly what we would write;
/// a line that merely decodes to the same value is rewritten in our grammar, because
/// real loaders may read it differently. Without `overwrite`, a non-empty different
/// value is left alone (`exists`).
pub fn write(path: &Path, key: &str, value: &str, overwrite: bool) -> Result<WriteOutcome, SinkError> {
    let enc = encode(value)?;
    let (text, mode) = match read_text(path)? {
        Some(t) => {
            let mode = crate::platform::mode_of(&fs::metadata(path).map_err(|e| SinkError::io(path, e))?);
            (t, mode)
        }
        None => (Zeroizing::new(String::new()), NEW_FILE_MODE),
    };
    if text.contains("DOTENV_PUBLIC_KEY") {
        return Err(SinkError::Encrypted { path: path.display().to_string() });
    }
    let eol = if text.contains("\r\n") { "\r\n" } else { "\n" };
    let raw: Vec<&str> = text.split_inclusive('\n').collect();
    let lines: Vec<&str> = raw.iter().map(|l| strip_eol(l)).collect();
    let entries = scan(&lines).map_err(|line| SinkError::Unparseable { path: path.display().to_string(), line })?;
    let mine: Vec<&Entry> = entries.iter().filter(|e| e.key == key).collect();
    if let [only] = mine.as_slice() {
        let export = if only.export { "export " } else { "" };
        let expected = Zeroizing::new(format!("{export}{key}={}", enc.as_str()));
        let existing = Zeroizing::new(lines[only.start..only.end].join("\n"));
        if *existing == *expected {
            return Ok(WriteOutcome::Unchanged);
        }
    }
    if !overwrite && mine.iter().any(|e| !e.value.is_empty() && e.value.as_str() != value) {
        return Err(SinkError::Exists);
    }

    let mut out = Zeroizing::new(String::with_capacity(text.len() + key.len() + enc.len() + 16));
    match mine.first() {
        None => {
            out.push_str(&text);
            if !text.is_empty() && !text.ends_with('\n') {
                out.push_str(eol);
            }
            out.push_str(key);
            out.push('=');
            out.push_str(&enc);
            out.push_str(eol);
        }
        Some(first) => {
            let mut i = 0;
            while i < raw.len() {
                if let Some(e) = mine.iter().find(|e| e.start == i) {
                    if e.start == first.start {
                        if e.export {
                            out.push_str("export ");
                        }
                        out.push_str(key);
                        out.push('=');
                        out.push_str(&enc);
                        out.push_str(line_terminator(raw[e.end - 1]));
                    }
                    i = e.end;
                    continue;
                }
                out.push_str(raw[i]);
                i += 1;
            }
        }
    }

    ensure_parent(path)?;
    atomic_write(path, out.as_bytes(), mode)?;
    match read(path, key)? {
        Some(v) if v.as_str() == value => {}
        _ => return Err(SinkError::VerifyFailed),
    }
    Ok(if mine.is_empty() { WriteOutcome::Created } else { WriteOutcome::Updated })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn enc(v: &str) -> String {
        encode(v).map(|z| z.to_string()).unwrap_or_else(|e| format!("ERR:{}", e.code()))
    }

    #[test]
    fn grammar() {
        assert_eq!(enc("sk-proj_abc.123/x:y@z+w="), "sk-proj_abc.123/x:y@z+w=");
        assert_eq!(enc(""), "''");
        assert_eq!(enc("with space"), "'with space'");
        assert_eq!(enc("a$HOME"), "'a$HOME'");
        assert_eq!(enc("back\\slash"), "'back\\slash'");
        assert_eq!(enc("dq\"q"), "'dq\"q'");
        assert_eq!(enc("l1\nl2"), "'l1\nl2'");
        assert_eq!(enc("it's"), "\"it's\"");
        assert_eq!(enc("it's $x"), "ERR:unquotable");
        assert_eq!(enc("it's \"x\""), "ERR:unquotable");
    }

    #[test]
    fn scan_handles_multiline_quotes_and_comments() {
        let text = "# comment\nA=1 # note\nexport B='x\ny'\nC=\"q\\\"z\"\nD=`t`\n";
        let lines: Vec<&str> = text.split_inclusive('\n').map(strip_eol).collect();
        let e = scan(&lines).unwrap();
        let got: Vec<(String, String, usize, usize, bool)> =
            e.iter().map(|e| (e.key.clone(), e.value.to_string(), e.start, e.end, e.export)).collect();
        assert_eq!(
            got,
            vec![
                ("A".into(), "1".into(), 1, 2, false),
                ("B".into(), "x\ny".into(), 2, 4, true),
                ("C".into(), "q\"z".into(), 4, 5, false),
                ("D".into(), "t".into(), 5, 6, false),
            ]
        );
    }

    #[test]
    fn unterminated_quote_is_reported_with_its_line() {
        let lines = ["A=1", "B='open", "C=2"];
        assert_eq!(scan(&lines).err(), Some(2));
    }
}
