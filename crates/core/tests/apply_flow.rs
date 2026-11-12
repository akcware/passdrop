mod common;

use std::fs;

use common::Sandbox;
use indexmap::IndexMap;
use passdrop_core::git::{self, GitStatus};
use passdrop_core::registry::Registry;
use passdrop_core::result::{Action, Status};
use passdrop_core::run::{self, Answer, Existing, Plan};
use passdrop_core::spec::{self, Spec};
use passdrop_core::validate::Warning;

const FIXTURE: &str = include_str!("fixtures/design_example.json");
const P8: &str = "-----BEGIN PRIVATE KEY-----\nMIGTAgEAMBMGByqGSM49AgEGCCqGSM49AwEHBHkwdwIBAQQgFAKEFAKEFAKE\n-----END PRIVATE KEY-----\n";

fn reg() -> &'static Registry {
    Registry::builtin()
}

fn parse_ok(json: &str) -> Spec {
    let c = spec::parse(json, reg());
    assert!(c.is_valid(), "{:?}", c.errors);
    c.spec.unwrap()
}

fn plan(s: &Sandbox, json: &str) -> Plan {
    run::plan_with(parse_ok(json), &s.env, reg(), &s.git_env()).unwrap_or_else(|e| panic!("{e:?}"))
}

fn answers(items: Vec<(&str, Answer)>) -> IndexMap<String, Answer> {
    items.into_iter().map(|(k, v)| (k.to_string(), v)).collect()
}

#[test]
fn design_doc_example_matches_the_fixture() {
    let doc = fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../DESIGN.md")).unwrap();
    let block = doc
        .split("```json\n")
        .skip(1)
        .map(|b| b.split("```").next().unwrap())
        .find(|b| b.contains("\"properties\""))
        .expect("DESIGN.md has a spec example");
    let a: serde_json::Value = serde_json::from_str(block).unwrap();
    let b: serde_json::Value = serde_json::from_str(FIXTURE).unwrap();
    assert_eq!(a, b, "DESIGN.md §6.4 and tests/fixtures/design_example.json drifted");
}

#[test]
fn design_example_is_valid_without_warnings() {
    let c = spec::parse(FIXTURE, reg());
    assert!(c.is_valid(), "{:?}", c.errors);
    assert!(c.warnings.is_empty(), "{:?}", c.warnings);
}

