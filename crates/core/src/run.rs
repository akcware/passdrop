//! Plan a request (resolve destinations, see what is already set) and apply the human's
//! answers. The window (milestone 2) sits between `plan` and `apply`.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use indexmap::IndexMap;
use secrecy::{ExposeSecret, SecretString};
use serde::Serialize;
use zeroize::Zeroizing;

use crate::generate::GenerateSpec;
use crate::git::{self, GitStatus};
use crate::guards::{self, Env, GuardError};
use crate::registry::Registry;
use crate::result::{Action, FieldResult, NEXT_FAILED, NEXT_OK, RunResult, Status};
use crate::sinks::{SinkError, WriteOutcome, dotenv, file};
use crate::spec::{Field, Issue, SinkSpec, Spec, Widget};
use crate::validate::{Rules, Warning, normalize};

#[derive(Debug, Clone)]
pub enum RawFile {
    Path(String),
    Dir(String),
}

#[derive(Debug, Clone)]
pub enum Target {
    Dotenv { path: PathBuf, key: String, raw: String },
    /// `name: None` means "the dropped file's own name".
    File { dir: PathBuf, name: Option<String>, mode: u32, raw: RawFile },
    Remote(SinkSpec),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Existing {
    Set,
    NotSet,
    Unknown,
}

#[derive(Debug, Clone)]
pub struct PlannedField {
    pub name: String,
    pub field: Field,
    pub required: bool,
    pub target: Option<Target>,
    pub rules: Rules,
    pub generate: Option<GenerateSpec>,
    pub existing: Existing,
    pub git: Option<GitStatus>,
}

pub struct Plan {
    pub spec: Spec,
    pub fields: Vec<PlannedField>,
    pub warnings: Vec<Issue>,
}

/// Where a dropped file would go, known only once the human drops it.
#[derive(Debug, Clone)]
pub struct DropTarget {
    pub path: PathBuf,
    /// Something is already at `path`; writing needs Replace.
    pub exists: bool,
    pub git: GitStatus,
    /// The dropped file is the destination itself (dropped from its own folder).
    pub is_source: bool,
}

impl PlannedField {
    /// For a file field: where `dropped` would be written, with the same guards as a write.
    pub fn drop_target(&self, env: &Env, dropped: &Path, git_env: &[(&str, &str)]) -> Result<DropTarget, GuardError> {
        let Some(Target::File { dir, name, .. }) = &self.target else {
            return Err(GuardError::Malformed);
        };
        let n = match name {
            Some(n) => n.clone(),
            None => dropped.file_name().map(|n| n.to_string_lossy().to_string()).ok_or(GuardError::Malformed)?,
        };
        if !guards::is_valid_file_name(&n) {
            return Err(GuardError::Refused("the file name is not a plain file name".into()));
        }
        let path = dir.join(&n);
        guards::check_path(env, &path)?;
        guards::check_existing(env, &path)?;
        Ok(DropTarget {
            exists: file::exists(&path),
            git: git::status_with(&path, git_env),
            is_source: same_file(dropped, &path),
            path,
        })
    }

    /// The destination line shown in the window (DESIGN.md §8).
    pub fn destination_label(&self, env: &Env) -> String {
        let to_agent = "returned to the agent (visible in chat)";
        let Some(target) = &self.target else {
            return to_agent.to_string();
        };
        let base = match target {
            Target::Dotenv { path, key, .. } => format!("{} · {key}", guards::display_path(env, path)),
            Target::File { dir, name, .. } => {
                let n = name.as_deref().unwrap_or("<dropped file name>");
                guards::display_path(env, &dir.join(n))
            }
            Target::Remote(SinkSpec::Keychain { service, account }) => format!("Keychain · {service} / {account}"),
            Target::Remote(SinkSpec::GhSecret { name, repo, env: e, org }) => {
                let scope = org.as_deref().map(|o| format!("org {o}")).or_else(|| repo.clone()).unwrap_or_else(|| "this repo".into());
                match e {
                    Some(e) => format!("GitHub secret {name} · {scope} · environment {e}"),
                    None => format!("GitHub secret {name} · {scope}"),
                }
            }
            Target::Remote(SinkSpec::VercelEnv { name, targets, .. }) => format!("Vercel env {name} · {}", targets.join(", ")),
            Target::Remote(other) => other.kind().to_string(),
        };
        if self.field.echo { format!("{base}, and {to_agent}") } else { base }
    }

