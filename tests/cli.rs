use std::{fs, os::unix::fs::PermissionsExt, path::PathBuf, process::{Command, Output}};

/// Scratch dir with fake `docker`/`sudo` scripts on PATH.
struct Env { root: PathBuf }

impl Env {
    fn new(name: &str) -> Env {
        let root = std::env::temp_dir().join(format!("strig-cli-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("bin")).unwrap();
        Env { root }
    }
    fn script(&self, name: &str, body: &str) {
        let p = self.root.join("bin").join(name);
        fs::write(&p, format!("#!/bin/sh\n{body}\n")).unwrap();
        fs::set_permissions(&p, fs::Permissions::from_mode(0o755)).unwrap();
    }
    fn stack(&self, name: &str, compose: &str) -> PathBuf {
        let d = self.root.join("stacks").join(name);
        fs::create_dir_all(&d).unwrap();
        fs::write(d.join("docker-compose.yml"), compose).unwrap();
        d
    }
    fn run(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_strig")).args(args)
            .env("PATH", format!("{}:{}", self.root.join("bin").display(), std::env::var("PATH").unwrap())).output().unwrap()
    }
}
impl Drop for Env { fn drop(&mut self) { let _ = fs::remove_dir_all(&self.root); } }

fn out(o: &Output) -> String { format!("{}{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr)) }
const PLAIN: &str = "services:\n  a:\n    image: x\n";

#[test]
fn render_prints_yaml_with_project_name_and_proxy_network() {
    let e = Env::new("render");
    let d = e.stack("Web", "services:\n  a:\n    image: x\n    environment:\n      - SERVICE_FQDN_A_80=a.example.net\n");
    let o = e.run(&["render", d.to_str().unwrap()]);
    assert!(o.status.success(), "{}", out(&o));
    let y = out(&o);
    assert!(y.contains("name: web") && y.contains("external: true") && y.contains("Host(`a.example.net`)"), "{y}");
}

#[test]
fn render_and_up_refuse_a_locked_stack() {
    let e = Env::new("lock");
    let d = e.stack("web", PLAIN);
    fs::write(d.join(".lock"), "wait for migrate\n").unwrap();
    for cmd in ["render", "up"] {
        let o = e.run(&[cmd, d.to_str().unwrap()]);
        assert_eq!(o.status.code(), Some(1), "{cmd}");
        assert!(out(&o).contains("is locked (.lock): wait for migrate"), "{cmd}: {}", out(&o));
    }
    assert!(!d.join("docker-compose.generated.yml").exists(), "locked `up` must write nothing");
}

#[test]
fn render_without_compose_file_fails() {
    let e = Env::new("nocompose");
    let d = e.root.join("empty");
    fs::create_dir_all(&d).unwrap();
    let o = e.run(&["render", d.to_str().unwrap()]);
    assert_eq!(o.status.code(), Some(1));
    assert!(out(&o).contains("no compose file"), "{}", out(&o));
}

#[test]
fn render_all_reports_each_stack_and_fails_on_errors() {
    let e = Env::new("all");
    e.stack("good", PLAIN);
    let bad = e.stack("bad", PLAIN);
    fs::write(bad.join(".lock"), "x").unwrap();
    let o = e.run(&["render", "--all", e.root.join("stacks").to_str().unwrap()]);
    assert_eq!(o.status.code(), Some(1));
    let t = out(&o);
    assert!(t.contains("OK       good") && t.contains("ERROR    bad: error:") && t.contains("2 stacks, 1 with errors"), "{t}");
    assert_eq!(e.run(&["render", "--all", "/nonexistent-strig-root"]).status.code(), Some(1));
    assert_eq!(e.run(&["render", "--all"]).status.code(), Some(2));
}

#[test]
fn import_migrate_without_host_or_dir_is_a_usage_error() {
    let e = Env::new("usage");
    for args in [&["import", "coolify"][..], &["migrate", "coolify", "local"]] {
        let o = e.run(args);
        assert_eq!(o.status.code(), Some(2), "{args:?}");
        assert!(out(&o).contains("usage: strig import|migrate coolify"), "{args:?}: {}", out(&o));
    }
}

#[test]
fn up_hands_rendered_file_and_both_env_files_to_compose() {
    let e = Env::new("up");
    let d = e.stack("web", PLAIN);
    fs::write(d.join(".env"), "A=1\n").unwrap();
    let log = e.root.join("docker.log");
    e.script("docker", &format!("echo \"$@\" >> '{}'", log.display()));
    assert!(e.run(&["up", d.to_str().unwrap()]).status.success());
    let l = fs::read_to_string(&log).unwrap();
    let d = d.canonicalize().unwrap();
    assert!(l.contains("compose --project-directory") && l.contains("docker-compose.generated.yml"), "{l}");
    assert!(l.contains(&format!("--env-file {0}/.env --env-file {0}/.env.generated up -d", d.display())), "{l}");
    assert!(d.join("docker-compose.generated.yml").exists());
    // extra args replace the default -d
    fs::remove_file(&log).unwrap();
    assert!(e.run(&["up", d.to_str().unwrap(), "--build"]).status.success());
    let l = fs::read_to_string(&log).unwrap();
    assert!(l.trim_end().ends_with("up --build"), "{l}");
}

#[test]
fn up_passes_on_the_exit_code_of_docker() {
    let e = Env::new("upfail");
    let d = e.stack("web", PLAIN);
    e.script("docker", "exit 3");
    assert_eq!(e.run(&["up", d.to_str().unwrap()]).status.code(), Some(3));
}

/// Fake Coolify host: one application `app` (uuid a1) whose compose mounts a state file; `running` decides what `docker ps` says.
fn coolify_app_env(name: &str, running: bool) -> Env {
    let e = Env::new(name);
    let compose = "services:\\n  x-1:\\n    image: ghcr.io/a/b:latest\\n    volumes:\\n      - /data/coolify/applications/a1/state.json:/app/state.json\\n";
    e.script("sudo", &format!("shift\ncase \"$*\" in\n *'from applications'*) echo a1,app;;\n *'from services order'*) ;;\n 'docker ps'*) {};;\n 'cat '*docker-compose.yaml) printf '{compose}';;\n 'cat '*/.env) echo PORT=80;;\n 'cat '*state.json) echo '{{\"t\":1}}';;\nesac",
        if running { "echo cid" } else { ":" }));
    e
}

#[test]
fn import_copies_application_and_its_bind_mounted_file() {
    let e = coolify_app_env("app", false);
    let t = e.root.join("target");
    let o = e.run(&["import", "coolify", "local", t.to_str().unwrap()]);
    assert!(o.status.success(), "{}", out(&o));
    assert!(out(&o).contains("app (application): .env: 1 lines, 1 file(s) copied"), "{}", out(&o));
    assert!(fs::read_to_string(t.join("app/docker-compose.yml")).unwrap().contains("./state.json:/app/state.json"));
    assert_eq!(fs::read_to_string(t.join("app/.env")).unwrap(), "PORT=80\n");
    assert_eq!(fs::read_to_string(t.join("app/state.json")).unwrap().trim(), "{\"t\":1}");
}

#[test]
fn import_refuses_when_application_with_files_is_running() {
    let e = coolify_app_env("apprun", true);
    let t = e.root.join("target");
    let o = e.run(&["import", "coolify", "local", t.to_str().unwrap()]);
    assert_eq!(o.status.code(), Some(1));
    assert!(out(&o).contains("nothing imported: application(s) app still running"), "{}", out(&o));
    assert!(!t.join("app").exists(), "nothing may be written");
}
