//! Test-only (cargo feature `test-harness`, never in release builds):
//! answers from a JSON file instead of the window, and PNG screenshots of the window.

use std::fs;
use std::path::{Path, PathBuf};

use eframe::egui::ColorImage;
use indexmap::IndexMap;
use passdrop_core::SecretString;
use passdrop_core::guards::Env;
use passdrop_core::registry::Registry;
use passdrop_core::result::{RunResult, Status};
use passdrop_core::run::{self, Answer, Input, Plan};
use serde::Deserialize;
use zeroize::Zeroizing;

use crate::form::Form;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TestAnswer {
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    file: Option<PathBuf>,
    #[serde(default)]
    replace: bool,
    #[serde(default)]
    allow_tracked: bool,
    #[serde(default)]
    gitignore: bool,
    #[serde(default)]
    trash: bool,
    #[serde(default)]
    trash_unexpected: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TestInput {
    #[serde(default)]
    pub cancel: bool,
    #[serde(default)]
    pub fields: IndexMap<String, TestAnswer>,
}

pub fn load(path: &str) -> Result<TestInput, String> {
    let text = fs::read_to_string(path).map_err(|e| format!("test input: {e}"))?;
    serde_json::from_str(&text).map_err(|e| format!("test input: {e}"))
}

/// Same apply path as the window, with answers from a file. "Trash" moves the original
/// into `.trash/` next to the input file, so tests never touch the real Trash.
pub fn run_without_window(plan: &Plan, env: &Env, registry: &Registry, input_path: &str) -> RunResult {
    let ti = match load(input_path) {
        Ok(t) => t,
        Err(e) => return RunResult::status_only(Status::Usage, e),
    };
    if ti.cancel {
        return RunResult::status_only(Status::Cancelled, crate::CANCELLED);
    }
    let trash_dir = Path::new(input_path).parent().unwrap_or(Path::new(".")).join(".trash");
    let trash = |p: &Path| -> Result<(), String> {
        fs::create_dir_all(&trash_dir).map_err(|e| e.to_string())?;
        let name = p.file_name().ok_or("no file name")?;
        fs::rename(p, trash_dir.join(name)).map_err(|e| e.to_string())
    };
    let answers = ti
        .fields
        .into_iter()
        .map(|(k, a)| {
            let input = match (a.text, a.file) {
                (Some(t), _) => Input::Text(SecretString::from(t)),
                (None, Some(f)) => Input::File(f),
                (None, None) => Input::Skip,
            };
            let answer = Answer {
                input,
                replace_existing: a.replace,
                allow_tracked: a.allow_tracked,
                add_to_gitignore: a.gitignore,
                trash_original: a.trash,
                trash_unexpected: a.trash_unexpected,
            };
            (k, answer)
        })
        .collect();
    run::apply_full(plan, answers, env, registry, &[], Some(&trash))
}

/// Fill the window from a test input, for screenshots of a filled-in state.
pub fn prefill(form: &mut Form<'_>, path: &str) {
    let Ok(ti) = load(path) else {
        return;
    };
    for (k, a) in ti.fields {
        let Some(i) = form.plan.fields.iter().position(|p| p.name == k) else {
            continue;
        };
        form.fields[i].replace = a.replace;
        if let Some(t) = a.text {
            form.fields[i].text = Zeroizing::new(t);
        }
        if let Some(f) = a.file {
            form.set_file(i, f);
        }
    }
}

pub fn save_png(path: &Path, img: &ColorImage) {
    let [w, h] = img.size;
    let mut buf = Vec::with_capacity(w * h * 4);
    for p in &img.pixels {
        buf.extend_from_slice(&p.to_array());
    }
    if let Err(e) = image::save_buffer(path, &buf, w as u32, h as u32, image::ExtendedColorType::Rgba8) {
        eprintln!("passdrop: screenshot failed: {e}");
    }
}