    fn reference(&self, env: &Env, final_path: Option<&PathBuf>) -> Option<String> {
        match self.target.as_ref()? {
            Target::Dotenv { path, key, .. } => Some(format!("env-file:{}#{key}", guards::display_path(env, path))),
            Target::File { dir, name, .. } => {
                let p = final_path.cloned().or_else(|| name.as_ref().map(|n| dir.join(n)))?;
                Some(format!("file:{}", guards::display_path(env, &p)))
            }
            Target::Remote(_) => None,
        }
    }
}

pub fn plan(spec: Spec, env: &Env, registry: &Registry) -> Result<Plan, Vec<Issue>> {
    plan_with(spec, env, registry, &[])
}

/// `git_env` lets tests isolate git from the user's global config.
pub fn plan_with(spec: Spec, env: &Env, registry: &Registry, git_env: &[(&str, &str)]) -> Result<Plan, Vec<Issue>> {
    let mut errors = Vec::new();
    let mut warnings = Vec::new();
    let mut fields = Vec::new();
    let mut seen = HashSet::new();

    for (name, field) in &spec.properties {
        let p = format!("properties.{name}.x-sink");
        let target = match &field.sink {
            None => None,
            Some(sink) => match resolve_target(env, sink) {
                Ok(t) => Some(t),
                Err(e) => {
                    errors.push(Issue::new(&p, e.code(), e.to_string()));
                    None
                }
            },
        };
        if let Some(key) = target.as_ref().and_then(destination_key)
            && !seen.insert(key) {
                errors.push(Issue::new(&p, "duplicate_destination", "two fields write to the same destination"));
            }
        let (existing, git) = match &target {
            Some(Target::Dotenv { path, key, .. }) => {
                let existing = match dotenv::is_set(path, key) {
                    Ok(true) => Existing::Set,
                    Ok(false) => Existing::NotSet,
                    Err(e) => {
                        warnings.push(Issue::new(&p, e.code(), e.to_string()));
                        Existing::Unknown
                    }
                };
                (existing, Some(git::status_with(path, git_env)))
            }
            Some(Target::File { dir, name: Some(n), .. }) => {
                let path = dir.join(n);
                let existing = if file::exists(&path) { Existing::Set } else { Existing::NotSet };
                (existing, Some(git::status_with(&path, git_env)))
            }
            _ => (Existing::Unknown, None),
        };
        fields.push(PlannedField {
            name: name.clone(),
            field: field.clone(),
            required: spec.is_required(name),
            target,
            rules: Rules::for_field(field, registry),
            generate: field.generate_spec(),
            existing,
            git,
        });
    }
    if errors.is_empty() { Ok(Plan { spec, fields, warnings }) } else { Err(errors) }
}

fn resolve_target(env: &Env, sink: &SinkSpec) -> Result<Target, GuardError> {
    match sink {
        SinkSpec::Dotenv { path, key } => {
            Ok(Target::Dotenv { path: guards::resolve(env, path)?, key: key.clone(), raw: path.clone() })
        }
        SinkSpec::File { dir, name, path, mode } => {
            let mode = mode.as_deref().and_then(file::parse_mode).unwrap_or(file::DEFAULT_MODE);
            if let Some(raw) = path {
                let full = guards::resolve(env, raw)?;
                let dir = full.parent().ok_or(GuardError::Malformed)?.to_path_buf();
                let name = full.file_name().map(|n| n.to_string_lossy().to_string());
                return Ok(Target::File { dir, name, mode, raw: RawFile::Path(raw.clone()) });
            }
            let raw = dir.as_ref().ok_or(GuardError::Malformed)?;
            let dir_path = guards::resolve_path(env, raw)?;
            if dir_path.exists() && !dir_path.is_dir() {
                return Err(GuardError::Refused("the x-sink dir exists and is not a directory".into()));
            }
            let name = match name.as_deref() {
                Some("original") | None => None,
                Some(n) => Some(n.to_string()),
            };
            // Check the rules against the final file path; use a stand-in for "original".
            let probe = dir_path.join(name.as_deref().unwrap_or("passdrop-name-check"));
            guards::check_path(env, &probe)?;
            if name.is_some() {
                guards::check_existing(env, &probe)?;
            }
            Ok(Target::File { dir: dir_path, name, mode, raw: RawFile::Dir(raw.clone()) })
        }
        other => Ok(Target::Remote(other.clone())),
    }
}

fn destination_key(t: &Target) -> Option<String> {
    match t {
        Target::Dotenv { path, key, .. } => Some(format!("dotenv:{}#{key}", path.display())),
        Target::File { dir, name: Some(n), .. } => Some(format!("file:{}", dir.join(n).display())),
        Target::File { name: None, .. } => None,
        Target::Remote(s) => Some(format!("{s:?}")),
    }
}

/// What the human gave for one field.
#[derive(Debug)]
pub enum Input {
    Text(SecretString),
    File(PathBuf),
    Skip,
}

#[derive(Debug)]
pub struct Answer {
    pub input: Input,
    /// The human chose Replace for a value that is already set. Without it, an existing
    /// value is never overwritten: Keep is the default.
    pub replace_existing: bool,
    /// The human ticked "write anyway" for a git-tracked destination.
    pub allow_tracked: bool,
    /// The human left "add to .gitignore" ticked.
    pub add_to_gitignore: bool,
    /// The human left "Move the original to the Trash" ticked (dropped files only).
    pub trash_original: bool,
    /// The human ticked Trash for a dropped file that did not look like the expected one
    /// (wrong extension or format). Without this, such a file is never moved: dropping the
    /// wrong file must not cost the user their original.
    pub trash_unexpected: bool,
}

/// A dropped file "looks unexpected" when its extension or format doesn't match the field.
pub fn is_unexpected_file(file_warnings: &[Warning]) -> bool {
    file_warnings.iter().any(|w| matches!(w, Warning::Extension | Warning::Format))
}

/// Moves a dropped original to the Trash. Injected so tests never touch the real Trash.
pub type TrashFn<'a> = &'a dyn Fn(&Path) -> Result<(), String>;

/// True when both paths resolve to the same file, or when that can't be ruled out.
fn same_file(a: &Path, b: &Path) -> bool {
    match (fs::canonicalize(a), fs::canonicalize(b)) {
        (Ok(x), Ok(y)) => x == y,
        (Ok(_), Err(e)) if e.kind() == std::io::ErrorKind::NotFound => false,
        _ => true,
    }
}

impl Answer {
    pub fn text(s: impl Into<String>) -> Answer {
        Answer::with(Input::Text(SecretString::from(s.into())))
    }

