//! Form state and rules, with no egui types, so it can be unit-tested.
//! window.rs only draws this state and forwards clicks.

use std::path::PathBuf;

use indexmap::IndexMap;
use passdrop_core::git::GitStatus;
use passdrop_core::guards::{self, Env};
use passdrop_core::registry::Registry;
use passdrop_core::run::{self, Answer, DropTarget, Existing, Input, Plan, PlannedField, Target};
use passdrop_core::sinks::file;
use passdrop_core::spec::{Widget, https_host};
use passdrop_core::validate::{Warning, normalize};
use passdrop_core::SecretString;
use zeroize::Zeroizing;

#[derive(Debug)]
pub struct DroppedFile {
    pub path: PathBuf,
    pub name: String,
    pub size: u64,
    pub warnings: Vec<Warning>,
    /// Where it would go, or why it can't (a guard refusal, too large, unreadable).
    pub target: Result<DropTarget, String>,
}

#[derive(Debug)]
pub struct FieldState {
    /// Typed text, or the selected enum value.
    pub text: Zeroizing<String>,
    pub file: Option<DroppedFile>,
    /// Replace (or Regenerate) an already-set value. Keep is the default.
    pub replace: bool,
    /// Write to a git-tracked destination anyway.
    pub allow_tracked: bool,
    pub add_to_gitignore: bool,
    pub trash_original: bool,
    /// Replace an existing file that has the dropped file's name.
    pub replace_dropped: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Link {
    pub host: String,
    pub url: String,
}

pub struct Form<'a> {
    pub plan: &'a Plan,
    pub env: &'a Env,
    pub registry: &'a Registry,
    pub git_env: &'a [(&'a str, &'a str)],
    pub fields: Vec<FieldState>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum DropOutcome {
    Assigned(usize),
    /// Several file fields could take it; ask the human.
    Ask(PathBuf),
    /// No file field is open for input.
    Ignored,
}

/// Tracks a file drag over the window. The pointer is trusted only if it moved during
/// the drag: some platforms don't report positions while a file is dragged in, and a
/// stale position would send a key file to the wrong field.
#[derive(Debug, Default)]
pub struct DragTracker {
    start: Option<Option<(f32, f32)>>,
}

impl DragTracker {
    /// Call once per frame. On the frame of a drop, returns the pointer position if it
    /// can be trusted.
    pub fn update(&mut self, hovering: bool, dropped: bool, pos: Option<(f32, f32)>) -> Option<(f32, f32)> {
        if dropped {
            let start = self.start.take().flatten();
            return match (pos, start) {
                (Some(p), Some(s)) if p != s => Some(p),
                _ => None,
            };
        }
        if !hovering {
            // A drag that left without dropping must not leave a stale start behind.
            self.start = None;
        } else if self.start.is_none() {
            self.start = Some(pos);
        }
        None
    }
}

impl<'a> Form<'a> {
    pub fn new(plan: &'a Plan, env: &'a Env, registry: &'a Registry, git_env: &'a [(&'a str, &'a str)]) -> Form<'a> {
        let fields = plan
            .fields
            .iter()
            .map(|pf| FieldState {
                text: Zeroizing::new(pf.field.default.clone().unwrap_or_default()),
                file: None,
                replace: false,
                allow_tracked: false,
                add_to_gitignore: pf.git == Some(GitStatus::Untracked),
                trash_original: pf.field.write_only && pf.field.widget() == Widget::File,
                replace_dropped: false,
            })
            .collect();
        Form { plan, env, registry, git_env, fields }
    }

    pub fn pf(&self, i: usize) -> &PlannedField {
        &self.plan.fields[i]
    }

    /// Already set and the human hasn't chosen Replace: no input is shown or used.
    pub fn is_kept(&self, i: usize) -> bool {
        self.pf(i).existing == Existing::Set && !self.fields[i].replace
    }

    pub fn is_generated(&self, i: usize) -> bool {
        self.pf(i).generate.is_some()
    }

    pub fn accepts_input(&self, i: usize) -> bool {
        !self.is_kept(i) && !self.is_generated(i)
    }

    fn has_value(&self, i: usize) -> bool {
        let st = &self.fields[i];
        match self.pf(i).field.widget() {
            Widget::File => st.file.is_some(),
            w => !normalize(w, &st.text).0.is_empty(),
        }
    }

    /// Will submitting write to this field's destination?
    fn will_write(&self, i: usize) -> bool {
        self.pf(i).target.is_some() && !self.is_kept(i) && (self.is_generated(i) || self.has_value(i))
    }

