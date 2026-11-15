use std::{fs, os::unix::fs::PermissionsExt, path::{Path, PathBuf}, process::{Command, Output}};

/// Scratch dir with a fake `docker` on PATH. The network `proxy` exists once the file `net` exists; `info` fails if `down` exists.
struct Env { root: PathBuf }

impl Env {
    fn new(name: &str) -> Env {
        let root = std::env::temp_dir().join(format!("strig-doctor-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("bin")).unwrap();
        let docker = root.join("bin/docker");
        fs::write(&docker, format!("#!/bin/sh\ncase \"$*\" in\n 'info '*) [ -f '{0}/down' ] && exit 1; echo 27.0;;\n 'compose version'*) echo 2.29;;\n 'network inspect'*) [ -f '{0}/net' ];;\n 'network create'*) touch '{0}/net';;\nesac\n", root.display())).unwrap();
        fs::set_permissions(&docker, fs::Permissions::from_mode(0o755)).unwrap();
        fs::write(root.join("net"), "").unwrap();
        Env { root }
    }
    fn stack(&self, name: &str, compose: &str) -> PathBuf {
        let d = self.root.join("stacks").join(name);
        fs::create_dir_all(&d).unwrap();
        fs::write(d.join("docker-compose.yml"), compose).unwrap();
        d
    }
    fn run(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_strig")).arg("doctor").args(args)
            .env("PATH", format!("{}:{}", self.root.join("bin").display(), std::env::var("PATH").unwrap())).output().unwrap()
    }
}
impl Drop for Env { fn drop(&mut self) { let _ = fs::remove_dir_all(&self.root); } }

fn out(o: &Output) -> String { format!("{}{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr)) }
fn mode(p: &Path) -> u32 { fs::metadata(p).unwrap().permissions().mode() & 0o777 }
const PLAIN: &str = "services:\n  a:\n    image: x\n";

#[test]
fn healthy_host_and_stack_pass() {
    let e = Env::new("ok");
    let d = e.stack("web", PLAIN);
    let o = e.run(&[d.to_str().unwrap()]);
    assert!(o.status.success(), "{}", out(&o));
    assert!(out(&o).contains("OK       web"), "{}", out(&o));
    assert!(!d.join(".env.generated").exists(), "doctor must not touch the stack");
}

#[test]
fn docker_unreachable_is_an_error() {
    let e = Env::new("down");
    fs::write(e.root.join("down"), "").unwrap();
    let o = e.run(&[]);
    assert_eq!(o.status.code(), Some(1));
    assert!(out(&o).contains("ERROR    Docker reachable"), "{}", out(&o));
}

#[test]
fn missing_network_is_an_error_and_fix_creates_it() {
    let e = Env::new("net");
    fs::remove_file(e.root.join("net")).unwrap();
    let o = e.run(&[]);
    assert_eq!(o.status.code(), Some(1));
    assert!(out(&o).contains("network `proxy` is missing"), "{}", out(&o));
    let o = e.run(&["fix"]);
    assert!(o.status.success(), "{}", out(&o));
    assert!(out(&o).contains("FIXED    network `proxy` created"), "{}", out(&o));
    assert!(e.root.join("net").exists());
}

#[test]
fn undefined_variable_is_an_error() {
    let e = Env::new("var");
    let d = e.stack("web", "services:\n  a:\n    image: x\n    environment:\n      - P=${STRIG_TEST_UNDEFINED_VAR}\n");
    let o = e.run(&[d.to_str().unwrap()]);
    assert_eq!(o.status.code(), Some(1));
    assert!(out(&o).contains("variables without a value") && out(&o).contains("STRIG_TEST_UNDEFINED_VAR"), "{}", out(&o));
    fs::write(d.join(".env"), "STRIG_TEST_UNDEFINED_VAR=1\n").unwrap();
    fs::set_permissions(d.join(".env"), fs::Permissions::from_mode(0o600)).unwrap();
    assert!(e.run(&[d.to_str().unwrap()]).status.success(), "defined in .env must pass");
}

#[test]
fn new_random_values_warn_but_do_not_fail() {
    let e = Env::new("warn");
    let d = e.stack("web", "services:\n  a:\n    image: x\n    environment:\n      - P=${SERVICE_PASSWORD_DB}\n");
    let o = e.run(&[d.to_str().unwrap()]);
    assert!(o.status.success(), "{}", out(&o));
    assert!(out(&o).contains("WARNING  web: `up` would create new random values") && out(&o).contains("SERVICE_PASSWORD_DB"), "{}", out(&o));
    assert!(!d.join(".env.generated").exists());
}

#[test]
fn locked_stack_is_an_error() {
    let e = Env::new("lock");
    let d = e.stack("web", PLAIN);
    fs::write(d.join(".lock"), "because\n").unwrap();
    let o = e.run(&[d.to_str().unwrap()]);
    assert_eq!(o.status.code(), Some(1));
    assert!(out(&o).contains("locked (.lock): because"), "{}", out(&o));
}

#[test]
fn world_readable_env_is_an_error_and_fix_sets_600() {
    let e = Env::new("perm");
    let d = e.stack("web", PLAIN);
    fs::write(d.join(".env"), "A=1\n").unwrap();
    fs::set_permissions(d.join(".env"), fs::Permissions::from_mode(0o644)).unwrap();
    let o = e.run(&[d.to_str().unwrap()]);
    assert_eq!(o.status.code(), Some(1));
    assert!(out(&o).contains(".env is readable by others"), "{}", out(&o));
    assert_eq!(mode(&d.join(".env")), 0o644, "no fix without `fix`");
    let o = e.run(&["fix", d.to_str().unwrap()]);
    assert!(o.status.success(), "{}", out(&o));
    assert!(out(&o).contains("FIXED    web: .env set to mode 600"), "{}", out(&o));
    assert_eq!(mode(&d.join(".env")), 0o600);
}

#[test]
fn all_checks_every_stack_dir_and_fails_if_any_fails() {
    let e = Env::new("all");
    e.stack("good", PLAIN);
    let bad = e.stack("bad", PLAIN);
    fs::write(bad.join(".lock"), "x").unwrap();
    let o = e.run(&["--all", e.root.join("stacks").to_str().unwrap()]);
    assert_eq!(o.status.code(), Some(1));
    let t = out(&o);
    assert!(t.contains("OK       good") && t.contains("ERROR    bad: locked"), "{t}");
}

#[test]
fn all_without_dir_is_a_usage_error() {
    let o = Env::new("usage").run(&["--all"]);
    assert_eq!(o.status.code(), Some(2));
    assert!(out(&o).contains("usage: strig doctor"), "{}", out(&o));
}