#[test]
fn full_flow_writes_every_destination() {
    let s = Sandbox::new();
    let downloads = s.home.join("Downloads");
    fs::create_dir_all(&downloads).unwrap();
    let p8 = downloads.join("AuthKey_ABC123XYZ.p8");
    fs::write(&p8, P8).unwrap();

    let p = plan(&s, FIXTURE);
    let r = run::apply_with(
        &p,
        answers(vec![
            ("asc_key", Answer::file(&p8)),
            ("issuer_id", Answer::text("57246542-96fe-1a63-e053-0824d011072a")),
            ("review_phone", Answer::text(" +90 555 000 00 00\n")),
            ("openai_key", Answer::text("sk-proj-test-not-real-0000")),
            ("session_secret", Answer::skip()),
        ]),
        &s.env,
        reg(),
        &s.git_env(),
    );
    assert_eq!(r.status, Status::Ok, "{}", r.to_json_line());

    let f = &r.fields;
    assert_eq!(f["asc_key"].action, Action::Created);
    assert_eq!(f["asc_key"].reference.as_deref(), Some("file:~/.appstoreconnect/private_keys/AuthKey_ABC123XYZ.p8"));
    assert_eq!(f["asc_key"].filename.as_deref(), Some("AuthKey_ABC123XYZ.p8"));
    assert_eq!(f["asc_key"].original, Some("kept"));
    assert_eq!(f["issuer_id"].action, Action::Echoed);
    assert_eq!(f["issuer_id"].value.as_deref(), Some("57246542-96fe-1a63-e053-0824d011072a"));
    assert_eq!(f["review_phone"].action, Action::Created);
    assert_eq!(f["review_phone"].warnings, vec![Warning::Trimmed]);
    assert_eq!(f["openai_key"].action, Action::Created);
    assert!(f["openai_key"].warnings.is_empty());
    assert_eq!(f["session_secret"].action, Action::Generated);
    assert_eq!(f["session_secret"].reference.as_deref(), Some("env-file:.env.local#SESSION_SECRET"));

    let key_file = s.home.join(".appstoreconnect/private_keys/AuthKey_ABC123XYZ.p8");
    assert_eq!(fs::read_to_string(&key_file).unwrap(), P8);
    assert_eq!(s.mode(&key_file), 0o600);
    let env = s.read(".env.local");
    assert!(env.contains("REVIEW_PHONE='+90 555 000 00 00'\n"));
    assert!(env.contains("OPENAI_API_KEY=sk-proj-test-not-real-0000\n"));
    let secret_line = env.lines().find(|l| l.starts_with("SESSION_SECRET=")).unwrap();
    assert_eq!(secret_line.len(), "SESSION_SECRET=".len() + 64);
    assert_eq!(s.mode(&s.path(".env.local")), 0o600);

    // The result carries no secret values.
    let line = r.to_json_line();
    for secret in ["+90 555", "sk-proj-test", &secret_line["SESSION_SECRET=".len()..], "MIGTAgEA"] {
        assert!(!line.contains(secret), "result leaked {secret}: {line}");
    }

    // A second run sees the values and reports unchanged / kept.
    let p2 = plan(&s, FIXTURE);
    let set: Vec<(&str, Existing)> = p2.fields.iter().map(|f| (f.name.as_str(), f.existing)).collect();
    assert!(set.contains(&("review_phone", Existing::Set)) && set.contains(&("session_secret", Existing::Set)));
    let mut phone = Answer::text("+90 555 000 00 00");
    phone.replace_existing = true;
    let r2 = run::apply_with(
        &p2,
        answers(vec![
            ("asc_key", Answer::file(&p8)),
            ("issuer_id", Answer::text("57246542-96fe-1a63-e053-0824d011072a")),
            ("review_phone", phone),
            ("openai_key", Answer::text("sk-proj-a-different-value")),
            ("session_secret", Answer::skip()),
        ]),
        &s.env,
        reg(),
        &s.git_env(),
    );
    assert_eq!(r2.status, Status::Ok, "{}", r2.to_json_line());
    assert_eq!(r2.fields["asc_key"].action, Action::Unchanged);
    assert_eq!(r2.fields["review_phone"].action, Action::Unchanged);
    // Keep is the default: without Replace, an already-set value is not touched.
    assert_eq!(r2.fields["openai_key"].action, Action::Kept);
    assert_eq!(r2.fields["session_secret"].action, Action::Kept);
    assert!(s.read(".env.local").contains("OPENAI_API_KEY=sk-proj-test-not-real-0000\n"));
    assert!(s.read(".env.local").contains(secret_line));
}

const ONE_DOTENV: &str = r#"{"title":"T","x-purpose":"P","properties":{
  "k":{"title":"K","writeOnly":true,"x-sink":{"kind":"dotenv","path":".env.local","key":"K"}}}}"#;

#[test]
fn keep_is_the_default_and_replace_overwrites() {
    let s = Sandbox::new();
    s.write(".env.local", "K=old\n");
    let p = plan(&s, ONE_DOTENV);
    assert_eq!(p.fields[0].existing, Existing::Set);
    let r = run::apply_with(&p, answers(vec![("k", Answer::text("new-value"))]), &s.env, reg(), &s.git_env());
    assert_eq!(r.fields["k"].action, Action::Kept);
    assert_eq!(s.read(".env.local"), "K=old\n");

    let mut a = Answer::text("new-value");
    a.replace_existing = true;
    let r = run::apply_with(&p, answers(vec![("k", a)]), &s.env, reg(), &s.git_env());
    assert_eq!(r.fields["k"].action, Action::Updated);
    assert_eq!(s.read(".env.local"), "K=new-value\n");
}

#[test]
fn a_value_that_appears_after_planning_is_not_overwritten() {
    let s = Sandbox::new();
    let p = plan(&s, ONE_DOTENV);
    assert_eq!(p.fields[0].existing, Existing::NotSet);
    s.write(".env.local", "K=written-meanwhile\n");
    let r = run::apply_with(&p, answers(vec![("k", Answer::text("new-value"))]), &s.env, reg(), &s.git_env());
    assert_eq!(r.fields["k"].error, Some("exists"));
    assert_eq!(s.read(".env.local"), "K=written-meanwhile\n");
}