    /// The git status that matters for this field right now.
    pub fn git_status(&self, i: usize) -> Option<GitStatus> {
        match (&self.pf(i).target, &self.fields[i].file) {
            (Some(Target::File { name: None, .. }), Some(f)) => f.target.as_ref().ok().map(|t| t.git),
            (Some(Target::File { name: None, .. }), None) => None,
            _ => self.pf(i).git,
        }
    }

    /// Why Submit is disabled. Empty means it's enabled.
    pub fn blockers(&self) -> Vec<String> {
        let mut out = Vec::new();
        for (i, pf) in self.plan.fields.iter().enumerate() {
            let title = &pf.field.title;
            let st = &self.fields[i];
            if pf.required && self.accepts_input(i) && !self.has_value(i) {
                out.push(format!("“{title}” is required."));
            }
            if let Some(f) = &st.file {
                match &f.target {
                    Err(e) => out.push(format!("“{title}”: {e}")),
                    Ok(t) if t.exists && !t.is_source && !st.replace_dropped && !st.replace => {
                        out.push(format!("“{title}”: a file named {} already exists; tick Replace to overwrite it.", f.name))
                    }
                    Ok(_) => {}
                }
            }
            if self.will_write(i) && self.git_status(i) == Some(GitStatus::Tracked) && !st.allow_tracked {
                out.push(format!("“{title}”: the destination is tracked by git; tick “Write anyway” or leave it empty."));
            }
        }
        out
    }

    pub fn can_submit(&self) -> bool {
        self.blockers().is_empty()
    }

    /// Live warnings for the current input. They never block.
    pub fn warnings(&self, i: usize) -> Vec<Warning> {
        if !self.accepts_input(i) {
            return Vec::new();
        }
        let pf = self.pf(i);
        let st = &self.fields[i];
        match pf.field.widget() {
            Widget::File => st.file.as_ref().map(|f| f.warnings.clone()).unwrap_or_default(),
            w => {
                let (v, trimmed) = normalize(w, &st.text);
                let v = Zeroizing::new(v);
                let mut out = pf.rules.check_text(&v, self.registry);
                if trimmed && !v.is_empty() {
                    out.insert(0, Warning::Trimmed);
                }
                out
            }
        }
    }

    /// Indices of file fields that can take a drop right now.
    fn open_file_fields(&self) -> Vec<usize> {
        (0..self.fields.len()).filter(|&i| self.pf(i).field.widget() == Widget::File && self.accepts_input(i)).collect()
    }

    /// Decide which field a dropped file goes to. `hovered` is the field under the
    /// pointer, if the platform reported a position during the drag.
    pub fn route_drop(&mut self, path: PathBuf, hovered: Option<usize>) -> DropOutcome {
        let open = self.open_file_fields();
        let target = match hovered {
            Some(h) if open.contains(&h) => Some(h),
            _ if open.len() == 1 => Some(open[0]),
            _ => {
                let empty: Vec<usize> = open.iter().copied().filter(|&i| self.fields[i].file.is_none()).collect();
                if open.is_empty() {
                    return DropOutcome::Ignored;
                }
                if empty.len() == 1 { Some(empty[0]) } else { None }
            }
        };
        match target {
            Some(i) => {
                self.set_file(i, path);
                DropOutcome::Assigned(i)
            }
            None => DropOutcome::Ask(path),
        }
    }

    pub fn set_file(&mut self, i: usize, path: PathBuf) {
        let pf = &self.plan.fields[i];
        let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        let (warnings, read_err) = match file::read_limited(&path, pf.field.max_bytes()) {
            Ok(bytes) => (pf.rules.check_file(&name, &bytes), None),
            Err(e) => (Vec::new(), Some(e.to_string())),
        };
        let target = match read_err {
            Some(e) => Err(e),
            None => pf.drop_target(self.env, &path, self.git_env).map_err(|e| e.to_string()),
        };
        // Offer the Trash only for a file that looks like the one asked for: dropping the
        // wrong file must never cost the user the original. They can still tick it.
        let looks_expected = target.is_ok() && !run::is_unexpected_file(&warnings);
        let write_only = pf.field.write_only;
        let st = &mut self.fields[i];
        st.add_to_gitignore = matches!(&target, Ok(t) if t.git == GitStatus::Untracked) || st.add_to_gitignore;
        st.replace_dropped = false;
        st.trash_original = write_only && looks_expected;
        st.file = Some(DroppedFile { path, name, size, warnings, target });
    }

    pub fn clear_file(&mut self, i: usize) {
        self.fields[i].file = None;
        self.fields[i].replace_dropped = false;
    }

