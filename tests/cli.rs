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

const IMPORT_LOCK: &str = "import coolify: waiting for `strig migrate coolify` (render/up would create new random values)";

/// Fake Coolify host with services `svc` (uuid u1), `hand` (u2) and `new` (u3, never imported); `svc` has a running container
/// holding the generated password and mounting /srv/data. Service env file: one user var, one generated secret.
fn coolify_migrate_env(name: &str) -> (Env, PathBuf) {
    let e = Env::new(name);
    e.script("sudo", "shift\ncase \"$*\" in\n *psql*) printf 'u1,svc\\nu2,hand\\nu3,new\\n';;\n *Mounts*) echo 'u1 /srv/data ';;\n 'docker ps -a'*u1*) echo svc-c-1;;\n 'docker inspect svc-c-1'*) printf 'SERVICE_PASSWORD_DB=secret\\nMY_VAR=1\\n';;\n 'cat '*/u1/.env|'cat '*/u2/.env|'cat '*/u3/.env) printf 'MY_VAR=1\\nSERVICE_PASSWORD_DB=secret\\n';;\nesac");
    let t = e.root.join("target");
    let compose = "services:\n  a:\n    image: x\n    environment:\n      - P=${SERVICE_PASSWORD_DB}\n    volumes:\n      - /srv/data:/d\n";
    for n in ["svc", "hand"] {
        fs::create_dir_all(t.join(n)).unwrap();
        fs::write(t.join(n).join("docker-compose.yml"), compose).unwrap();
    }
    fs::write(t.join("svc/.env"), "MY_VAR=1\n").unwrap();
    fs::write(t.join("svc/.lock"), IMPORT_LOCK).unwrap();
    fs::write(t.join("hand/.lock"), "do not touch").unwrap();
    (e, t)
}

#[test]
fn migrate_takes_over_secrets_unlocks_and_warns_about_bind_mounts() {
    let (e, t) = coolify_migrate_env("migrate");
    let o = e.run(&["migrate", "coolify", "local", t.to_str().unwrap()]);
    let s = out(&o);
    // `new` was never imported -> error -> exit 1, but the others are still processed
    assert_eq!(o.status.code(), Some(1), "{s}");
    assert!(s.contains("svc: 1 lines of .env.generated taken over, render OK, 2/2 values found in running containers"), "{s}");
    assert!(s.contains("WARNING bind mount /srv/data is used by a running container (project u1)"), "{s}");
    assert!(s.contains("ERROR    new: no docker-compose.yml, run `import coolify` first"), "{s}");
    assert_eq!(fs::read_to_string(t.join("svc/.env.generated")).unwrap(), "SERVICE_PASSWORD_DB=secret\n");
    assert!(!t.join("svc/.lock").exists(), "import lock must be removed");
}

#[test]
fn migrate_only_touches_the_named_stacks_and_rejects_unknown_names() {
    let (e, t) = coolify_migrate_env("only");
    let o = e.run(&["migrate", "coolify", "local", t.to_str().unwrap(), "--only", "svc"]);
    let s = out(&o);
    assert_eq!(o.status.code(), Some(0), "{s}");
    assert!(s.contains("svc:") && !s.contains("new:") && !s.contains("hand:"), "{s}");
    let o = e.run(&["migrate", "coolify", "local", t.to_str().unwrap(), "--only", "nope"]);
    assert!(out(&o).contains("--only nope: no such"), "{}", out(&o));
}

#[test]
fn migrate_skips_a_hand_made_lock() {
    let (e, t) = coolify_migrate_env("handlock");
    let s = out(&e.run(&["migrate", "coolify", "local", t.to_str().unwrap()]));
    assert!(s.contains("hand: SKIPPED, locked (.lock): do not touch"), "{s}");
    assert_eq!(fs::read_to_string(t.join("hand/.lock")).unwrap(), "do not touch");
    assert!(!t.join("hand/.env.generated").exists());
}

#[test]
fn migrate_is_idempotent_and_never_overwrites_generated_values() {
    let (e, t) = coolify_migrate_env("idem");
    fs::write(t.join("svc/.env.generated"), "SERVICE_PASSWORD_DB=mine\n").unwrap();
    let o = e.run(&["migrate", "coolify", "local", t.to_str().unwrap()]);
    assert!(out(&o).contains("ERROR    svc"), "write_new must refuse to overwrite: {}", out(&o));
    assert_eq!(fs::read_to_string(t.join("svc/.env.generated")).unwrap(), "SERVICE_PASSWORD_DB=mine\n");
}

#[test]
fn non_local_host_goes_over_ssh() {
    let e = Env::new("ssh");
    let log = e.root.join("ssh.log");
    e.script("ssh", &format!("echo \"$@\" >> '{}'; exit 1", log.display()));
    let o = e.run(&["import", "coolify", "me@pi", e.root.join("t").to_str().unwrap()]);
    assert_eq!(o.status.code(), Some(1));
    let l = fs::read_to_string(&log).unwrap();
    assert!(l.starts_with("-o BatchMode=yes me@pi sudo -n docker exec coolify-db"), "{l}");
}

