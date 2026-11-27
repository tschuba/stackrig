use std::{fs, io::Write, os::unix::fs::PermissionsExt, path::PathBuf, process::{Command, Output, Stdio}};

/// Fake homelab repo (one host `pi`, stack `a` with two images) and a fake `crane` that knows the tags 1.0.0, 1.0.1, 1.1.0, 2.0.0.
struct Env { root: PathBuf }

impl Env {
    fn new(name: &str) -> Env {
        let root = std::env::temp_dir().join(format!("strig-update-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        for d in ["bin", "host_vars/pi", "stacks/a"] { fs::create_dir_all(root.join(d)).unwrap(); }
        fs::write(root.join("inventory.yml"), "all:\n  children:\n    homelab:\n      hosts:\n        pi: { ansible_host: 127.0.0.1 }\n").unwrap();
        fs::write(root.join("host_vars/pi/stacks.yml"), "stacks:\n  - a\n").unwrap();
        fs::write(root.join("stacks/a/compose.yml"), "services:\n  x:\n    image: o/x:1.0.0\n  y:\n    image: o/y:1.0.0\n").unwrap();
        let crane = root.join("bin/crane");
        fs::write(&crane, "#!/bin/sh\ncase \"$1\" in\n ls) printf '1.0.0\\n1.0.1\\n1.1.0\\n2.0.0\\nlatest\\n';;\n digest) case \"$2\" in *:1.0.0|*:1.0.1|*:1.1.0|*:2.0.0|*:latest) echo sha256:abc;; *) echo 'MANIFEST_UNKNOWN' >&2; exit 1;; esac;;\nesac\n").unwrap();
        fs::set_permissions(&crane, fs::Permissions::from_mode(0o755)).unwrap();
        Env { root }
    }
    fn compose(&self) -> String { fs::read_to_string(self.root.join("stacks/a/compose.yml")).unwrap() }
    fn run(&self, args: &[&str], input: &str) -> Output {
        let mut c = Command::new(env!("CARGO_BIN_EXE_strig")).arg("update").args(args)
            .env("STRIG_HOME", &self.root).env("PATH", format!("{}:{}", self.root.join("bin").display(), std::env::var("PATH").unwrap()))
            .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
        c.stdin.take().unwrap().write_all(input.as_bytes()).unwrap();
        c.wait_with_output().unwrap()
    }
}
impl Drop for Env { fn drop(&mut self) { let _ = fs::remove_dir_all(&self.root); } }
fn err(o: &Output) -> String { String::from_utf8_lossy(&o.stderr).into_owned() }

#[test]
fn check_stays_in_major_and_hints_the_jump() {
    let e = Env::new("check");
    let o = e.run(&["--check", "a"], "");
    let s = String::from_utf8_lossy(&o.stdout).into_owned();
    assert!(s.contains("1.0.0 -> 1.1.0 (major available: 2.0.0, use --major)"), "{s}");
    assert!(e.compose().contains("o/x:1.0.0"));
}

#[test]
fn interactive_keep_custom_tag_and_bogus_tag_then_no_deploy() {
    let e = Env::new("inter");
    // x: bogus tag (asked again), then `n` keeps; y: custom tag 1.0.1; deploy: n
    let o = e.run(&["-i", "a"], "bogus\nn\n1.0.1\nn\n");
    let s = err(&o);
    assert!(s.contains("tag `bogus` not found"), "{s}");
    assert_eq!(e.compose(), "services:\n  x:\n    image: o/x:1.0.0\n  y:\n    image: o/y:1.0.1\n");
    assert_eq!(o.status.code(), Some(0), "{s}");
}

#[test]
fn interactive_quit_writes_nothing() {
    let e = Env::new("quit");
    let before = e.compose();
    let o = e.run(&["-i", "a"], "y\nq\n");
    assert_eq!(e.compose(), before);
    assert_eq!(o.status.code(), Some(1));
    assert_eq!(e.run(&["-i", "a"], "").status.code(), Some(1), "EOF aborts");
}

#[test]
fn check_prints_header_and_service_column() {
    let e = Env::new("hdr");
    let o = e.run(&["--check", "a"], "");
    let l: Vec<Vec<String>> = String::from_utf8_lossy(&o.stdout).lines().map(|x| x.split_whitespace().map(String::from).collect()).collect();
    assert_eq!(l[0][..4], ["STACK", "SERVICE", "IMAGE", "CHANGE"], "{l:?}");
    assert_eq!(l.iter().filter(|r| r[0] == "STACK").count(), 1, "header only once: {l:?}");
    assert_eq!(l[1][..3], ["a", "x", "o/x:1.0.0"], "{l:?}");
    assert_eq!(l[2][..3], ["a", "y", "o/y:1.0.0"], "{l:?}");
    // interactive prompts carry no header
    let o = e.run(&["-i", "a"], "q\n");
    assert!(!err(&o).contains("CHANGE") && err(&o).contains(" x "), "{}", err(&o));
}

#[test]
fn variable_versions_are_read_from_and_written_to_env() {
    let e = Env::new("var");
    let c = "services:\n  x:\n    image: o/x:${TAG}\n  y:\n    image: o/y:${TAG}\n  z:\n    image: o/z:${NOPE}\n  w:\n    image: o/w:${D:-1.0.0}\n";
    fs::write(e.root.join("stacks/a/compose.yml"), c).unwrap();
    let env = e.root.join("stacks/a/.env");
    fs::write(&env, "A=1\nTAG=1.0.0\n").unwrap();
    let s = String::from_utf8_lossy(&e.run(&["--check", "a"], "").stdout).into_owned();
    assert!(s.contains("o/x:${TAG}") && s.contains("1.0.0 -> 1.1.0") && s.contains("o/z:${NOPE}") && s.contains("skipped (variable)"), "{s}");
    assert_eq!(fs::read_to_string(&env).unwrap(), "A=1\nTAG=1.0.0\n", "check writes nothing");
    let o = e.run(&["a"], "");
    assert_eq!(fs::read_to_string(&env).unwrap(), "A=1\nTAG=1.1.0\nD=1.1.0\n", "{}", err(&o));
    assert_eq!(e.compose(), c, "compose stays");
}

#[test]
fn shared_variable_with_different_results_is_left_alone() {
    let e = Env::new("shared");
    fs::write(e.root.join("stacks/a/compose.yml"), "services:\n  x:\n    image: o/x:${T}\n  y:\n    image: o/y:${T}-alpine\n").unwrap();
    let env = e.root.join("stacks/a/.env");
    fs::write(&env, "T=1.0.0\n").unwrap();
    let o = e.run(&["a"], "");
    assert_eq!(fs::read_to_string(&env).unwrap(), "T=1.0.0\n");
    assert!(err(&o).contains("different results"), "{}", err(&o));
}

#[test]
fn unreadable_env_skips_in_check_and_fails_on_update() {
    let e = Env::new("vault");
    fs::write(e.root.join("stacks/a/compose.yml"), "services:\n  x:\n    image: o/x:${T}\n").unwrap();
    fs::write(e.root.join("stacks/a/.env"), "$ANSIBLE_VAULT;1.1;AES256\nabc\n").unwrap();
    // no ansible-vault in PATH-less env is not guaranteed, so either way decrypt must fail on this junk
    let o = e.run(&["--check", "a"], "");
    assert!(String::from_utf8_lossy(&o.stdout).contains(".env not readable") && o.status.success(), "{}", err(&o));
    let o = e.run(&["a"], "");
    assert!(err(&o).contains(".env not readable") && !o.status.success());
}

#[test]
fn include_exclude_limit_check_and_update_to_that_service() {
    let e = Env::new("svc");
    let o = e.run(&["--check", "a", "--include", "y"], "");
    let s = String::from_utf8_lossy(&o.stdout).into_owned();
    assert!(s.contains("o/y:1.0.0") && !s.contains("o/x:"), "{s}");
    // interactive: only y is asked (custom tag), x stays; deploy: n
    let o = e.run(&["-i", "--exclude", "x", "a"], "1.0.1\nn\n");
    assert_eq!(e.compose(), "services:\n  x:\n    image: o/x:1.0.0\n  y:\n    image: o/y:1.0.1\n", "{}", err(&o));
    // bad usage: unknown service, missing value
    assert!(err(&e.run(&["a", "--include", "nope"], "")).contains("matches no service"));
    let o = e.run(&["--check", "--all", "--exclude", "*/x"], "");
    assert!(!String::from_utf8_lossy(&o.stdout).contains("o/x:"));
    for args in [&["a", "--exclude"][..]] {
        assert_eq!(e.run(args, "").status.code(), Some(2), "{args:?}");
    }
}

#[test]
fn interactive_rejects_check_and_pin() {
    let e = Env::new("rej");
    for f in ["--check", "--pin"] { assert_eq!(e.run(&["-i", f, "a"], "").status.code(), Some(2), "{f}"); }
}

#[test]
fn ls_filters_and_rejects_bad_input() {
    let e = Env::new("lsfilter");
    fs::write(e.root.join("stacks/a/meta.yml"), "domains: [a.example.org]\n").unwrap();
    let run = |args: &[&str]| Command::new(env!("CARGO_BIN_EXE_strig")).args(args).env("STRIG_HOME", &e.root).output().unwrap();
    for (args, code) in [(&["ls", "a", "--show", "domains"][..], 0), (&["ls", "--host", "pi", "--show", "stack"], 0), (&["ls", "nope"], 1), (&["ls", "a", "nope"], 1), (&["ls", "a", "a", "--show", "stack"], 0), (&["ls", "--host", "pi", "--host", "pi", "--show", "stack"], 0), (&["ls", "--host", "x"], 1), (&["ls", "--bogus"], 1), (&["ls", "--show", "nope"], 1), (&["ls", "--show"], 1), (&["ls", "--show", "image", "--show", "image"], 1), (&["ls", "--show", "stack,service,image,tag,domains,auth"], 0), (&["domains"], 2)] {
        let o = run(args);
        assert_eq!(o.status.code(), Some(code), "{args:?}: {}", err(&o));
    }
    assert!(String::from_utf8_lossy(&run(&["ls", "a", "--show", "domains"]).stdout).contains("a.example.org"));
    let o = run(&["ls", "--show", "domains,auth"]);
    assert!(String::from_utf8_lossy(&o.stdout).starts_with("DOMAINS"), "{}", err(&o));
}

#[test]
fn ls_sorts_by_shown_columns_in_order() {
    let e = Env::new("lssort");
    let o = Command::new(env!("CARGO_BIN_EXE_strig")).args(["ls", "--show", "tag,service"]).env("STRIG_HOME", &e.root).output().unwrap();
    let t = String::from_utf8_lossy(&o.stdout).into_owned();
    assert!(t.starts_with("TAG"), "{t}");
    assert_eq!(t.lines().skip(1).map(|l| l.split_whitespace().last().unwrap()).collect::<Vec<_>>(), ["x", "y"], "{t}");
    let o = Command::new(env!("CARGO_BIN_EXE_strig")).args(["ls", "--show", "tag,service"]).env("STRIG_HOME", &e.root).output().unwrap();
    assert_eq!(String::from_utf8_lossy(&o.stdout).lines().nth(2).unwrap().trim(), "y", "tag 1.0.0 repeats, so it is blanked");
}

#[test]
fn plain_update_changes_files_but_does_not_deploy() {
    let e = Env::new("nodeploy");
    let o = e.run(&["a"], "");
    let s = String::from_utf8_lossy(&o.stdout).into_owned();
    assert!(e.compose().contains("o/x:1.1.0") && s.contains("NOT deployed") && !s.contains("[2/4]"), "{s}{}", err(&o));
    assert_eq!(e.run(&["--check", "--deploy", "a"], "").status.code(), Some(2));
}
