use std::process::{Command, Output};

fn strig(args: &[&str]) -> Output { Command::new(env!("CARGO_BIN_EXE_strig")).args(args).output().unwrap() }
fn out(o: &Output) -> String { String::from_utf8_lossy(&o.stdout).into_owned() }
fn err(o: &Output) -> String { String::from_utf8_lossy(&o.stderr).into_owned() }

const CMDS: [&str; 12] = ["render", "up", "proxy", "route", "doctor", "import", "migrate", "ls", "domains", "drift", "deploy", "update"];

#[test]
fn overview_lists_all_commands() {
    for args in [&["--help"][..], &["-h"], &["help"]] {
        let o = strig(args);
        assert!(o.status.success(), "{args:?}");
        for c in CMDS { assert!(out(&o).contains(c), "{args:?} misses {c}"); }
    }
}

#[test]
fn no_args_and_unknown_command_print_help_to_stderr_with_exit_2() {
    for args in [&[][..], &["bogus"]] {
        let o = strig(args);
        assert_eq!(o.status.code(), Some(2), "{args:?}");
        assert!(err(&o).contains("USAGE:"), "{args:?}");
    }
}

#[test]
fn version() {
    for f in ["-V", "--version"] {
        assert_eq!(out(&strig(&[f])).trim(), concat!("strig ", env!("CARGO_PKG_VERSION")));
    }
}

#[test]
fn every_command_has_help_in_both_forms() {
    for c in CMDS {
        let (a, b, d) = (strig(&["help", c]), strig(&[c, "--help"]), strig(&[c, "-h"]));
        assert!(a.status.success() && b.status.success() && d.status.success(), "{c}");
        assert!(out(&a).starts_with("strig "), "{c}");
        assert_eq!(out(&a), out(&b), "{c}");
        assert_eq!(out(&a), out(&d), "{c}");
    }
}

#[test]
fn help_for_unknown_command_fails() {
    let o = strig(&["help", "foo"]);
    assert_eq!(o.status.code(), Some(2));
    assert!(err(&o).contains("unknown command `foo`"));
}

#[test]
fn help_does_not_touch_the_filesystem() {
    // `--help` must win over a dir argument: no stack is rendered, no error about a missing dir
    let o = strig(&["render", "/nonexistent-strig-dir", "--help"]);
    assert!(o.status.success() && out(&o).contains("strig render"), "{}", err(&o));
}
