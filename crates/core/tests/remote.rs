//! keychain, gh-secret and vercel-env sinks against fakes: an in-memory keychain and
//! shell scripts standing in for `gh` and `vercel`. Nothing real is touched.

mod common;

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use common::Sandbox;
use indexmap::IndexMap;
use passdrop_core::registry::Registry;
use passdrop_core::result::{Action, RunResult, Status};
use passdrop_core::run::{self, Answer, Existing, Plan};
use passdrop_core::sinks::keychain::{KeychainBackend, MemoryKeychain};
use passdrop_core::spec;

const VALUE: &str = "VALUE-remote-5e1c";

/// Records its arguments; `secret list` prints the names set so far as JSON;
/// `secret set NAME` stores stdin. `FAIL` in the state dir makes `set` fail loudly,
/// echoing the value to stderr, to prove passdrop redacts it.
const FAKE_GH: &str = r#"#!/bin/bash
STATE="$(cd "$(dirname "$0")" && pwd)/state"; mkdir -p "$STATE"
echo "$*" >> "$STATE/gh-args.log"
if [ "$1 $2" = "secret list" ]; then
  printf '['; sep=''
  [ -f "$STATE/gh-names" ] && while read -r n; do printf '%s{"name":"%s"}' "$sep" "$n"; sep=','; done < "$STATE/gh-names"
  printf ']\n'; exit 0
fi
if [ "$1 $2" = "secret set" ]; then
  value="$(cat)"
  if [ -f "$STATE/FAIL" ]; then echo "HTTP 422: could not store '$value'" >&2; exit 1; fi
  printf '%s' "$value" > "$STATE/gh-value-$3"
  grep -qx "$3" "$STATE/gh-names" 2>/dev/null || echo "$3" >> "$STATE/gh-names"
  exit 0
fi
exit 2
"#;

/// `env ls --format json` lists "KEY TARGET" records, including a plain value, as the
/// real CLI does. `env add KEY TARGET` stores stdin and refuses duplicates without --force.
const FAKE_VERCEL: &str = r#"#!/bin/bash
STATE="$(cd "$(dirname "$0")" && pwd)/state"; mkdir -p "$STATE"
echo "$*" >> "$STATE/vercel-args.log"
if [ "$1 $2" = "env ls" ]; then
  printf '{"envs":['; sep=''
  [ -f "$STATE/vercel-records" ] && while read -r k t; do
    printf '%s{"key":"%s","value":"PLAINVALUE-%s","type":"plain","target":["%s"]}' "$sep" "$k" "$k" "$t"; sep=','
  done < "$STATE/vercel-records"
  printf ']}\n'; exit 0
fi
if [ "$1 $2" = "env add" ]; then
  value="$(cat)"
  if grep -qx "$3 $4" "$STATE/vercel-records" 2>/dev/null && [[ " $* " != *" --force "* ]]; then
    echo "Error: already exists" >&2; exit 1
  fi
  printf '%s' "$value" > "$STATE/vercel-value-$3-$4"
  grep -qx "$3 $4" "$STATE/vercel-records" 2>/dev/null || echo "$3 $4" >> "$STATE/vercel-records"
  exit 0
fi
exit 2
"#;

struct Fx {
    s: Sandbox,
    state: PathBuf,
    kc: Arc<MemoryKeychain>,
}

fn fx() -> Fx {
    let mut s = Sandbox::new();
    let bin = s.root.join("bin");
    fs::create_dir_all(&bin).unwrap();
    for (name, body) in [("gh", FAKE_GH), ("vercel", FAKE_VERCEL)] {
        let p = bin.join(name);
        fs::write(&p, body).unwrap();
        fs::set_permissions(&p, fs::Permissions::from_mode(0o755)).unwrap();
    }
    s.env.tools.gh = bin.join("gh");
    s.env.tools.vercel = bin.join("vercel");
    let kc = Arc::new(MemoryKeychain::default());
    s.env.keychain = kc.clone();
    Fx { state: bin.join("state"), s, kc }
}

const SPEC: &str = r#"{"title":"T","x-purpose":"P","properties":{
  "kc":{"title":"KC","writeOnly":true,"x-sink":{"kind":"keychain","service":"passdrop-test","account":"me"}},
  "gh":{"title":"GH","writeOnly":true,"x-sink":{"kind":"gh-secret","name":"API_KEY","repo":"owner/repo"}},
  "vc":{"title":"VC","writeOnly":true,"x-sink":{"kind":"vercel-env","name":"API_KEY","targets":["production","development"],"sensitive":true}}}}"#;

