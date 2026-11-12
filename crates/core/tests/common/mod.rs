#![allow(dead_code)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::Command;

use passdrop_core::guards::Env;

/// A fake home with a project inside, under cargo's target tmp dir (never the real home,
/// never /tmp, which the guards refuse). Git is isolated from the user's config and
/// can't see repositories above the sandbox.
pub struct Sandbox {
    _dir: tempfile::TempDir,
    pub root: PathBuf,
    pub home: PathBuf,
    pub proj: PathBuf,
    pub env: Env,
    ceiling: String,
}

impl Sandbox {
    pub fn new() -> Sandbox {
        let dir = tempfile::Builder::new().prefix("pd-").tempdir_in(env!("CARGO_TARGET_TMPDIR")).unwrap();
        let root = fs::canonicalize(dir.path()).unwrap();
        let home = root.join("home");
        let proj = home.join("proj");
        fs::create_dir_all(&proj).unwrap();
        let uid = unsafe { libc::geteuid() };
        let env = Env::new(home.clone(), proj.clone(), uid);
        let ceiling = root.display().to_string();
        Sandbox { _dir: dir, root, home, proj, env, ceiling }
    }

    pub fn git_env(&self) -> [(&str, &str); 3] {
        [("GIT_CONFIG_GLOBAL", "/dev/null"), ("GIT_CONFIG_NOSYSTEM", "1"), ("GIT_CEILING_DIRECTORIES", self.ceiling.as_str())]
    }

    pub fn git(&self, args: &[&str]) {
        let out = Command::new("git")
            .arg("-C")
            .arg(&self.proj)
            .args(args)
            .envs(self.git_env())
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@example.invalid")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@example.invalid")
            .output()
            .unwrap();
        assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    }

    pub fn path(&self, rel: &str) -> PathBuf {
        self.proj.join(rel)
    }

    pub fn write(&self, rel: &str, content: &str) -> PathBuf {
        let p = self.path(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(&p, content).unwrap();
        p
    }

    pub fn read(&self, rel: &str) -> String {
        fs::read_to_string(self.path(rel)).unwrap()
    }

    pub fn mode(&self, p: &std::path::Path) -> u32 {
        fs::metadata(p).unwrap().permissions().mode() & 0o777
    }
}