    pub fn file(p: impl Into<PathBuf>) -> Answer {
        Answer::with(Input::File(p.into()))
    }

    pub fn skip() -> Answer {
        Answer::with(Input::Skip)
    }

    pub fn with(input: Input) -> Answer {
        Answer {
            input,
            replace_existing: false,
            allow_tracked: false,
            add_to_gitignore: false,
            trash_original: false,
            trash_unexpected: false,
        }
    }
}

enum Payload {
    Text(SecretString),
    Bytes(Zeroizing<Vec<u8>>),
}

pub fn apply(plan: &Plan, answers: IndexMap<String, Answer>, env: &Env, registry: &Registry) -> RunResult {
    apply_with(plan, answers, env, registry, &[])
}

pub fn apply_with(
    plan: &Plan,
    answers: IndexMap<String, Answer>,
    env: &Env,
    registry: &Registry,
    git_env: &[(&str, &str)],
) -> RunResult {
    apply_full(plan, answers, env, registry, git_env, None)
}

/// `trash` is called for a dropped original only after a verified write, and never when
/// the original is the destination itself.
pub fn apply_full(
    plan: &Plan,
    mut answers: IndexMap<String, Answer>,
    env: &Env,
    registry: &Registry,
    git_env: &[(&str, &str)],
    trash: Option<TrashFn<'_>>,
) -> RunResult {
    let mut fields = IndexMap::new();
    for pf in &plan.fields {
        let answer = answers.shift_remove(&pf.name).unwrap_or_else(Answer::skip);
        fields.insert(pf.name.clone(), apply_field(pf, answer, env, registry, git_env, trash));
    }
    let any_failed = fields.values().any(|f| f.action == Action::Failed);
    let any_done = fields.values().any(|f| !matches!(f.action, Action::Failed | Action::Skipped));
    let status = match (any_failed, any_done) {
        (false, _) => Status::Ok,
        (true, true) => Status::Partial,
        (true, false) => Status::Failed,
    };
    RunResult {
        status,
        fields,
        errors: Vec::new(),
        warnings: plan.warnings.clone(),
        message: None,
        next: Some(if status == Status::Ok { NEXT_OK } else { NEXT_FAILED }),
    }
}

fn apply_field(
    pf: &PlannedField,
    mut answer: Answer,
    env: &Env,
    registry: &Registry,
    git_env: &[(&str, &str)],
    trash: Option<TrashFn<'_>>,
) -> FieldResult {
    // The window showed "already set" with Keep as the default; only an explicit Replace writes.
    // A value that appears after planning is caught inside the sink write (`exists`).
    if pf.existing == Existing::Set && !answer.replace_existing {
        let mut r = FieldResult::new(Action::Kept);
        r.reference = pf.reference(env, None);
        return r;
    }

    let widget = pf.field.widget();
    let mut warnings = Vec::new();
    let mut filename = None;
    let mut dropped: Option<PathBuf> = None;
    let mut unexpected_file = false;
    let payload = if let Some(g) = pf.generate {
        match g.generate() {
            Ok(s) => Payload::Text(s),
            Err(_) => return FieldResult::failed("generate_failed", "the system random generator failed"),
        }
    } else {
        match std::mem::replace(&mut answer.input, Input::Skip) {
            Input::Skip => return skipped(pf),
            Input::Text(s) => {
                if widget == Widget::File {
                    return FieldResult::failed("wrong_input", "this field expects a file");
                }
                let (v, trimmed) = normalize(widget, s.expose_secret());
                let v = Zeroizing::new(v);
                if v.is_empty() {
                    return skipped(pf);
                }
                if trimmed {
                    warnings.push(Warning::Trimmed);
                }
                warnings.extend(pf.rules.check_text(&v, registry));
                Payload::Text(SecretString::from(v.to_string()))
            }
            Input::File(p) => {
                if widget != Widget::File {
                    return FieldResult::failed("wrong_input", "this field expects text");
                }
                let bytes = match file::read_limited(&p, pf.field.max_bytes()) {
                    Ok(b) => b,
                    Err(e) => return FieldResult::failed(e.code(), e.to_string()),
                };
                let fname = p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                let file_warnings = pf.rules.check_file(&fname, &bytes);
                unexpected_file = is_unexpected_file(&file_warnings);
                warnings.extend(file_warnings);
                filename = Some(fname);
                dropped = Some(p);
                Payload::Bytes(bytes)
            }
        }
    };

    let echo_value = match (&payload, pf.field.echo && !pf.field.write_only) {
        (Payload::Text(s), true) => Some(s.expose_secret().to_string()),
        _ => None,
    };
    let Some(target) = &pf.target else {
        let mut r = FieldResult::new(Action::Echoed);
        r.value = echo_value;
        r.warnings = warnings;
        return r;
    };

    let mut original = dropped.as_ref().map(|_| "kept");
    let mut r = match write_target(target, &payload, filename.as_deref(), &answer, env, git_env) {
        Ok(w) => {
            // Move the original only after a write that was verified by reading it back
            // (never for gh/vercel, which can't be read back), and never the destination.
            let may_trash = answer.trash_original
                && (!unexpected_file || answer.trash_unexpected)
                && w.verified == Some(true);
            if let (Some(src), true) = (&dropped, may_trash)
                && !w.path.as_ref().is_some_and(|p| same_file(src, p))
            {
                match trash.map(|t| t(src)) {
                    Some(Ok(())) => original = Some("trashed"),
                    _ => warnings.push(Warning::TrashFailed),
                }
            }
            if let (true, Some(GitStatus::Untracked), Some(path)) = (answer.add_to_gitignore, w.git, &w.path)
                && git::add_to_gitignore_with(path, git_env).is_err()
            {
                warnings.push(Warning::GitignoreFailed);
            }
            let action = match (pf.generate.is_some(), w.outcome) {
                (true, _) => Action::Generated,
                (false, WriteOutcome::Created) => Action::Created,
                (false, WriteOutcome::Updated) => Action::Updated,
                (false, WriteOutcome::Unchanged) => Action::Unchanged,
            };
            let mut r = FieldResult::new(action);
            r.reference = pf.reference(env, w.path.as_ref());
            r.verified = w.verified;
            r.value = echo_value;
            r
        }
        Err(e) => FieldResult::failed(e.code(), e.to_string()),
    };
    r.original = original;
    r.filename = filename;
    warnings.sort();
    warnings.dedup();
    r.warnings = warnings;
    r
}

fn skipped(pf: &PlannedField) -> FieldResult {
    if pf.required {
        FieldResult::failed("missing_required", "a required field was left empty")
    } else {
        FieldResult::new(Action::Skipped)
    }
}

struct Written {
    outcome: WriteOutcome,
    /// Local destinations only.
    path: Option<PathBuf>,
    git: Option<GitStatus>,
    /// Some(true) when read back and equal; None when the destination can't be read back.
    verified: Option<bool>,
}

impl Written {
    fn local(outcome: WriteOutcome, path: PathBuf, git: GitStatus) -> Written {
        Written { outcome, path: Some(path), git: Some(git), verified: Some(true) }
    }
}

fn payload_bytes(payload: &Payload) -> &[u8] {
    match payload {
        Payload::Text(s) => s.expose_secret().as_bytes(),
        Payload::Bytes(b) => b,
    }
}

/// Re-runs the guards right before writing: the window may have been open for minutes.
fn write_target(
    target: &Target,
    payload: &Payload,
    filename: Option<&str>,
    answer: &Answer,
    env: &Env,
    git_env: &[(&str, &str)],
) -> Result<Written, SinkError> {
    let overwrite = answer.replace_existing;
    match target {
        Target::Dotenv { path, key, raw } => {
            if &guards::resolve(env, raw)? != path {
                return Err(GuardError::Changed.into());
            }
            let git_status = git::status_with(path, git_env);
            if git_status == GitStatus::Tracked && !answer.allow_tracked {
                return Err(SinkError::TrackedNotConfirmed);
            }
            let text: &str = match payload {
                Payload::Text(s) => s.expose_secret(),
                Payload::Bytes(b) => std::str::from_utf8(b).map_err(|_| SinkError::BinaryForDotenv)?,
            };
            let outcome = dotenv::write(path, key, text, overwrite)?;
            Ok(Written::local(outcome, path.clone(), git_status))
        }
        Target::File { dir, name, mode, raw } => {
            let fname = match (name, filename) {
                (Some(n), _) => n.clone(),
                (None, Some(f)) => f.to_string(),
                (None, None) => return Err(GuardError::Malformed.into()),
            };
            if !guards::is_valid_file_name(&fname) {
                return Err(GuardError::Refused("the file name is not a plain file name".into()).into());
            }
            let full = dir.join(&fname);
            let unchanged = match raw {
                RawFile::Path(r) => guards::resolve(env, r)? == full,
                RawFile::Dir(d) => &guards::resolve_path(env, d)? == dir,
            };
            if !unchanged {
                return Err(GuardError::Changed.into());
            }
            guards::check_path(env, &full)?;
            guards::check_existing(env, &full)?;
            let git_status = git::status_with(&full, git_env);
            if git_status == GitStatus::Tracked && !answer.allow_tracked {
                return Err(SinkError::TrackedNotConfirmed);
            }
            let outcome = file::write(&full, payload_bytes(payload), *mode, overwrite)?;
            Ok(Written::local(outcome, full, git_status))
        }
        Target::Remote(s) => Err(SinkError::NotImplemented(s.kind())),
    }
}

/// `passdrop check`: per field, is a value already there? No values.
#[derive(Debug, Serialize)]
pub struct CheckField {
    pub set: Existing,
    #[serde(rename = "ref", skip_serializing_if = "Option::is_none")]
    pub reference: Option<String>,
    pub destination: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub git: Option<GitStatus>,
}

pub fn check_report(plan: &Plan, env: &Env) -> IndexMap<String, CheckField> {
    plan.fields
        .iter()
        .map(|pf| {
            let cf = CheckField {
                set: pf.existing,
                reference: pf.reference(env, None),
                destination: pf.destination_label(env),
                git: pf.git,
            };
            (pf.name.clone(), cf)
        })
        .collect()
}
