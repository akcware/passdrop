//! `vercel-env` sink: `vercel env add NAME TARGET`, value on stdin, one call per target.
//! Existence comes from `vercel env ls --format json`. That output includes the values of
//! "plain" variables, so it is parsed in memory for key names only and then zeroized.

use std::path::Path;

use serde::Deserialize;
use serde_json::Value;

use super::tool::{self, Tools};
use super::{SinkError, WriteOutcome};

#[derive(Deserialize)]
struct Listing {
    envs: Vec<Listed>,
}

#[derive(Deserialize)]
struct Listed {
    key: String,
    #[serde(default)]
    target: Value,
}

/// Which of `targets` already have `name`.
pub fn existing_targets(tools: &Tools, cwd: &Path, name: &str, targets: &[String]) -> Result<Vec<String>, SinkError> {
    let args: Vec<String> = ["env", "ls", "--format", "json", "--non-interactive"].map(String::from).to_vec();
    let out = tool::run("vercel", &tools.vercel, &args, cwd, None)?;
    // Parse straight from the zeroized buffer; `value` fields are never deserialized.
    let listing: Listing = serde_json::from_slice(&out.stdout)
        .map_err(|e| SinkError::Tool { tool: "vercel", message: format!("unexpected `vercel env ls` output: {e}") })?;
    let mut found = Vec::new();
    for env in listing.envs.iter().filter(|e| e.key == name) {
        let ts: Vec<String> = match &env.target {
            Value::String(s) => vec![s.clone()],
            Value::Array(a) => a.iter().filter_map(|v| v.as_str().map(String::from)).collect(),
            _ => Vec::new(),
        };
        for t in ts {
            if targets.contains(&t) && !found.contains(&t) {
                found.push(t);
            }
        }
    }
    Ok(found)
}

pub fn is_set(tools: &Tools, cwd: &Path, name: &str, targets: &[String]) -> Result<bool, SinkError> {
    Ok(!existing_targets(tools, cwd, name, targets)?.is_empty())
}

pub fn write(
    tools: &Tools,
    cwd: &Path,
    name: &str,
    targets: &[String],
    sensitive: Option<bool>,
    value: &[u8],
    overwrite: bool,
) -> Result<WriteOutcome, SinkError> {
    if std::str::from_utf8(value).is_err() {
        return Err(SinkError::BinaryNotSupported("vercel-env"));
    }
    let existing = existing_targets(tools, cwd, name, targets)?;
    if !existing.is_empty() && !overwrite {
        return Err(SinkError::Exists);
    }
    for t in targets {
        let mut args: Vec<String> = ["env", "add", name, t.as_str()].map(String::from).to_vec();
        // Vercel allows sensitive variables only for production and preview.
        match sensitive {
            Some(true) if t != "development" => args.push("--sensitive".into()),
            Some(false) => args.push("--no-sensitive".into()),
            _ => {}
        }
        if existing.contains(t) {
            args.push("--force".into());
        }
        args.extend(["--yes".to_string(), "--non-interactive".to_string()]);
        tool::run("vercel", &tools.vercel, &args, cwd, Some(value))?;
    }
    Ok(if existing.is_empty() { WriteOutcome::Created } else { WriteOutcome::Updated })
}
