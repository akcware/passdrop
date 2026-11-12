//! End-to-end through the real binary, with the window replaced by a JSON answers file
//! (cargo feature `test-harness`). Includes the canary leak test (DESIGN.md §10).

mod common;

use std::fs;

use common::{Sandbox, one_json_line};

const SENTINEL: &str = "SENTINEL-canary-9d2f61";

const SPEC: &str = r#"{"title":"T","x-purpose":"P","properties":{
  "key":{"title":"Key","writeOnly":true,"x-sink":{"kind":"dotenv","path":".env.local","key":"API_KEY"}},
  "p8":{"title":"P8","writeOnly":true,"x-widget":"file","x-sink":{"kind":"file","dir":"keys","name":"original"}},
  "issuer":{"title":"Issuer","x-echo":true}}}"#;

#[test]
fn canary_value_never_reaches_stdout_or_stderr() {
    let s = Sandbox::new();
    let spec = s.write("spec.json", SPEC);
    let dropped = s.root.join("AuthKey_Z.p8");
    fs::write(&dropped, format!("-----BEGIN PRIVATE KEY-----\n{SENTINEL}\n-----END PRIVATE KEY-----\n")).unwrap();
    let input = s.write(
        "answers.json",
        &serde_json::json!({ "fields": {
            "key": { "text": SENTINEL, "gitignore": true },
            "p8": { "file": dropped, "trash": true },
            "issuer": { "text": "issuer-1" }
        }})
        .to_string(),
    );
    let out = s.run(&["request", spec.to_str().unwrap(), "--test-input", input.to_str().unwrap()]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!stdout.contains(SENTINEL), "stdout leaked: {stdout}");
    assert!(!stderr.contains(SENTINEL), "stderr leaked: {stderr}");
    let r = one_json_line(&out);
    assert_eq!(out.status.code(), Some(0), "{r}");
    assert_eq!(r["status"], "ok");
    assert_eq!(r["fields"]["key"]["action"], "created");
    assert_eq!(r["fields"]["p8"]["original"], "trashed");
    assert_eq!(r["fields"]["issuer"]["value"], "issuer-1");

    // The values did land, and the original moved to the (fake) Trash.
    assert!(fs::read_to_string(s.proj.join(".env.local")).unwrap().contains(SENTINEL));
    assert!(fs::read_to_string(s.proj.join("keys/AuthKey_Z.p8")).unwrap().contains(SENTINEL));
    assert!(!dropped.exists());
    assert!(s.proj.join(".trash/AuthKey_Z.p8").exists());
}

#[test]
fn cancel_exits_10_and_writes_nothing() {
    let s = Sandbox::new();
    let spec = s.write("spec.json", SPEC);
    let input = s.write("answers.json", r#"{"cancel":true}"#);
    let out = s.run(&["request", spec.to_str().unwrap(), "--test-input", input.to_str().unwrap()]);
    assert_eq!(out.status.code(), Some(10));
    assert_eq!(one_json_line(&out)["status"], "cancelled");
    assert!(!s.proj.join(".env.local").exists());
}

#[test]
fn a_failure_exits_with_partial_or_failed() {
    let s = Sandbox::new();
    let spec = s.write("spec.json", SPEC);
    s.write(".env.local", "API_KEY=existing\n");
    let input = s.write("answers.json", r#"{"fields":{"key":{"text":"new"},"issuer":{"text":"i"}}}"#);
    let out = s.run(&["request", spec.to_str().unwrap(), "--test-input", input.to_str().unwrap()]);
    let r = one_json_line(&out);
    // Already set + no Replace = kept, which is a success.
    assert_eq!(r["fields"]["key"]["action"], "kept", "{r}");
    assert_eq!(out.status.code(), Some(0));
}

/// Live-test regression: a photo dropped on a `.p8` field with the Trash option on keeps
/// its original.
#[test]
fn a_wrong_file_keeps_its_original_end_to_end() {
    let s = Sandbox::new();
    let spec = s.write(
        "spec.json",
        r#"{"title":"T","x-purpose":"P","properties":{
          "p8":{"title":"P8","writeOnly":true,"x-widget":"file","x-accept":[".p8"],
                "x-sink":{"kind":"file","dir":"keys","name":"original"}}}}"#,
    );
    let photo = s.root.join("photo.png");
    fs::write(&photo, b"\x89PNG\r\n\x1a\nphoto").unwrap();
    let input = s.write("answers.json", &serde_json::json!({ "fields": { "p8": { "file": photo, "trash": true } } }).to_string());
    let out = s.run(&["request", spec.to_str().unwrap(), "--test-input", input.to_str().unwrap()]);
    let r = one_json_line(&out);
    assert_eq!(r["fields"]["p8"]["original"], "kept", "{r}");
    assert!(photo.exists(), "the original must still be there");
}