#[test]
fn route_derives_everything_from_the_hosts() {
    let e = Env::new("route");
    let d = e.stack("app", "services:\n  web:\n    image: x\n    environment:\n      - SERVICE_FQDN_WEB_80=app.example.net\n");
    let t = e.root.join("proxy");
    fs::create_dir_all(t.join("dynamic")).unwrap();
    e.script("sudo", "shift\nexec \"$@\"");
    e.script("ip", "echo '1.1.1.1 via 192.168.178.1 dev eth0 src 192.168.178.50 uid 1000'");
    e.script("docker", &format!("case \"$*\" in\n *Mounts*) echo '[{{\"Source\":\"{}\",\"Destination\":\"/traefik\"}}]';;\n *Config.Cmd*) echo '[\"--providers.file.directory=/traefik/dynamic/\"]';;\n *Ports*) echo '{{\"80/tcp\":[{{\"HostIp\":\"0.0.0.0\",\"HostPort\":\"8080\"}}]}}';;\nesac", t.display()));
    let dir = d.to_str().unwrap();
    let o = e.run(&["route", dir, "--dry-run"]);
    let y = String::from_utf8_lossy(&o.stdout);
    assert!(o.status.success() && y.contains("url: 'http://192.168.178.50:8080'") && y.contains("Host(`app.example.net`)"), "{y} {}", String::from_utf8_lossy(&o.stderr));
    assert!(!t.join("dynamic/stackrig-app.yaml").exists(), "dry run must not write");
    let o = e.run(&["route", dir]);
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    assert_eq!(fs::read_to_string(t.join("dynamic/stackrig-app.yaml")).unwrap(), y);
    // overrides skip the lookups
    let o = e.run(&["route", dir, "--upstream", "http://10.0.0.9:1", "--dest", t.join("dynamic").to_str().unwrap(), "--resolver", "r2"]);
    assert!(o.status.success());
    let f = fs::read_to_string(t.join("dynamic/stackrig-app.yaml")).unwrap();
    assert!(f.contains("http://10.0.0.9:1") && f.contains("certResolver: r2"));
}

#[test]
fn behind_proxy_env_renders_plain_http_labels() {
    let e = Env::new("behind");
    let d = e.stack("app", "services:\n  web:\n    image: x\n    environment:\n      - SERVICE_FQDN_WEB_80=app.example.net\n");
    let run = |v: Option<&str>| {
        let mut c = Command::new(env!("CARGO_BIN_EXE_strig"));
        c.args(["render", d.to_str().unwrap()]);
        if let Some(v) = v { c.env("STRIG_BEHIND_PROXY", v); }
        String::from_utf8_lossy(&c.output().unwrap().stdout).into_owned()
    };
    assert!(run(None).contains("certresolver"));
    let y = run(Some("1"));
    assert!(y.contains("routers.http-app-web.rule") && !y.contains("certresolver") && !y.contains("redirect"), "{y}");
}

#[test]
fn inner_proxy_container_switches_labels_and_proxy_up_runs_on_the_host() {
    let e = Env::new("proxy");
    let d = e.stack("app", "services:\n  web:\n    image: x\n    environment:\n      - SERVICE_FQDN_WEB_80=app.example.net\n");
    let log = e.root.join("docker.log");
    e.script("docker", &format!("echo \"$*\" >> {}\ncase \"$*\" in 'ps -aq'*) echo abc123;; esac\ncat >/dev/null 2>&1 </dev/null", log.display()));
    let o = e.run(&["render", d.to_str().unwrap()]);
    assert!(!String::from_utf8_lossy(&o.stdout).contains("certresolver"), "label container must switch the mode");
    e.script("docker", &format!("echo \"$*\" >> {}\ncat >> {}.stdin", log.display(), log.display()));
    assert!(String::from_utf8_lossy(&e.run(&["render", d.to_str().unwrap()]).stdout).contains("certresolver"));
    assert_eq!(e.run(&["proxy", "up"]).status.code(), Some(1), "--trust is required");
    let o = e.run(&["proxy", "up", "--trust", "10.0.0.2", "--port", "8081"]);
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    let sent = fs::read_to_string(format!("{}.stdin", log.display())).unwrap();
    assert!(sent.contains("stackrig.proxy=true") && sent.contains("'8081:80'") && sent.contains("10.0.0.2/32"), "{sent}");
}

#[test]
fn proxy_up_warns_when_the_port_changes() {
    let e = Env::new("proxyport");
    e.script("docker", "case \"$*\" in 'inspect'*) echo '{\"80/tcp\":[{\"HostPort\":\"8080\"}]}';; esac\ncat >/dev/null");
    let err = |a: &[&str]| String::from_utf8_lossy(&e.run(a).stderr).into_owned();
    assert!(err(&["proxy", "up", "--trust", "10.0.0.2", "--port", "9000"]).contains("port changed 8080 -> 9000"));
    assert!(!err(&["proxy", "up", "--trust", "10.0.0.2"]).contains("port changed"));
}
