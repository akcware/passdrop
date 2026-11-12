mod common;

use std::fs;
use std::os::unix::fs::symlink;

use common::Sandbox;
use passdrop_core::guards::{self, Env, GuardError};

#[test]
fn relative_paths_resolve_against_cwd() {
    let s = Sandbox::new();
    assert_eq!(guards::resolve(&s.env, ".env.local").unwrap(), s.proj.join(".env.local"));
    assert_eq!(guards::resolve(&s.env, "~/proj/.env").unwrap(), s.proj.join(".env"));
}

#[test]
fn symlink_into_a_protected_directory_is_refused() {
    let s = Sandbox::new();
    fs::create_dir_all(s.home.join(".ssh")).unwrap();
    fs::write(s.home.join(".ssh/authorized_keys"), "x").unwrap();
    symlink(s.home.join(".ssh/authorized_keys"), s.proj.join(".env")).unwrap();
    assert!(matches!(guards::resolve(&s.env, ".env"), Err(GuardError::Refused(_))));
}

#[test]
fn dotdot_only_in_the_existing_part() {
    let s = Sandbox::new();
    fs::create_dir_all(s.proj.join("sub")).unwrap();
    assert_eq!(guards::resolve(&s.env, "sub/../.env").unwrap(), s.proj.join(".env"));
    assert_eq!(guards::resolve(&s.env, "missing/../.env"), Err(GuardError::DotDot));
}

#[test]
fn directories_and_foreign_files_are_refused() {
    let s = Sandbox::new();
    fs::create_dir_all(s.proj.join("d")).unwrap();
    assert_eq!(guards::resolve(&s.env, "d"), Err(GuardError::NotRegularFile));
    s.write(".env", "A=1\n");
    let other = Env::new(s.home.clone(), s.proj.clone(), s.env.uid + 1);
    assert_eq!(guards::resolve(&other, ".env"), Err(GuardError::WrongOwner));
}

#[test]
fn worktree_env_is_allowed_but_agent_config_is_not() {
    let s = Sandbox::new();
    fs::create_dir_all(s.proj.join(".claude/worktrees/feature-1a2b")).unwrap();
    assert!(guards::resolve(&s.env, ".claude/worktrees/feature-1a2b/.env").is_ok());
    assert!(matches!(guards::resolve(&s.env, ".claude/settings.json"), Err(GuardError::Refused(_))));
    assert!(matches!(
        guards::resolve(&s.env, ".claude/worktrees/feature-1a2b/.claude/settings.local.json"),
        Err(GuardError::Refused(_))
    ));
}

#[test]
fn home_dotfiles_are_refused() {
    let s = Sandbox::new();
    assert!(matches!(guards::resolve(&s.env, "~/.zshrc"), Err(GuardError::Refused(_))));
    assert!(matches!(guards::resolve(&s.env, "../.npmrc"), Err(GuardError::Refused(_))));
}