fn plan(f: &Fx) -> Plan {
    let c = spec::parse(SPEC, Registry::builtin());
    assert!(c.is_valid(), "{:?}", c.errors);
    run::plan_with(c.spec.unwrap(), &f.s.env, Registry::builtin(), &f.s.git_env()).unwrap()
}

fn apply(f: &Fx, p: &Plan, items: Vec<(&str, Answer)>) -> RunResult {
    let a: IndexMap<String, Answer> = items.into_iter().map(|(k, v)| (k.to_string(), v)).collect();
    run::apply_with(p, a, &f.s.env, Registry::builtin(), &f.s.git_env())
}

fn read(p: &Path) -> String {
    fs::read_to_string(p).unwrap_or_default()
}

fn replace(v: &str) -> Answer {
    let mut a = Answer::text(v);
    a.replace_existing = true;
    a
}

#[test]
fn first_write_creates_everywhere_with_the_value_only_on_stdin() {
    let f = fx();
    let p = plan(&f);
    assert!(p.fields.iter().all(|pf| pf.existing == Existing::NotSet), "{:?}", p.warnings);
    let r = apply(&f, &p, vec![("kc", Answer::text(VALUE)), ("gh", Answer::text(VALUE)), ("vc", Answer::text(VALUE))]);
    assert_eq!(r.status, Status::Ok, "{}", r.to_json_line());

    assert_eq!(r.fields["kc"].action, Action::Created);
    assert_eq!(r.fields["kc"].verified, Some(true));
    assert_eq!(r.fields["kc"].reference.as_deref(), Some("keychain:passdrop-test/me"));
    assert_eq!(f.kc.get("passdrop-test", "me").unwrap().unwrap().as_slice(), VALUE.as_bytes());

    assert_eq!(r.fields["gh"].action, Action::Created);
    assert_eq!(r.fields["gh"].verified, None, "GitHub can't be read back");
    assert_eq!(r.fields["gh"].reference.as_deref(), Some("gh-secret:owner/repo#API_KEY"));
    assert_eq!(read(&f.state.join("gh-value-API_KEY")), VALUE);

    assert_eq!(r.fields["vc"].action, Action::Created);
    assert_eq!(r.fields["vc"].reference.as_deref(), Some("vercel-env:production,development#API_KEY"));
    assert_eq!(read(&f.state.join("vercel-value-API_KEY-production")), VALUE);
    assert_eq!(read(&f.state.join("vercel-value-API_KEY-development")), VALUE);

    // Never on a command line; --sensitive only where Vercel allows it.
    let gh_args = read(&f.state.join("gh-args.log"));
    let vc_args = read(&f.state.join("vercel-args.log"));
    assert!(!gh_args.contains(VALUE) && !vc_args.contains(VALUE));
    assert!(gh_args.contains("secret set API_KEY --repo owner/repo"));
    assert!(vc_args.contains("env add API_KEY production --sensitive --yes --non-interactive"));
    assert!(vc_args.contains("env add API_KEY development --yes --non-interactive"));
    assert!(!r.to_json_line().contains(VALUE));
}

#[test]
fn already_set_is_kept_by_default_and_replaced_on_request() {
    let f = fx();
    let p = plan(&f);
    apply(&f, &p, vec![("kc", Answer::text("old")), ("gh", Answer::text("old")), ("vc", Answer::text("old"))]);

    let p2 = plan(&f);
    assert!(p2.fields.iter().all(|pf| pf.existing == Existing::Set));
    let r = apply(&f, &p2, vec![("kc", Answer::text("new")), ("gh", Answer::text("new")), ("vc", Answer::text("new"))]);
    for k in ["kc", "gh", "vc"] {
        assert_eq!(r.fields[k].action, Action::Kept, "{k}");
    }
    assert_eq!(read(&f.state.join("gh-value-API_KEY")), "old");

    let r = apply(&f, &p2, vec![("kc", replace("new")), ("gh", replace("new")), ("vc", replace("new"))]);
    assert_eq!(r.status, Status::Ok, "{}", r.to_json_line());
    for k in ["kc", "gh", "vc"] {
        assert_eq!(r.fields[k].action, Action::Updated, "{k}");
    }
    assert_eq!(f.kc.get("passdrop-test", "me").unwrap().unwrap().as_slice(), b"new");
    assert_eq!(read(&f.state.join("gh-value-API_KEY")), "new");
    assert_eq!(read(&f.state.join("vercel-value-API_KEY-production")), "new");
    assert!(read(&f.state.join("vercel-args.log")).contains("env add API_KEY production --sensitive --force"));
}