    /// The answers for `run::apply`. Takes the typed text out of the form.
    pub fn take_answers(&mut self) -> IndexMap<String, Answer> {
        let mut out = IndexMap::new();
        for i in 0..self.fields.len() {
            let name = self.plan.fields[i].name.clone();
            let widget = self.plan.fields[i].field.widget();
            let accepts = self.accepts_input(i);
            let st = &mut self.fields[i];
            let input = if !accepts {
                Input::Skip
            } else if widget == Widget::File {
                st.file.as_ref().map_or(Input::Skip, |f| Input::File(f.path.clone()))
            } else {
                let text = std::mem::replace(&mut st.text, Zeroizing::new(String::new()));
                Input::Text(SecretString::from(text.to_string()))
            };
            let is_file = matches!(input, Input::File(_));
            // For an unexpected file the Trash box starts unticked, so a tick is the human's.
            let unexpected = st.file.as_ref().is_some_and(|f| run::is_unexpected_file(&f.warnings));
            out.insert(
                name,
                Answer {
                    input,
                    replace_existing: st.replace || st.replace_dropped,
                    allow_tracked: st.allow_tracked,
                    add_to_gitignore: st.add_to_gitignore,
                    trash_original: is_file && st.trash_original,
                    trash_unexpected: is_file && st.trash_original && unexpected,
                },
            );
        }
        out
    }

    /// Where to get the value. A registry URL wins over the agent's URLs; agent URLs
    /// must pass the same https check as the spec, and the displayed host is the real one.
    pub fn links(&self, i: usize) -> Vec<Link> {
        let pf = self.pf(i);
        if let Some(url) = self.registry.lookup_for_field(&pf.field).and_then(|h| h.entry.url.clone()) {
            return link(&url).into_iter().collect();
        }
        let mut out: Vec<Link> = Vec::new();
        let candidates = pf.field.docs_url.iter().cloned().chain(pf.field.description.iter().flat_map(|d| urls_in(d)));
        for u in candidates {
            if let Some(l) = link(&u)
                && !out.contains(&l) {
                    out.push(l);
                }
        }
        out
    }

    /// Steps from the registry (trusted data, not agent text).
    pub fn steps(&self, i: usize) -> Vec<String> {
        self.registry.lookup_for_field(&self.pf(i).field).map(|h| h.entry.steps.clone()).unwrap_or_default()
    }

    pub fn destination(&self, i: usize) -> String {
        let pf = self.pf(i);
        match (&pf.target, &self.fields[i].file) {
            (Some(Target::File { name: None, .. }), Some(f)) => match &f.target {
                Ok(t) => guards::display_path(self.env, &t.path),
                Err(_) => pf.destination_label(self.env),
            },
            _ => pf.destination_label(self.env),
        }
    }
}

fn link(url: &str) -> Option<Link> {
    https_host(url).map(|h| Link { host: h.to_string(), url: url.to_string() })
}

/// https URLs in free text, with trailing punctuation removed.
pub fn urls_in(text: &str) -> Vec<String> {
    text.split_whitespace()
        .filter_map(|w| {
            let start = w.find("https://")?;
            let u = w[start..].trim_end_matches(['.', ',', ')', ';', ':', '!', '?', '"', '\'', '»', '”']);
            Some(u.to_string())
        })
        .collect()
}

/// Who launched passdrop, from environment markers. A hint, not proof (DESIGN.md §8).
pub fn caller() -> &'static str {
    let has = |k: &str| std::env::var_os(k).is_some();
    if has("CLAUDECODE") || has("CLAUDE_CODE_ENTRYPOINT") {
        "Claude Code"
    } else if std::env::vars_os().any(|(k, _)| k.to_string_lossy().starts_with("CODEX_")) {
        "Codex"
    } else {
        "an unknown caller"
    }
}

pub fn warning_text(w: Warning) -> &'static str {
    match w {
        Warning::Pattern => "Doesn't match the expected pattern.",
        Warning::Prefix => "Doesn't start with the expected prefix.",
        Warning::Length => "Unusual length for this value.",
        Warning::Format => "Doesn't look like the expected format.",
        Warning::Extension => "Unexpected file type.",
        Warning::Placeholder => "Looks like a placeholder, not a real value.",
        Warning::Trimmed => "Surrounding spaces or line breaks will be removed.",
        Warning::Registry => "Doesn't look like this provider's usual key format. Formats change, so this may be fine.",
        Warning::LoaderDependent => "Contains $, which some .env loaders (e.g. Bun) expand.",
        Warning::LooksLikeKey => "This looks like a secret key, and this field is returned to the agent.",
        Warning::GitignoreFailed => "Couldn't add the file to .gitignore.",
        Warning::TrashFailed => "Couldn't move the original to the Trash.",
    }
}

