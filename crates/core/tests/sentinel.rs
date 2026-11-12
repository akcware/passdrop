//! The core promise at library level: a value never shows up in results, error
//! messages or Debug output, on success or failure paths.

mod common;

use std::fs;

use common::Sandbox;
use indexmap::IndexMap;
use passdrop_core::registry::Registry;
use passdrop_core::result::RunResult;
use passdrop_core::run::{self, Answer};
use passdrop_core::sinks::{dotenv, file};
use passdrop_core::spec;

const SENTINEL: &str = "SENTINEL7f3a9c";

fn run_one(s: &Sandbox, json: &str, answer: Answer) -> RunResult {
    let c = spec::parse(json, Registry::builtin());
    assert!(c.is_valid(), "{:?}", c.errors);
    let p = run::plan_with(c.spec.unwrap(), &s.env, Registry::builtin(), &s.git_env()).unwrap();
    let mut a = IndexMap::new();
    a.insert("k".to_string(), answer);
    run::apply_with(&p, a, &s.env, Registry::builtin(), &s.git_env())
}

fn assert_clean(what: &str, text: &str) {
    assert!(!text.contains(SENTINEL), "{what} leaked the value: {text}");
}

const DOTENV_SPEC: &str = r#"{"title":"T","x-purpose":"P","properties":{
  "k":{"title":"K","writeOnly":true,"x-sink":{"kind":"dotenv","path":".env","key":"K"}}}}"#;

#[test]
fn answers_debug_is_redacted() {
    assert_clean("Answer Debug", &format!("{:?}", Answer::text(SENTINEL)));
}

#[test]
fn success_result_is_clean() {
    let s = Sandbox::new();
    let r = run_one(&s, DOTENV_SPEC, Answer::text(SENTINEL));
    assert_clean("result JSON", &r.to_json_line());
    assert_clean("result Debug", &format!("{r:?}"));
    assert!(s.read(".env").contains(SENTINEL));
}

#[test]
fn unquotable_failure_is_clean() {
    let s = Sandbox::new();
    let r = run_one(&s, DOTENV_SPEC, Answer::text(format!("it's ${SENTINEL}")));
    assert_eq!(r.fields["k"].error, Some("unquotable"));
    assert_clean("result JSON", &r.to_json_line());
    assert_clean("result Debug", &format!("{r:?}"));
}

#[test]
fn unparseable_file_with_a_secret_inside_is_clean() {
    let s = Sandbox::new();
    s.write(".env", &format!("OTHER={SENTINEL}\nBROKEN='{SENTINEL}\n"));
    let e = dotenv::write(&s.path(".env"), "K", "v", false).unwrap_err();
    assert_clean("SinkError Display", &e.to_string());
    assert_clean("SinkError Debug", &format!("{e:?}"));
    let r = run_one(&s, DOTENV_SPEC, Answer::text("v"));
    assert_clean("result JSON", &r.to_json_line());
}

#[test]
fn tracked_file_failure_is_clean() {
    let s = Sandbox::new();
    s.git(&["init", "-q"]);
    s.write(".env", "A=1\n");
    s.git(&["add", ".env"]);
    s.git(&["commit", "-q", "-m", "x"]);
    let r = run_one(&s, DOTENV_SPEC, Answer::text(SENTINEL));
    assert_eq!(r.fields["k"].error, Some("tracked_file"));
    assert_clean("result JSON", &r.to_json_line());
}

#[test]
fn oversized_dropped_file_is_clean() {
    let s = Sandbox::new();
    let big = s.root.join("big.p8");
    fs::write(&big, SENTINEL.repeat(10)).unwrap();
    let e = file::read_limited(&big, 16).unwrap_err();
    assert_clean("SinkError Display", &e.to_string());
    let json = r#"{"title":"T","x-purpose":"P","properties":{
      "k":{"title":"K","writeOnly":true,"x-widget":"file","x-max-bytes":16,
           "x-sink":{"kind":"file","dir":"keys","name":"original"}}}}"#;
    let r = run_one(&s, json, Answer::file(&big));
    assert_eq!(r.fields["k"].error, Some("too_large"));
    assert_clean("result JSON", &r.to_json_line());
}

#[test]
fn spec_errors_never_echo_values_from_a_default() {
    // A spec can't carry a secret default, and the error says so without quoting it.
    let json = format!(
        r#"{{"title":"T","x-purpose":"P","properties":{{"k":{{"title":"K","writeOnly":true,"default":"{SENTINEL}","x-sink":{{"kind":"dotenv","path":".env","key":"K"}}}}}}}}"#
    );
    let c = spec::parse(&json, Registry::builtin());
    assert!(!c.is_valid());
    for e in &c.errors {
        assert_clean("spec issue", &e.message);
    }
}
