mod common;

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::process::Command;

use common::Sandbox;
use passdrop_core::sinks::{WriteOutcome, dotenv};

const CORPUS: [&str; 14] = [
    "sk-proj_abc.123/x:y@z+w=",
    "",
    "with space",
    "a$HOME",
    "a${HOME}b",
    "back\\slash",
    "lit\\nchars",
    "dq\"quote",
    "hash # x",
    "line1\nline2",
    "trail ",
    "a`b",
    "it's",
    "it's a\nb",
];

#[test]
fn creates_new_file_with_mode_0600() {
    let s = Sandbox::new();
    let p = s.path(".env.local");
    assert_eq!(dotenv::write(&p, "KEY", "v1", false).unwrap(), WriteOutcome::Created);
    assert_eq!(s.read(".env.local"), "KEY=v1\n");
    assert_eq!(s.mode(&p), 0o600);
}

#[test]
fn replaces_in_place_and_keeps_every_other_byte() {
    let s = Sandbox::new();
    let original = "# top\r\nA=1\r\nexport KEY=old # c\r\nB='x\r\ny'\r\nKEY=dup\r\nC=3";
    let p = s.write(".env", original);
    assert_eq!(dotenv::write(&p, "KEY", "new value", true).unwrap(), WriteOutcome::Updated);
    assert_eq!(s.read(".env"), "# top\r\nA=1\r\nexport KEY='new value'\r\nB='x\r\ny'\r\nC=3");
}

#[test]
fn appends_and_fixes_a_missing_final_newline() {
    let s = Sandbox::new();
    let p = s.write(".env", "A=1");
    assert_eq!(dotenv::write(&p, "KEY", "v", false).unwrap(), WriteOutcome::Created);
    assert_eq!(s.read(".env"), "A=1\nKEY=v\n");
}

#[test]
fn same_value_is_unchanged_and_not_rewritten() {
    let s = Sandbox::new();
    let p = s.write(".env", "KEY='a b'\n");
    let before = fs::metadata(&p).unwrap().modified().unwrap();
    assert_eq!(dotenv::write(&p, "KEY", "a b", false).unwrap(), WriteOutcome::Unchanged);
    assert_eq!(fs::metadata(&p).unwrap().modified().unwrap(), before);
}

#[test]
fn a_different_value_is_not_overwritten_without_replace() {
    let s = Sandbox::new();
    let p = s.write(".env", "KEY=old\n");
    assert_eq!(dotenv::write(&p, "KEY", "new", false).unwrap_err().code(), "exists");
    assert_eq!(s.read(".env"), "KEY=old\n");
    // An empty value counts as not set.
    let p2 = s.write(".env2", "KEY=\n");
    assert_eq!(dotenv::write(&p2, "KEY", "new", false).unwrap(), WriteOutcome::Updated);
}

#[test]
fn same_value_in_another_spelling_is_rewritten_in_our_grammar() {
    // `"a\\b"` decodes to a\b under common dotenv rules, but Node reads it as a\\b.
    let s = Sandbox::new();
    let p = s.write(".env", "KEY=\"a\\\\b\"\n");
    assert_eq!(dotenv::write(&p, "KEY", "a\\b", false).unwrap(), WriteOutcome::Updated);
    assert_eq!(s.read(".env"), "KEY='a\\b'\n");
}

#[test]
fn keeps_the_existing_mode() {
    let s = Sandbox::new();
    let p = s.write(".env", "A=1\n");
    fs::set_permissions(&p, fs::Permissions::from_mode(0o644)).unwrap();
    dotenv::write(&p, "KEY", "v", false).unwrap();
    assert_eq!(s.mode(&p), 0o644);
}

#[test]
fn corpus_round_trips_or_is_refused_untouched() {
    let s = Sandbox::new();
    let p = s.path(".env");
    for (i, v) in CORPUS.iter().enumerate() {
        let key = format!("K{i}");
        dotenv::write(&p, &key, v, false).unwrap();
        assert_eq!(dotenv::read(&p, &key).unwrap().as_deref().map(String::as_str), Some(*v), "{key}");
    }
    let before = s.read(".env");
    for bad in ["it's $x", "it's \"q\"", "it's \\", "it's `x`"] {
        let e = dotenv::write(&p, "BAD", bad, false).unwrap_err();
        assert_eq!(e.code(), "unquotable");
    }
    assert_eq!(s.read(".env"), before);
}

#[test]
fn refuses_unterminated_and_encrypted_files_untouched() {
    let s = Sandbox::new();
    let p = s.write(".env", "A='open\nB=2\n");
    let e = dotenv::write(&p, "KEY", "v", false).unwrap_err();
    assert_eq!(e.code(), "unparseable");
    assert!(e.to_string().contains("line 1"));
    assert_eq!(s.read(".env"), "A='open\nB=2\n");

    let p2 = s.write(".env.enc", "DOTENV_PUBLIC_KEY=\"03ab\"\nA=\"encrypted:xyz\"\n");
    assert_eq!(dotenv::write(&p2, "KEY", "v", false).unwrap_err().code(), "encrypted_dotenv");
}

/// Real-loader check: Node reads back exactly what we wrote. Skipped when node is absent.
#[test]
fn node_reads_what_we_write() {
    if Command::new("node").arg("--version").output().is_err() {
        eprintln!("node not found; skipping");
        return;
    }
    let s = Sandbox::new();
    let p = s.path(".env.node");
    let keys: Vec<String> = (0..CORPUS.len()).map(|i| format!("K{i}")).collect();
    for (k, v) in keys.iter().zip(CORPUS) {
        dotenv::write(&p, k, v, false).unwrap();
    }
    let script = "process.loadEnvFile(process.argv[1]); const ks = JSON.parse(process.argv[2]); \
                  process.stdout.write(JSON.stringify(ks.map(k => process.env[k] ?? null)))";
    let out = Command::new("node")
        .args(["-e", script, p.to_str().unwrap(), &serde_json::to_string(&keys).unwrap()])
        .env_clear()
        .env("PATH", std::env::var("PATH").unwrap())
        .env("HOME", "/nonexistent")
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let got: Vec<Option<String>> = serde_json::from_slice(&out.stdout).unwrap();
    for ((k, v), g) in keys.iter().zip(CORPUS).zip(got) {
        assert_eq!(g.as_deref(), Some(v), "{k}");
    }
}