#[test]
fn a_value_that_appears_after_planning_is_not_overwritten() {
    let f = fx();
    let p = plan(&f);
    // Set elsewhere while the window was open.
    f.kc.set("passdrop-test", "me", b"meanwhile").unwrap();
    fs::create_dir_all(&f.state).unwrap();
    fs::write(f.state.join("gh-names"), "API_KEY\n").unwrap();
    fs::write(f.state.join("vercel-records"), "API_KEY production\n").unwrap();
    let r = apply(&f, &p, vec![("kc", Answer::text(VALUE)), ("gh", Answer::text(VALUE)), ("vc", Answer::text(VALUE))]);
    for k in ["kc", "gh", "vc"] {
        assert_eq!(r.fields[k].error, Some("exists"), "{k}");
    }
    assert_eq!(f.kc.get("passdrop-test", "me").unwrap().unwrap().as_slice(), b"meanwhile");
}

#[test]
fn tool_errors_are_reported_without_the_value() {
    let f = fx();
    let p = plan(&f);
    fs::create_dir_all(&f.state).unwrap();
    fs::write(f.state.join("FAIL"), "").unwrap();
    let r = apply(&f, &p, vec![("gh", Answer::text(VALUE))]);
    assert_eq!(r.fields["gh"].error, Some("tool_failed"));
    let msg = r.fields["gh"].message.clone().unwrap();
    assert!(msg.contains("<redacted>") && !msg.contains(VALUE), "{msg}");
}

#[test]
fn vercel_plain_values_in_the_listing_never_reach_the_result() {
    let f = fx();
    fs::create_dir_all(&f.state).unwrap();
    fs::write(f.state.join("vercel-records"), "OTHER production\nAPI_KEY development\n").unwrap();
    let p = plan(&f);
    let vc = p.fields.iter().find(|pf| pf.name == "vc").unwrap();
    assert_eq!(vc.existing, Existing::Set);
    let rep = serde_json::to_string(&run::check_report(&p, &f.s.env)).unwrap();
    let r = apply(&f, &p, vec![("vc", replace(VALUE))]);
    for out in [rep, r.to_json_line()] {
        assert!(!out.contains("PLAINVALUE"), "{out}");
    }
}

#[test]
fn dropped_originals_are_never_trashed_for_destinations_that_cant_be_read_back() {
    use std::cell::RefCell;
    let f = fx();
    let json = r#"{"title":"T","x-purpose":"P","properties":{
      "gh":{"title":"GH","writeOnly":true,"x-widget":"file","x-sink":{"kind":"gh-secret","name":"PEM","repo":"o/r"}},
      "kc":{"title":"KC","writeOnly":true,"x-widget":"file","x-sink":{"kind":"keychain","service":"s","account":"a"}}}}"#;
    let c = spec::parse(json, Registry::builtin());
    let p = run::plan_with(c.spec.unwrap(), &f.s.env, Registry::builtin(), &f.s.git_env()).unwrap();
    let pem = "-----BEGIN PRIVATE KEY-----\nx\n-----END PRIVATE KEY-----\n";
    let (a_path, b_path) = (f.s.root.join("a.pem"), f.s.root.join("b.pem"));
    fs::write(&a_path, pem).unwrap();
    fs::write(&b_path, pem).unwrap();
    let trashed: RefCell<Vec<PathBuf>> = RefCell::new(Vec::new());
    let trash = |p: &Path| -> Result<(), String> {
        trashed.borrow_mut().push(p.to_path_buf());
        Ok(())
    };
    let mut ga = Answer::file(&a_path);
    ga.trash_original = true;
    let mut ka = Answer::file(&b_path);
    ka.trash_original = true;
    let answers: IndexMap<String, Answer> = [("gh".to_string(), ga), ("kc".to_string(), ka)].into_iter().collect();
    let r = run::apply_full(&p, answers, &f.s.env, Registry::builtin(), &f.s.git_env(), Some(&trash));
    assert_eq!(r.fields["gh"].original, Some("kept"), "{}", r.to_json_line());
    assert_eq!(r.fields["kc"].original, Some("trashed"), "a verified keychain write may trash");
    assert_eq!(*trashed.borrow(), vec![b_path]);
}

#[test]
fn missing_tools_warn_at_plan_time() {
    let mut f = fx();
    f.s.env.tools.vercel = f.s.root.join("nope");
    let p = plan(&f);
    let vc = p.fields.iter().find(|pf| pf.name == "vc").unwrap();
    assert_eq!(vc.existing, Existing::Unknown);
    assert!(p.warnings.iter().any(|w| w.code == "tool_missing"));
}
