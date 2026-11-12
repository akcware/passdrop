//! Running external CLIs (`gh`, `vercel`) safely: the value goes in on stdin, never on
//! the command line, and anything we relay from their output has the value redacted.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use zeroize::Zeroizing;

use super::SinkError;

/// Which executables to run. Tests point these at fakes.
#[derive(Debug, Clone)]
pub struct Tools {
    pub gh: PathBuf,
    pub vercel: PathBuf,
}

impl Default for Tools {
    fn default() -> Tools {
        Tools { gh: PathBuf::from("gh"), vercel: PathBuf::from("vercel") }
    }
}

pub struct ToolOutput {
    pub stdout: Zeroizing<Vec<u8>>,
}

/// Run `program args…` in `cwd`, feeding `stdin` if given. Non-zero exit is an error
/// whose message is the first stderr line, with `secret` redacted.
pub fn run(
    name: &'static str,
    program: &Path,
    args: &[String],
    cwd: &Path,
    stdin: Option<&[u8]>,
) -> Result<ToolOutput, SinkError> {
    let mut cmd = Command::new(program);
    cmd.args(args)
        .current_dir(cwd)
        .stdin(if stdin.is_some() { Stdio::piped() } else { Stdio::null() })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env("NO_COLOR", "1")
        .env("GH_PROMPT_DISABLED", "1")
        .env("GH_NO_UPDATE_NOTIFIER", "1");
    let mut child = cmd.spawn().map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound => SinkError::ToolMissing(name),
        _ => SinkError::Tool { tool: name, message: e.to_string() },
    })?;
    if let (Some(data), Some(mut pipe)) = (stdin, child.stdin.take()) {
        // A tool that exits early closes the pipe; its exit status explains why.
        let _ = pipe.write_all(data);
    }
    let out = child.wait_with_output().map_err(|e| SinkError::Tool { tool: name, message: e.to_string() })?;
    let stdout = Zeroizing::new(out.stdout);
    let stderr = Zeroizing::new(out.stderr);
    if !out.status.success() {
        return Err(SinkError::Tool { tool: name, message: first_line(&stderr, &stdout, stdin) });
    }
    Ok(ToolOutput { stdout })
}

/// A short, value-free summary of what went wrong.
fn first_line(stderr: &[u8], stdout: &[u8], secret: Option<&[u8]>) -> String {
    let text = Zeroizing::new(String::from_utf8_lossy(if stderr.is_empty() { stdout } else { stderr }).to_string());
    let line = text.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("exited with an error");
    let mut msg = line.chars().take(300).collect::<String>();
    if let Some(s) = secret.and_then(|s| std::str::from_utf8(s).ok())
        && s.len() >= 3 {
            msg = msg.replace(s, "<redacted>");
        }
    msg
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_messages_never_carry_the_value() {
        let m = first_line(b"\nerror: bad value hunter2-SECRET here\nmore", b"", Some(b"hunter2-SECRET"));
        assert_eq!(m, "error: bad value <redacted> here");
    }
}
