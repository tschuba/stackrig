use std::{fs, os::unix::fs::PermissionsExt, path::PathBuf, process::Command};

/// Homelab repo with stacks b and a (listed b first), fake `ssh` that reports `docker ps`.
struct Env { root: PathBuf }
impl Env {
    fn new(name: &str) -> Env {
        let root = std::env::temp_dir().join(format!("strig-ls-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        for d in ["bin", "host_vars/pi", "stacks/a", "stacks/b"] { fs::create_dir_all(root.join(d)).unwrap(); }
        fs::write(root.join("inventory.yml"), "all:\n  children:\n    homelab:\n      hosts:\n        pi: { ansible_host: 127.0.0.1 }\n").unwrap();
        fs::write(root.join("host_vars/pi/stacks.yml"), "stacks:\n  - b\n  - a\n").unwrap();
        // b: services z, y in this order; y has two Host rules, labels as list and as map
        fs::write(root.join("stacks/b/compose.yml"), "services:\n  z:\n    image: o/z:2\n  y:\n    image: o/y:1\n    labels:\n      t.rule: Host(`y1.org`) || Host(`y2.org`)\n").unwrap();
        fs::write(root.join("stacks/b/meta.yml"), "domains: [y2.org, y1.org, extra.org]\n").unwrap();
        fs::write(root.join("stacks/a/compose.yml"), "services:\n  s:\n    image: o/s:1\n    labels:\n    - t.rule=Host(`a.org`)\n").unwrap();
        fs::write(root.join("stacks/a/meta.yml"), "domains: [a.org]\n").unwrap();
        let ssh = root.join("bin/ssh");
        // a: 2 of 2 running, b: 10 of 10 running
        fs::write(&ssh, format!("#!/bin/sh\n{}", "for i in 1 2; do echo 'a running'; done\nfor i in 1 2 3 4 5 6 7 8 9 10; do echo 'b running'; done\n")).unwrap();
        fs::set_permissions(&ssh, fs::Permissions::from_mode(0o755)).unwrap();
        Env { root }
    }
    fn ls(&self, args: &[&str]) -> Vec<Vec<String>> {
        let o = Command::new(env!("CARGO_BIN_EXE_strig")).arg("ls").args(args).env("STRIG_HOME", &self.root)
            .env("PATH", format!("{}:{}", self.root.join("bin").display(), std::env::var("PATH").unwrap())).output().unwrap();
        assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
        // columns are separated by 2+ spaces; a blank cell shows up as a missing field, so keep the raw lines instead
        String::from_utf8_lossy(&o.stdout).lines().map(|l| vec![l.to_string()]).collect()
    }
}
impl Drop for Env { fn drop(&mut self) { let _ = fs::remove_dir_all(&self.root); } }
fn words(rows: &[Vec<String>]) -> Vec<Vec<String>> { rows.iter().map(|r| r[0].split_whitespace().map(String::from).collect()).collect() }

#[test]
fn default_view_sorts_stacks_but_keeps_compose_order_of_services() {
    let w = words(&Env::new("default").ls(&["--host", "pi"]));
    // header, stack a, then stack b with z before y (compose order, not alphabetical)
    assert_eq!(w[1][0], "a");
    let b: Vec<&str> = w.iter().skip(2).filter_map(|r| r.iter().find(|x| *x == "z" || *x == "y").map(String::as_str)).collect();
    assert_eq!(b, ["z", "y"], "{w:?}");
}

#[test]
fn service_with_two_domains_gets_follow_up_line_and_unlabelled_domain_comes_last() {
    // default view: compose order (z, y), domains per meta.yml order, y's second domain on a follow-up line, domain without any label last
    let w = words(&Env::new("domains").ls(&["b", "--host", "pi"]));
    let flat: Vec<String> = w.iter().skip(1).map(|r| r.join(" ")).collect();
    assert_eq!(flat, ["b pi 10/10 up z o/z 2", "y o/y 1 y2.org no", "y1.org no", "extra.org no"].map(String::from), "{w:?}");
}

#[test]
fn show_sorts_by_columns_empty_last_and_drops_duplicates() {
    let w = words(&Env::new("sort").ls(&["--show", "tag,service"]));
    assert_eq!(w[1..].iter().map(|r| r.join(" ")).collect::<Vec<_>>(), ["1 s", "y", "2 z"], "{w:?}");
    let w = words(&Env::new("dup").ls(&["--show", "host"]));
    assert_eq!(w.len(), 2, "{w:?}");
}

#[test]
fn state_shows_running_over_all_containers_and_sorts_as_text() {
    let e = Env::new("state");
    let w = words(&e.ls(&["--show", "state,stack"]));
    assert_eq!(w[1..].iter().map(|r| r.join(" ")).collect::<Vec<_>>(), ["10/10 up b", "2/2 up a"], "{w:?}");
}

#[test]
fn show_without_state_does_not_call_ssh() {
    let e = Env::new("nossh");
    fs::write(e.root.join("bin/ssh"), "#!/bin/sh\nexit 1\n").unwrap();
    assert_eq!(words(&e.ls(&["--show", "stack"])).len(), 3);
}