#[test]
fn a_dropped_file_never_silently_replaces_a_same_named_file() {
    let s = Sandbox::new();
    let json = r#"{"title":"T","x-purpose":"P","properties":{
      "k":{"title":"K","writeOnly":true,"x-widget":"file","x-sink":{"kind":"file","dir":"keys","name":"original"}}}}"#;
    s.write("keys/AuthKey_A.p8", "old contents");
    let dropped = s.root.join("AuthKey_A.p8");
    fs::write(&dropped, P8).unwrap();
    let p = plan(&s, json);
    assert_eq!(p.fields[0].existing, Existing::Unknown);
    let r = run::apply_with(&p, answers(vec![("k", Answer::file(&dropped))]), &s.env, reg(), &s.git_env());
    assert_eq!(r.fields["k"].error, Some("exists"));
    assert_eq!(s.read("keys/AuthKey_A.p8"), "old contents");

    let mut a = Answer::file(&dropped);
    a.replace_existing = true;
    let r = run::apply_with(&p, answers(vec![("k", a)]), &s.env, reg(), &s.git_env());
    assert_eq!(r.fields["k"].action, Action::Updated);
    assert_eq!(s.read("keys/AuthKey_A.p8"), P8);
}

#[test]
fn tracked_destination_needs_confirmation() {
    let s = Sandbox::new();
    s.git(&["init", "-q"]);
    s.write(".env.local", "A=1\n");
    s.git(&["add", ".env.local"]);
    s.git(&["commit", "-q", "-m", "init"]);
    let p = plan(&s, ONE_DOTENV);
    assert_eq!(p.fields[0].git, Some(GitStatus::Tracked));

    let r = run::apply_with(&p, answers(vec![("k", Answer::text("value-1"))]), &s.env, reg(), &s.git_env());
    assert_eq!(r.fields["k"].error, Some("tracked_file"));
    assert_eq!(r.status, Status::Failed);
    assert_eq!(s.read(".env.local"), "A=1\n");

    let mut a = Answer::text("value-1");
    a.allow_tracked = true;
    let r = run::apply_with(&p, answers(vec![("k", a)]), &s.env, reg(), &s.git_env());
    assert_eq!(r.fields["k"].action, Action::Created);
}

#[test]
fn untracked_destination_can_be_added_to_gitignore() {
    let s = Sandbox::new();
    s.git(&["init", "-q"]);
    let p = plan(&s, ONE_DOTENV);
    assert_eq!(p.fields[0].git, Some(GitStatus::Untracked));
    let mut a = Answer::text("value-1");
    a.add_to_gitignore = true;
    let r = run::apply_with(&p, answers(vec![("k", a)]), &s.env, reg(), &s.git_env());
    assert_eq!(r.status, Status::Ok, "{}", r.to_json_line());
    assert_eq!(s.read(".gitignore"), "/.env.local\n");
    assert_eq!(git::status_with(&s.path(".env.local"), &s.git_env()), GitStatus::Ignored);
}

#[test]
fn outside_a_repo_git_status_is_not_repo() {
    let s = Sandbox::new();
    let p = plan(&s, ONE_DOTENV);
    assert_eq!(p.fields[0].git, Some(GitStatus::NotRepo));
}

#[test]
fn empty_dot_git_does_not_crash() {
    let s = Sandbox::new();
    fs::create_dir_all(s.proj.join(".git")).unwrap();
    let p = plan(&s, ONE_DOTENV);
    assert!(matches!(p.fields[0].git, Some(GitStatus::NotRepo | GitStatus::Unknown)));
    let r = run::apply_with(&p, answers(vec![("k", Answer::text("value-1"))]), &s.env, reg(), &s.git_env());
    assert_eq!(r.status, Status::Ok);
}

#[test]
fn remote_sinks_fail_cleanly_and_status_is_partial() {
    let mut s = Sandbox::new();
    // Never the real gh: a path that doesn't exist.
    s.env.tools.gh = s.root.join("no-such-gh");
    let json = r#"{"title":"T","x-purpose":"P","properties":{
      "a":{"title":"A","writeOnly":true,"x-sink":{"kind":"dotenv","path":".env","key":"A"}},
      "b":{"title":"B","writeOnly":true,"x-sink":{"kind":"gh-secret","name":"B"}}}}"#;
    let p = plan(&s, json);
    let r = run::apply_with(
        &p,
        answers(vec![("a", Answer::text("value-a")), ("b", Answer::text("value-b"))]),
        &s.env,
        reg(),
        &s.git_env(),
    );
    assert_eq!(r.status, Status::Partial);
    assert_eq!(r.fields["b"].error, Some("tool_missing"));
    assert_eq!(r.exit_code(), 20);
    // The plan already warned that gh is missing.
    assert!(p.warnings.iter().any(|w| w.code == "tool_missing"));
}

