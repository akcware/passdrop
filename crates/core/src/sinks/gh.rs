//! `gh-secret` sink: `gh secret set NAME`, value on stdin (gh reads the body from stdin
//! when `--body` is absent). GitHub can't return a secret's value, so writes are not
//! read back; `gh secret list` tells whether the name already exists.

use std::collections::HashSet;
use std::path::Path;

use serde::Deserialize;

use super::tool::{self, Tools};
use super::{SinkError, WriteOutcome};

/// GitHub's limit for a secret value.
pub const MAX_BYTES: usize = 48 * 1024;

#[derive(Debug, Clone, Copy)]
pub struct Scope<'a> {
    pub repo: Option<&'a str>,
    pub env: Option<&'a str>,
    pub org: Option<&'a str>,
}

impl Scope<'_> {
    fn args(&self) -> Vec<String> {
        let mut a = Vec::new();
        if let Some(r) = self.repo {
            a.extend(["--repo".to_string(), r.to_string()]);
        }
        if let Some(e) = self.env {
            a.extend(["--env".to_string(), e.to_string()]);
        }
        if let Some(o) = self.org {
            a.extend(["--org".to_string(), o.to_string()]);
        }
        a
    }

    /// For refs and the window: `org:acme`, `owner/repo`, or `this repo`.
    pub fn label(&self) -> String {
        let base = match (self.org, self.repo) {
            (Some(o), _) => format!("org:{o}"),
            (None, Some(r)) => r.to_string(),
            (None, None) => "this repo".to_string(),
        };
        match self.env {
            Some(e) => format!("{base} (environment {e})"),
            None => base,
        }
    }
}

#[derive(Deserialize)]
struct Listed {
    name: String,
}

pub fn list(tools: &Tools, cwd: &Path, scope: Scope<'_>) -> Result<HashSet<String>, SinkError> {
    let mut args = vec!["secret".to_string(), "list".to_string(), "--json".to_string(), "name".to_string()];
    args.extend(scope.args());
    let out = tool::run("gh", &tools.gh, &args, cwd, None)?;
    let items: Vec<Listed> = serde_json::from_slice(&out.stdout)
        .map_err(|e| SinkError::Tool { tool: "gh", message: format!("unexpected `gh secret list` output: {e}") })?;
    Ok(items.into_iter().map(|i| i.name).collect())
}

pub fn is_set(tools: &Tools, cwd: &Path, name: &str, scope: Scope<'_>) -> Result<bool, SinkError> {
    Ok(list(tools, cwd, scope)?.contains(name))
}

pub fn write(
    tools: &Tools,
    cwd: &Path,
    name: &str,
    scope: Scope<'_>,
    value: &[u8],
    overwrite: bool,
) -> Result<WriteOutcome, SinkError> {
    if value.len() > MAX_BYTES {
        return Err(SinkError::TooLarge { limit: MAX_BYTES as u64 });
    }
    if std::str::from_utf8(value).is_err() {
        return Err(SinkError::BinaryNotSupported("gh-secret"));
    }
    let existed = is_set(tools, cwd, name, scope)?;
    if existed && !overwrite {
        return Err(SinkError::Exists);
    }
    let mut args = vec!["secret".to_string(), "set".to_string(), name.to_string()];
    args.extend(scope.args());
    tool::run("gh", &tools.gh, &args, cwd, Some(value))?;
    Ok(if existed { WriteOutcome::Updated } else { WriteOutcome::Created })
}
