//! git/gh helpers for `strig update --commit|--branch|--single-branch|--push|--pr` (plain `git` in the homelab repo).
use std::{path::Path, process::Command};

fn run(root: &Path, prog: &str, args: &[&str]) -> Result<String, String> {
    let o = Command::new(prog).current_dir(root).args(args).output().map_err(|e| format!("{prog}: {e}"))?;
    let (so, se) = (String::from_utf8_lossy(&o.stdout).trim().to_string(), String::from_utf8_lossy(&o.stderr).trim().to_string());
    if o.status.success() { Ok(so) } else { Err(format!("{prog} {}: {}", args.join(" "), if se.is_empty() { so } else { se })) }
}
pub(crate) fn git(root: &Path, args: &[&str]) -> Result<String, String> { run(root, "git", args) }

pub(crate) fn current(root: &Path) -> Result<String, String> { git(root, &["rev-parse", "--abbrev-ref", "HEAD"]) }

/// Default branch of the repo: `origin/HEAD`, else `gh`.
pub(crate) fn default_branch(root: &Path) -> Result<String, String> {
    if let Ok(b) = git(root, &["symbolic-ref", "--short", "refs/remotes/origin/HEAD"]) { return Ok(b.trim_start_matches("origin/").to_string()); }
    run(root, "gh", &["repo", "view", "--json", "defaultBranchRef", "--jq", ".defaultBranchRef.name"]).ok().filter(|b| !b.is_empty())
        .ok_or("cannot determine the default branch (no origin/HEAD, `gh repo view` failed)".into())
}

/// Branch flags need: on the default branch, not behind origin, and none of `branches` existing (locally or on origin).
pub(crate) fn preflight(root: &Path, def: &str, branches: &[String]) -> Result<(), String> {
    let cur = current(root)?;
    if cur != def { return Err(format!("--branch/--single-branch need the default branch `{def}` checked out (on `{cur}`)")); }
    git(root, &["fetch", "origin", def])?;
    if git(root, &["rev-list", "--count", &format!("HEAD..origin/{def}")])? != "0" { return Err(format!("`{def}` is behind origin/{def}; pull first")); }
    for b in branches {
        if git(root, &["rev-parse", "--verify", "--quiet", &format!("refs/heads/{b}")]).is_ok() || !git(root, &["ls-remote", "--heads", "origin", b])?.is_empty() {
            return Err(format!("branch `{b}` already exists"));
        }
    }
    Ok(())
}

/// True if any of `files` has uncommitted changes (or is untracked).
pub(crate) fn dirty(root: &Path, files: &[String]) -> Result<bool, String> {
    let mut a = vec!["status", "--porcelain", "--"];
    a.extend(files.iter().map(String::as_str));
    Ok(!git(root, &a)?.is_empty())
}

/// Commits only `files`, so other staged changes stay out.
pub(crate) fn commit(root: &Path, files: &[String], msg: &str) -> Result<(), String> {
    let f: Vec<&str> = files.iter().map(String::as_str).collect();
    git(root, &[&["add", "--"], &f[..]].concat())?;
    git(root, &[&["commit", "-m", msg, "--"], &f[..]].concat()).map(|_| ())
}

pub(crate) fn push(root: &Path, branch: &str) -> Result<(), String> { git(root, &["push", "-u", "origin", branch]).map(|_| ()) }

pub(crate) fn pr(root: &Path, def: &str, branch: &str, title: &str, body: &str) -> Result<String, String> {
    run(root, "gh", &["pr", "create", "--base", def, "--head", branch, "--title", title, "--body", body])
}