#[test]
fn skipping_required_fails_and_optional_skips() {
    let s = Sandbox::new();
    let json = r#"{"title":"T","x-purpose":"P","required":["a"],"properties":{
      "a":{"title":"A","writeOnly":true,"x-sink":{"kind":"dotenv","path":".env","key":"A"}},
      "b":{"title":"B","writeOnly":true,"x-sink":{"kind":"dotenv","path":".env","key":"B"}}}}"#;
    let p = plan(&s, json);
    let r = run::apply_with(&p, answers(vec![("a", Answer::text("  "))]), &s.env, reg(), &s.git_env());
    assert_eq!(r.fields["a"].error, Some("missing_required"));
    assert_eq!(r.fields["b"].action, Action::Skipped);
    assert_eq!(r.status, Status::Failed);
}

#[test]
fn plan_rejects_refused_and_duplicate_destinations() {
    let s = Sandbox::new();
    let refused = r#"{"title":"T","x-purpose":"P","properties":{
      "a":{"title":"A","writeOnly":true,"x-sink":{"kind":"dotenv","path":"~/.zshrc","key":"A"}}}}"#;
    let e = run::plan_with(parse_ok(refused), &s.env, reg(), &s.git_env()).err().unwrap();
    assert_eq!(e[0].code, "destination_refused");

    let dup = r#"{"title":"T","x-purpose":"P","properties":{
      "a":{"title":"A","writeOnly":true,"x-sink":{"kind":"dotenv","path":".env","key":"A"}},
      "b":{"title":"B","writeOnly":true,"x-sink":{"kind":"dotenv","path":"./.env","key":"A"}}}}"#;
    let e = run::plan_with(parse_ok(dup), &s.env, reg(), &s.git_env()).err().unwrap();
    assert_eq!(e[0].code, "duplicate_destination");
}

#[test]
fn destination_changed_between_plan_and_write_is_refused() {
    let s = Sandbox::new();
    let p = plan(&s, ONE_DOTENV);
    fs::create_dir_all(s.home.join(".ssh")).unwrap();
    fs::write(s.home.join(".ssh/x"), "").unwrap();
    std::os::unix::fs::symlink(s.home.join(".ssh/x"), s.path(".env.local")).unwrap();
    let r = run::apply_with(&p, answers(vec![("k", Answer::text("value-1"))]), &s.env, reg(), &s.git_env());
    assert_eq!(r.fields["k"].error, Some("destination_refused"));
    assert_eq!(fs::read_to_string(s.home.join(".ssh/x")).unwrap(), "");
}

#[test]
fn check_report_shows_state_without_values() {
    let s = Sandbox::new();
    s.write(".env.local", "K=already-set-value\n");
    let p = plan(&s, ONE_DOTENV);
    let rep = run::check_report(&p, &s.env);
    assert_eq!(rep["k"].set, Existing::Set);
    let json = serde_json::to_string(&rep).unwrap();
    assert!(!json.contains("already-set-value"));
    assert!(json.contains("env-file:.env.local#K"));
}

