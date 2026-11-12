//! passdrop CLI (DESIGN.md §5).
//!
//! stdout gets exactly one JSON line for `request`, `validate` and `check`, whatever
//! happens (submit, cancel, timeout, panic). stderr never carries values.

mod form;
mod init;
mod pointer;
mod theme;
#[cfg(feature = "test-harness")]
mod harness;
mod window;

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::atomic::{AtomicU8, Ordering};
use std::time::{Duration, Instant};

use passdrop_core::guards::Env;
use passdrop_core::registry::Registry;
use passdrop_core::result::{RunResult, Status};
use passdrop_core::run::{self, Plan};
use passdrop_core::spec::{self, MAX_SPEC_BYTES, MAX_TIMEOUT_SECS, MIN_TIMEOUT_SECS};

use crate::form::Form;
use crate::window::{Options, Outcome};

const USAGE: &str = "usage: passdrop <command>

commands:
  request <spec.json | -> [--timeout SECS] [--on-top]
                             open the window, write the values, print one JSON line
  validate <spec.json | ->   check a spec and its destinations (one JSON line)
  check <spec.json | ->      is each value already set? never prints values (one JSON line)
  schema                     print the JSON Schema of the spec format
  init [--claude] [--codex] [--undo]
                             install the agent skill for Claude Code and Codex (or remove it)
  --version";

pub const CANCELLED: &str = "The user cancelled. Nothing was written. Don't ask for these values in chat.";
const TIMED_OUT: &str = "The window timed out. Nothing was written.";

// One owner for the final JSON line: whoever moves the state first.
const WAITING: u8 = 0;
const WRITING: u8 = 1;
const DONE: u8 = 2;
static STATE: AtomicU8 = AtomicU8::new(WAITING);

fn claim(from: u8, to: u8) -> bool {
    STATE.compare_exchange(from, to, Ordering::SeqCst, Ordering::SeqCst).is_ok()
}

/// Print the final line unless another path already did.
fn emit(r: &RunResult) -> i32 {
    if STATE.swap(DONE, Ordering::SeqCst) != DONE {
        println!("{}", r.to_json_line());
    }
    r.exit_code()
}

/// A panic prints a generic error line (never the payload) and its location on stderr.
fn install_panic_hook() {
    std::panic::set_hook(Box::new(|info| {
        if STATE.swap(DONE, Ordering::SeqCst) != DONE {
            println!("{}", RunResult::status_only(Status::Error, "internal error").to_json_line());
        }
        let at = info.location().map(|l| format!(" at {}:{}", l.file(), l.line())).unwrap_or_default();
        eprintln!("passdrop: internal error{at}");
    }));
}

/// Process start, for the test harness's first-frame timing.
#[cfg(feature = "test-harness")]
pub static STARTED: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();

fn main() -> ExitCode {
    #[cfg(feature = "test-harness")]
    STARTED.get_or_init(Instant::now);
    install_panic_hook();
    let args: Vec<String> = std::env::args().skip(1).collect();
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    let code = match args.as_slice() {
        ["request", rest @ ..] => request(rest),
        ["validate", spec_arg] => validate(spec_arg, false),
        ["check", spec_arg] => validate(spec_arg, true),
        ["schema"] => {
            println!("{}", serde_json::to_string_pretty(&spec::json_schema()).expect("schema serializes"));
            0
        }
        ["init", rest @ ..] => init_cmd(rest),
        ["--version"] | ["-V"] => {
            println!("passdrop {}", env!("CARGO_PKG_VERSION"));
            0
        }
        ["help"] | ["--help"] | ["-h"] => {
            println!("{USAGE}");
            0
        }
        _ => usage("unknown command or wrong arguments; run `passdrop help`"),
    };
    ExitCode::from(code as u8)
}

