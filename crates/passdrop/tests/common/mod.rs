#![allow(dead_code)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// Fake home + project under cargo's target tmp dir; the binary runs there with a
/// minimal environment, so it never sees the real home or the user's git config.
pub struct Sandbox {
    _dir: tempfile::TempDir,
    pub root: PathBuf,
    pub home: PathBuf,
    pub proj: PathBuf,
}

impl Sandbox {
    pub fn new() -> Sandbox {
        let dir = tempfile::Builder::new().prefix("cli-").tempdir_in(env!("CARGO_TARGET_TMPDIR")).unwrap();
        let root = fs::canonicalize(dir.path()).unwrap();
        let home = root.join("home");
        let proj = home.join("proj");
        fs::create_dir_all(&proj).unwrap();
        Sandbox { _dir: dir, root, home, proj }
    }

    pub fn write(&self, rel: &str, content: &str) -> PathBuf {
        let p = self.proj.join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(&p, content).unwrap();
        p
    }

    pub fn cmd(&self, args: &[&str]) -> Command {
        let mut c = Command::new(env!("CARGO_BIN_EXE_passdrop"));
        c.args(args)
            .current_dir(&self.proj)
            .env_clear()
            .env("PATH", std::env::var("PATH").unwrap())
            .env("HOME", &self.home)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CEILING_DIRECTORIES", &self.root);
        c
    }

    pub fn run(&self, args: &[&str]) -> Output {
        self.cmd(args).output().unwrap()
    }
}

/// stdout must be exactly one JSON line; returns it parsed.
pub fn one_json_line(out: &Output) -> serde_json::Value {
    let stdout = String::from_utf8(out.stdout.clone()).unwrap();
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(lines.len(), 1, "stdout must be exactly one line, got: {stdout:?}");
    serde_json::from_str(lines[0]).unwrap()
}

pub fn fixture() -> &'static Path {
    Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../core/tests/fixtures/design_example.json"))
}