pub fn human_size(n: u64) -> String {
    if n < 1024 { format!("{n} B") } else if n < 1024 * 1024 { format!("{:.1} KB", n as f64 / 1024.0) } else { format!("{:.1} MB", n as f64 / 1048576.0) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use passdrop_core::run;
    use passdrop_core::spec;

    struct Fx {
        _dir: tempfile::TempDir,
        env: Env,
        root: PathBuf,
    }

    fn fx() -> Fx {
        // Under target/, never /tmp (the guards refuse shared temp dirs).
        let base = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/tmp");
        std::fs::create_dir_all(&base).unwrap();
        let dir = tempfile::Builder::new().prefix("form-").tempdir_in(&base).unwrap();
        let root = std::fs::canonicalize(dir.path()).unwrap();
        let home = root.join("home");
        std::fs::create_dir_all(home.join("proj")).unwrap();
        let env = Env::new(home.clone(), home.join("proj"), unsafe { libc::geteuid() });
        Fx { _dir: dir, env, root }
    }

    const NO_GIT: [(&str, &str); 1] = [("GIT_CEILING_DIRECTORIES", "/")];

    fn plan(f: &Fx, json: &str) -> Plan {
        let c = spec::parse(json, Registry::builtin());
        assert!(c.is_valid(), "{:?}", c.errors);
        run::plan_with(c.spec.unwrap(), &f.env, Registry::builtin(), &NO_GIT).unwrap()
    }

    const TWO_FILES: &str = r#"{"title":"T","x-purpose":"P","required":["a"],"properties":{
      "a":{"title":"A","writeOnly":true,"x-widget":"file","x-sink":{"kind":"file","dir":"keys","name":"original"}},
      "b":{"title":"B","writeOnly":true,"x-widget":"file","x-sink":{"kind":"file","dir":"other","name":"original"}},
      "c":{"title":"C","writeOnly":true,"x-sink":{"kind":"dotenv","path":".env","key":"OPENAI_API_KEY"}}}}"#;

    #[test]
    fn drag_position_is_trusted_only_if_it_moved_during_this_drag() {
        let mut t = DragTracker::default();
        // Moved during the drag: trusted.
        t.update(true, false, Some((10.0, 10.0)));
        t.update(true, false, Some((50.0, 80.0)));
        assert_eq!(t.update(false, true, Some((50.0, 80.0))), Some((50.0, 80.0)));
        // No movement reported: not trusted.
        t.update(true, false, Some((5.0, 5.0)));
        assert_eq!(t.update(false, true, Some((5.0, 5.0))), None);
        // A drag that left without dropping leaves nothing stale behind.
        t.update(true, false, Some((1.0, 1.0)));
        t.update(false, false, Some((1.0, 1.0)));
        t.update(true, false, Some((300.0, 300.0)));
        assert_eq!(t.update(false, true, Some((300.0, 300.0))), None);
        // No position at all: not trusted.
        assert_eq!(t.update(false, true, None), None);
    }

    #[test]
    fn trash_is_offered_only_for_files_that_look_expected() {
        let f = fx();
        let json = r#"{"title":"T","x-purpose":"P","properties":{
          "k":{"title":"K","writeOnly":true,"x-widget":"file","x-accept":[".p8"],
               "x-sink":{"kind":"file","dir":"keys","name":"original"}}}}"#;
        let p = plan(&f, json);
        let mut form = Form::new(&p, &f.env, Registry::builtin(), &NO_GIT);
        let photo = f.root.join("06-photo.png");
        std::fs::write(&photo, b"png").unwrap();
        form.set_file(0, photo);
        assert!(!form.fields[0].trash_original, "a wrong file must not default to the Trash");
        let a = form.take_answers();
        assert!(!a["k"].trash_original && !a["k"].trash_unexpected);

        let mut form = Form::new(&p, &f.env, Registry::builtin(), &NO_GIT);
        let key = f.root.join("AuthKey_OK.p8");
        std::fs::write(&key, "-----BEGIN PRIVATE KEY-----\nx\n-----END PRIVATE KEY-----\n").unwrap();
        form.set_file(0, key);
        assert!(form.fields[0].trash_original);
        let a = form.take_answers();
        assert!(a["k"].trash_original && !a["k"].trash_unexpected);

        // Ticking the box by hand for a wrong file is explicit consent.
        let mut form = Form::new(&p, &f.env, Registry::builtin(), &NO_GIT);
        let photo = f.root.join("07-photo.png");
        std::fs::write(&photo, b"png").unwrap();
        form.set_file(0, photo);
        form.fields[0].trash_original = true;
        let a = form.take_answers();
        assert!(a["k"].trash_original && a["k"].trash_unexpected);
    }

    #[test]
    fn required_blocks_until_filled() {
        let f = fx();
        let p = plan(&f, TWO_FILES);
        let mut form = Form::new(&p, &f.env, Registry::builtin(), &NO_GIT);
        assert_eq!(form.blockers().len(), 1);
        let file = f.root.join("k.p8");
        std::fs::write(&file, "-----BEGIN PRIVATE KEY-----\n").unwrap();
        form.set_file(0, file);
        assert!(form.can_submit(), "{:?}", form.blockers());
    }

    #[test]
    fn drops_route_to_the_hovered_or_only_empty_field_otherwise_ask() {
        let f = fx();
        let p = plan(&f, TWO_FILES);
        let mut form = Form::new(&p, &f.env, Registry::builtin(), &NO_GIT);
        let x = f.root.join("x.p8");
        std::fs::write(&x, "x").unwrap();
        assert_eq!(form.route_drop(x.clone(), None), DropOutcome::Ask(x.clone()));
        assert_eq!(form.route_drop(x.clone(), Some(1)), DropOutcome::Assigned(1));
        assert_eq!(form.route_drop(x.clone(), None), DropOutcome::Assigned(0));
        // Hovering the text field doesn't count as a file target.
        form.clear_file(0);
        form.clear_file(1);
        assert_eq!(form.route_drop(x.clone(), Some(2)), DropOutcome::Ask(x));
    }

    #[test]
    fn a_name_collision_blocks_until_replace() {
        let f = fx();
        let p = plan(&f, TWO_FILES);
        let mut form = Form::new(&p, &f.env, Registry::builtin(), &NO_GIT);
        std::fs::create_dir_all(f.env.cwd.join("keys")).unwrap();
        std::fs::write(f.env.cwd.join("keys/k.p8"), "old").unwrap();
        let file = f.root.join("k.p8");
        std::fs::write(&file, "-----BEGIN PRIVATE KEY-----\nnew\n").unwrap();
        form.set_file(0, file);
        assert!(form.blockers().iter().any(|b| b.contains("already exists")));
        form.fields[0].replace_dropped = true;
        assert!(form.can_submit());
        let a = form.take_answers();
        assert!(a["a"].replace_existing && a["a"].trash_original);
    }

    #[test]
    fn kept_fields_send_no_input_and_registry_links_win() {
        let f = fx();
        std::fs::write(f.env.cwd.join(".env"), "OPENAI_API_KEY=sk-old\n").unwrap();
        let json = r#"{"title":"T","x-purpose":"P","properties":{
          "c":{"title":"C","writeOnly":true,"x-docs-url":"https://evil.example.com/get-key",
               "x-sink":{"kind":"dotenv","path":".env","key":"OPENAI_API_KEY"}}}}"#;
        let p = plan(&f, json);
        let mut form = Form::new(&p, &f.env, Registry::builtin(), &NO_GIT);
        assert!(form.is_kept(0));
        form.fields[0].text = Zeroizing::new("sk-typed".into());
        let a = form.take_answers();
        assert!(matches!(a["c"].input, Input::Skip) && !a["c"].replace_existing);
        assert_eq!(form.links(0), vec![Link { host: "platform.openai.com".into(), url: "https://platform.openai.com/api-keys".into() }]);
    }

    #[test]
    fn agent_links_are_checked_and_show_the_real_host() {
        let f = fx();
        let json = r#"{"title":"T","x-purpose":"P","properties":{
          "k":{"title":"K","description":"Get it at https://openai.com@evil.example/x, or https://example.com/keys.","x-echo":true}}}"#;
        let p = plan(&f, json);
        let form = Form::new(&p, &f.env, Registry::builtin(), &NO_GIT);
        assert_eq!(form.links(0), vec![Link { host: "example.com".into(), url: "https://example.com/keys".into() }]);
    }

    #[test]
    fn live_warnings_follow_the_input() {
        let f = fx();
        let p = plan(&f, TWO_FILES);
        let mut form = Form::new(&p, &f.env, Registry::builtin(), &NO_GIT);
        form.fields[2].text = Zeroizing::new(" pk-wrong-format-value ".into());
        assert_eq!(form.warnings(2), vec![Warning::Trimmed, Warning::Registry]);
    }
}