/// Human-readable output, not JSON: this is run by people, not agents.
fn init_cmd(args: &[&str]) -> i32 {
    let mut targets = init::Targets { claude: false, codex: false };
    let mut undo = false;
    for a in args {
        match *a {
            "--claude" => targets.claude = true,
            "--codex" => targets.codex = true,
            "--undo" => undo = true,
            other => {
                eprintln!("passdrop init: unknown option {other}\n\n{USAGE}");
                return 2;
            }
        }
    }
    if !targets.claude && !targets.codex {
        targets = init::Targets { claude: true, codex: true };
    }
    let Some(home) = std::env::var_os("HOME").filter(|h| !h.is_empty()) else {
        eprintln!("passdrop init: HOME is not set");
        return 1;
    };
    let home = PathBuf::from(home);
    let result = if undo { init::uninstall(&home, &targets) } else { init::install(&home, &targets) };
    match result {
        Ok(lines) => {
            for l in lines {
                println!("{l}");
            }
            0
        }
        Err(e) => {
            eprintln!("passdrop init: {e}");
            1
        }
    }
}

fn usage(msg: &str) -> i32 {
    eprintln!("{USAGE}");
    emit(&RunResult::status_only(Status::Usage, msg))
}

fn read_spec(arg: &str) -> Result<String, String> {
    let mut buf = Vec::new();
    let limit = MAX_SPEC_BYTES as u64 + 1;
    let res = if arg == "-" {
        std::io::stdin().take(limit).read_to_end(&mut buf)
    } else {
        std::fs::File::open(arg).and_then(|f| f.take(limit).read_to_end(&mut buf))
    };
    res.map_err(|e| format!("can't read the spec: {e}"))?;
    if buf.len() > MAX_SPEC_BYTES {
        return Err(format!("the spec is larger than {MAX_SPEC_BYTES} bytes"));
    }
    String::from_utf8(buf).map_err(|_| "the spec is not UTF-8".to_string())
}

fn make_env() -> Result<Env, String> {
    let home = std::env::var_os("HOME").filter(|h| !h.is_empty()).ok_or("HOME is not set")?;
    let cwd = std::env::current_dir().map_err(|e| format!("can't read the current directory: {e}"))?;
    // SAFETY: geteuid has no preconditions and can't fail.
    let uid = unsafe { libc::geteuid() };
    Ok(Env::new(PathBuf::from(home), cwd, uid))
}

/// Parse, check and plan. Err carries the line to print.
#[allow(clippy::result_large_err)]
fn load_plan(spec_arg: &str, env: &Env) -> Result<(Plan, Vec<passdrop_core::spec::Issue>), RunResult> {
    let json = read_spec(spec_arg).map_err(|m| RunResult::status_only(Status::Usage, m))?;
    let registry = Registry::builtin();
    let parsed = spec::parse(&json, registry);
    let Some(spec) = parsed.spec.filter(|_| parsed.errors.is_empty()) else {
        return Err(RunResult::invalid_spec(parsed.errors, parsed.warnings));
    };
    let plan = run::plan(spec, env, registry).map_err(|errors| RunResult::invalid_spec(errors, parsed.warnings.clone()))?;
    let mut warnings = parsed.warnings;
    warnings.extend(plan.warnings.iter().cloned());
    Ok((plan, warnings))
}

fn validate(spec_arg: &str, check: bool) -> i32 {
    let env = match make_env() {
        Ok(e) => e,
        Err(msg) => return emit(&RunResult::status_only(Status::Error, msg)),
    };
    let (plan, warnings) = match load_plan(spec_arg, &env) {
        Ok(p) => p,
        Err(r) => return emit(&r),
    };
    if check {
        let mut out = serde_json::json!({ "status": "ok", "fields": run::check_report(&plan, &env) });
        if !warnings.is_empty() {
            out["warnings"] = serde_json::to_value(&warnings).expect("issues serialize");
        }
        STATE.store(DONE, Ordering::SeqCst);
        println!("{out}");
        return 0;
    }
    let mut r = RunResult::status_only(Status::Valid, "spec and destinations are valid");
    r.warnings = warnings;
    emit(&r)
}

fn gui_available() -> Result<(), String> {
    if std::env::var_os("SSH_CONNECTION").is_some() || std::env::var_os("SSH_TTY").is_some() {
        return Err("running over SSH, so the window can't reach the user's screen".into());
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    if std::env::var_os("DISPLAY").is_none() && std::env::var_os("WAYLAND_DISPLAY").is_none() {
        return Err("no display (DISPLAY and WAYLAND_DISPLAY are unset)".into());
    }
    Ok(())
}

/// The timeout runs on its own thread: eframe stops calling the UI while the window
/// is covered, so a frame-driven timer can stall.
fn spawn_timeout(secs: u64) {
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_secs(secs));
        if claim(WAITING, DONE) {
            println!("{}", RunResult::status_only(Status::Timeout, TIMED_OUT).to_json_line());
            std::process::exit(Status::Timeout.exit_code());
        }
    });
}

