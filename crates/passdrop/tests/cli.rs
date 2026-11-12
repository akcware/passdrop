//! Default-build CLI behaviour. No window is opened by these tests.

mod common;

use common::{Sandbox, fixture, one_json_line};

#[test]
fn validate_and_check_the_design_example() {
    let s = Sandbox::new();
    let f = fixture().to_str().unwrap();
    let out = s.run(&["validate", f]);
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(one_json_line(&out)["status"], "valid");
    let out = s.run(&["check", f]);
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(one_json_line(&out)["fields"]["openai_key"]["set"], "not_set");
}

#[test]
fn invalid_spec_and_usage_errors_exit_2() {
    let s = Sandbox::new();
    let bad = s.write("bad.json", r#"{"title":"T","x-purpose":"P","properties":{"k":{"title":"K"}}}"#);
    let out = s.run(&["request", bad.to_str().unwrap()]);
    assert_eq!(out.status.code(), Some(2));
    assert_eq!(one_json_line(&out)["errors"][0]["code"], "no_destination");
    let out = s.run(&["request"]);
    assert_eq!(out.status.code(), Some(2));
    assert_eq!(one_json_line(&out)["status"], "usage");
    let out = s.run(&["request", "x.json", "--timeout", "5"]);
    assert_eq!(one_json_line(&out)["status"], "usage");
}

#[test]
fn ssh_sessions_get_no_gui_without_opening_a_window() {
    let s = Sandbox::new();
    let out = s.cmd(&["request", fixture().to_str().unwrap()]).env("SSH_CONNECTION", "10.0.0.1 1 10.0.0.2 22").output().unwrap();
    assert_eq!(out.status.code(), Some(12));
    assert_eq!(one_json_line(&out)["status"], "no_gui");
}

/// The test-only paths don't exist in a default build.
#[cfg(not(feature = "test-harness"))]
#[test]
fn default_build_rejects_test_only_options() {
    let s = Sandbox::new();
    let f = fixture().to_str().unwrap();
    for opt in ["--test-input", "--screenshot"] {
        let out = s.run(&["request", f, opt, "x.json"]);
        assert_eq!(out.status.code(), Some(2), "{opt}");
        assert_eq!(one_json_line(&out)["status"], "usage");
    }
}
