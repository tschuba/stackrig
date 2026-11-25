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
fn interactive_rejects_check_and_pin() {
    let e = Env::new("rej");
    for f in ["--check", "--pin"] { assert_eq!(e.run(&["-i", f, "a"], "").status.code(), Some(2), "{f}"); }
}

#[test]
fn domains_filters_by_stack_and_host() {
    let e = Env::new("domains");
    fs::write(e.root.join("stacks/a/meta.yml"), "domains: [a.example.org]\n").unwrap();
    let run = |args: &[&str]| Command::new(env!("CARGO_BIN_EXE_strig")).args(args).env("STRIG_HOME", &e.root).output().unwrap();
    for (args, code) in [(&["domains", "a"][..], 0), (&["domains", "--host", "pi"], 0), (&["domains", "nope"], 1), (&["domains", "--host", "x"], 1), (&["domains", "--bogus"], 1), (&["ls", "nope"], 1), (&["domains", "a", "nope"], 1), (&["domains", "a", "a"], 0), (&["domains", "--host", "pi", "--host", "pi"], 0), (&["ls", "--host", "x"], 1), (&["ls", "--bogus"], 1), (&["ls", "--show", "nope"], 1), (&["ls", "--show"], 1), (&["ls", "--show", "image", "--show", "image"], 1), (&["ls", "--show", "stack,service,image,tag,domains"], 0)] {
        let o = run(args);
        assert_eq!(o.status.code(), Some(code), "{args:?}: {}", err(&o));
    }
    assert!(String::from_utf8_lossy(&run(&["domains", "a"]).stdout).contains("a.example.org"));
    let o = run(&["ls", "--show", "domains,stack"]);
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
