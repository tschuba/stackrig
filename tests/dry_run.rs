use std::{fs, os::unix::fs::PermissionsExt, path::Path, process::Command};

/// `migrate --dry-run` must run the real code (lock removed, .env.generated written) but only in a copy: the target stays untouched.
#[test]
fn migrate_dry_run_leaves_target_untouched() {
    let root = std::env::temp_dir().join(format!("strig-dry-{}", std::process::id()));
    let (bin, target) = (root.join("bin"), root.join("target"));
    fs::create_dir_all(&bin).unwrap();
    fs::create_dir_all(target.join("svc")).unwrap();
    // fake `sudo -n ...` standing in for the Pi: one service `svc` (uuid u1), one generated value, nothing running
    let sudo = bin.join("sudo");
    fs::write(&sudo, "#!/bin/sh\nshift\ncase \"$*\" in\n *psql*) echo u1,svc;;\n 'cat '*) echo SERVICE_PASSWORD_DB=secret;;\nesac\n").unwrap();
    fs::set_permissions(&sudo, fs::Permissions::from_mode(0o755)).unwrap();
    fs::write(target.join("svc/docker-compose.yml"), "services:\n  a:\n    image: x\n    environment:\n      - P=${SERVICE_PASSWORD_DB}\n").unwrap();
    let lock = target.join("svc/.lock");
    let lock_text = "import coolify: waiting for `strig migrate coolify` (render/up would create new random values)";
    fs::write(&lock, lock_text).unwrap();

    let out = Command::new(env!("CARGO_BIN_EXE_strig")).args(["migrate", "coolify", "local"]).arg(&target).arg("--dry-run")
        .env("PATH", format!("{}:{}", bin.display(), std::env::var("PATH").unwrap())).output().unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("svc: 1 lines of .env.generated taken over"), "{stdout}{}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(fs::read_to_string(&lock).unwrap(), lock_text, "lock must survive a dry run");
    assert!(!Path::new(&target.join("svc/.env.generated")).exists());
    fs::remove_dir_all(&root).unwrap();
}

/// `import --dry-run` must report like a real run but create nothing below the target.
#[test]
fn import_dry_run_writes_nothing() {
    let root = std::env::temp_dir().join(format!("strig-dry-import-{}", std::process::id()));
    let (bin, target) = (root.join("bin"), root.join("target"));
    fs::create_dir_all(&bin).unwrap();
    // fake `sudo -n ...`: no applications, one service `svc` (uuid u1) with a compose and a user variable
    let sudo = bin.join("sudo");
    fs::write(&sudo, "#!/bin/sh\nshift\ncase \"$*\" in\n *'from applications'*) ;;\n *'from services order'*) echo u1,svc;;\n *docker_compose_raw*) printf 'services:\\n  a:\\n    image: x\\n';;\n 'cat '*) echo MY_VAR=1;;\nesac\n").unwrap();
    fs::set_permissions(&sudo, fs::Permissions::from_mode(0o755)).unwrap();
    let run = |extra: &[&str]| Command::new(env!("CARGO_BIN_EXE_strig")).args(["import", "coolify", "local"]).arg(&target).args(extra)
        .env("PATH", format!("{}:{}", bin.display(), std::env::var("PATH").unwrap())).output().unwrap();

    let out = run(&["--dry-run"]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("svc: .env: 1 lines"), "{stdout}{}", String::from_utf8_lossy(&out.stderr));
    assert!(!target.exists(), "dry run must not create the target");

    // control: the same call without the flag does write (keeps the test above honest)
    assert!(run(&[]).status.success());
    assert!(target.join("svc/docker-compose.yml").exists());
    fs::remove_dir_all(&root).unwrap();
}