/// Real Trash. NSFileManager on macOS: Finder's AppleScript method would ask the user
/// for an extra automation permission.
fn move_to_trash(p: &Path) -> Result<(), String> {
    #[allow(unused_mut)]
    let mut ctx = trash::TrashContext::default();
    #[cfg(target_os = "macos")]
    {
        use trash::macos::{DeleteMethod, TrashContextExtMacos};
        ctx.set_delete_method(DeleteMethod::NsFileManager);
    }
    ctx.delete(p).map_err(|e| e.to_string())
}

fn request(args: &[&str]) -> i32 {
    let mut spec_arg = None;
    let mut on_top = false;
    let mut timeout = None;
    #[cfg(feature = "test-harness")]
    let mut test_input: Option<String> = None;
    #[cfg(feature = "test-harness")]
    let mut screenshot: Option<PathBuf> = None;

    let mut it = args.iter();
    while let Some(a) = it.next() {
        match *a {
            "--on-top" => on_top = true,
            "--timeout" => match it.next().and_then(|v| v.parse::<u64>().ok()) {
                Some(v) if (MIN_TIMEOUT_SECS..=MAX_TIMEOUT_SECS).contains(&v) => timeout = Some(v),
                _ => return usage(&format!("--timeout needs seconds between {MIN_TIMEOUT_SECS} and {MAX_TIMEOUT_SECS}")),
            },
            #[cfg(feature = "test-harness")]
            "--test-input" => match it.next() {
                Some(v) => test_input = Some(v.to_string()),
                None => return usage("--test-input needs a file"),
            },
            #[cfg(feature = "test-harness")]
            "--screenshot" => match it.next() {
                Some(v) => screenshot = Some(PathBuf::from(v)),
                None => return usage("--screenshot needs a file"),
            },
            s if s.starts_with("--") => return usage(&format!("unknown option {s}")),
            s if spec_arg.is_none() => spec_arg = Some(s),
            _ => return usage("request takes one spec file (or -)"),
        }
    }
    let Some(spec_arg) = spec_arg else {
        return usage("request needs a spec file (or - for stdin)");
    };
    let env = match make_env() {
        Ok(e) => e,
        Err(msg) => return emit(&RunResult::status_only(Status::Error, msg)),
    };
    let (plan, _warnings) = match load_plan(spec_arg, &env) {
        Ok(p) => p,
        Err(r) => return emit(&r),
    };
    let registry = Registry::builtin();

    #[cfg(feature = "test-harness")]
    if let (Some(input), None) = (&test_input, &screenshot) {
        let r = harness::run_without_window(&plan, &env, registry, input);
        return emit(&r);
    }

    if let Err(why) = gui_available() {
        return emit(&RunResult::status_only(Status::NoGui, why));
    }
    let secs = timeout.unwrap_or_else(|| plan.spec.timeout_secs());
    spawn_timeout(secs);

    let no_git_env: [(&str, &str); 0] = [];
    #[allow(unused_mut)]
    let mut form = Form::new(&plan, &env, registry, &no_git_env);
    #[cfg(feature = "test-harness")]
    if let Some(input) = &test_input {
        harness::prefill(&mut form, input);
    }
    let opts = Options {
        on_top,
        deadline: Instant::now() + Duration::from_secs(secs),
        #[cfg(feature = "test-harness")]
        screenshot,
        #[cfg(not(feature = "test-harness"))]
        screenshot: None,
    };
    match window::run(form, opts) {
        Err(e) => emit(&RunResult::status_only(Status::NoGui, format!("the window could not be opened: {e}"))),
        Ok(Outcome::Submitted(answers)) => {
            if !claim(WAITING, WRITING) {
                // The timeout already printed its line and is exiting the process.
                loop {
                    std::thread::park();
                }
            }
            let trash = |p: &Path| move_to_trash(p);
            let r = run::apply_full(&plan, answers, &env, registry, &no_git_env, Some(&trash));
            emit(&r)
        }
        Ok(Outcome::Cancelled | Outcome::Pending) => emit(&RunResult::status_only(Status::Cancelled, CANCELLED)),
    }
}