#[test]
fn originals_are_trashed_only_after_a_verified_write_and_never_when_they_are_the_destination() {
    use std::cell::RefCell;
    use std::path::{Path, PathBuf};

    let s = Sandbox::new();
    let json = r#"{"title":"T","x-purpose":"P","properties":{
      "k":{"title":"K","writeOnly":true,"x-widget":"file","x-sink":{"kind":"file","dir":"keys","name":"original"}}}}"#;
    let trashed: RefCell<Vec<PathBuf>> = RefCell::new(Vec::new());
    let trash = |p: &Path| -> Result<(), String> {
        trashed.borrow_mut().push(p.to_path_buf());
        Ok(())
    };

    // 1. A verified write moves the dropped original.
    let dropped = s.root.join("AuthKey_T.p8");
    fs::write(&dropped, P8).unwrap();
    let p = plan(&s, json);
    let mut a = Answer::file(&dropped);
    a.trash_original = true;
    let r = run::apply_full(&p, answers(vec![("k", a)]), &s.env, reg(), &s.git_env(), Some(&trash));
    assert_eq!(r.fields["k"].action, Action::Created);
    assert_eq!(r.fields["k"].original, Some("trashed"));
    assert_eq!(*trashed.borrow(), vec![dropped.clone()]);

    // 2. Dropping the destination itself: unchanged, and never trashed.
    let dest = s.path("keys/AuthKey_T.p8");
    let dt = p.fields[0].drop_target(&s.env, &dest, &s.git_env()).unwrap();
    assert!(dt.is_source && dt.exists);
    let mut a = Answer::file(&dest);
    a.trash_original = true;
    let r = run::apply_full(&p, answers(vec![("k", a)]), &s.env, reg(), &s.git_env(), Some(&trash));
    assert_eq!(r.fields["k"].action, Action::Unchanged);
    assert_eq!(r.fields["k"].original, Some("kept"));
    assert_eq!(trashed.borrow().len(), 1);
    assert!(dest.exists());

    // 3. A failed write (same name, different bytes, no Replace) never trashes.
    let other_dir = s.root.join("elsewhere");
    fs::create_dir_all(&other_dir).unwrap();
    let other = other_dir.join("AuthKey_T.p8");
    fs::write(&other, "different").unwrap();
    let dt = p.fields[0].drop_target(&s.env, &other, &s.git_env()).unwrap();
    assert!(!dt.is_source && dt.exists);
    let mut a = Answer::file(&other);
    a.trash_original = true;
    let r = run::apply_full(&p, answers(vec![("k", a)]), &s.env, reg(), &s.git_env(), Some(&trash));
    assert_eq!(r.fields["k"].error, Some("exists"));
    assert_eq!(r.fields["k"].original, Some("kept"));
    assert_eq!(trashed.borrow().len(), 1);

    // 4. Without a trash function the original stays and the result says so.
    let fresh = s.root.join("AuthKey_U.p8");
    fs::write(&fresh, P8).unwrap();
    let mut a = Answer::file(&fresh);
    a.trash_original = true;
    let r = run::apply_with(&p, answers(vec![("k", a)]), &s.env, reg(), &s.git_env());
    assert_eq!(r.fields["k"].original, Some("kept"));
    assert_eq!(r.fields["k"].warnings, vec![Warning::TrashFailed]);
}

/// Live-test regression (2026-11-12): a PNG dropped on a `.p8` field was copied and its
/// original moved to the Trash. A file that doesn't look like the expected one is only
/// trashed with explicit consent (`trash_unexpected`).
#[test]
fn a_wrong_file_type_is_never_trashed_without_explicit_consent() {
    use std::cell::RefCell;
    use std::path::{Path, PathBuf};

    let s = Sandbox::new();
    let json = r#"{"title":"T","x-purpose":"P","properties":{
      "k":{"title":"K","writeOnly":true,"x-widget":"file","x-accept":[".p8"],
           "x-sink":{"kind":"file","dir":"keys","name":"original"}}}}"#;
    let trashed: RefCell<Vec<PathBuf>> = RefCell::new(Vec::new());
    let trash = |p: &Path| -> Result<(), String> {
        trashed.borrow_mut().push(p.to_path_buf());
        Ok(())
    };
    let p = plan(&s, json);

    let photo = s.root.join("06-photo.png");
    fs::write(&photo, b"\x89PNG\r\n\x1a\nnot a key").unwrap();
    let mut a = Answer::file(&photo);
    a.trash_original = true;
    let r = run::apply_full(&p, answers(vec![("k", a)]), &s.env, reg(), &s.git_env(), Some(&trash));
    assert_eq!(r.fields["k"].action, Action::Created);
    assert_eq!(r.fields["k"].original, Some("kept"));
    assert!(r.fields["k"].warnings.contains(&Warning::Extension));
    assert!(trashed.borrow().is_empty());
    assert!(photo.exists());

    // A key with the right extension but no PEM header is also "unexpected".
    let bad_key = s.root.join("AuthKey_BAD.p8");
    fs::write(&bad_key, "not pem").unwrap();
    let mut a = Answer::file(&bad_key);
    a.trash_original = true;
    let r = run::apply_full(&p, answers(vec![("k", a)]), &s.env, reg(), &s.git_env(), Some(&trash));
    assert_eq!(r.fields["k"].original, Some("kept"));
    assert!(trashed.borrow().is_empty());

    // With explicit consent it is moved.
    let photo2 = s.root.join("07-photo.png");
    fs::write(&photo2, b"png").unwrap();
    let mut a = Answer::file(&photo2);
    a.trash_original = true;
    a.trash_unexpected = true;
    let r = run::apply_full(&p, answers(vec![("k", a)]), &s.env, reg(), &s.git_env(), Some(&trash));
    assert_eq!(r.fields["k"].original, Some("trashed"));
    assert_eq!(*trashed.borrow(), vec![photo2]);
}
