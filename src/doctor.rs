use crate::{secrets::parse_env, LOCK};

/// Variables a compose text needs from the environment: `$X`, `${X}`, `${X:?..}`; not `$$X` or ones with a default (`:-`, `-`, `:+`, `+`).
/// shortcut: a variable inside the default of another (`${A:-${B}}`) counts as required; upgrade if that gives false alarms.
pub(crate) fn referenced_vars(text: &str) -> std::collections::BTreeSet<String> {
    let b = text.as_bytes();
    let (mut out, mut i) = (std::collections::BTreeSet::new(), 0);
    let name_end = |mut e: usize| { while e < b.len() && (b[e].is_ascii_alphanumeric() || b[e] == b'_') { e += 1; } e };
    while i < b.len() {
        if b[i] != b'$' { i += 1; continue; }
        match b.get(i + 1) {
            Some(b'$') => i += 2,
            Some(b'{') => {
                let e = name_end(i + 2);
                let rest = &text[e..];
                let default = [":-", "-", ":+", "+"].iter().any(|p| rest.starts_with(p));
                if e > i + 2 && !default { out.insert(text[i + 2..e].to_string()); }
                i = e.max(i + 2);
            }
            Some(c) if c.is_ascii_alphabetic() || *c == b'_' => { let e = name_end(i + 1); out.insert(text[i + 1..e].to_string()); i = e; }
            _ => i += 1,
        }
    }
    out
}

pub(crate) fn docker(args: &[&str]) -> Option<String> {
    std::process::Command::new("docker").args(args).output().ok().filter(|o| o.status.success()).map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
}

/// Host checks. With `fix`, creates the missing network `proxy`. Returns the number of problems left.
pub(crate) fn doctor_host(fix: bool) -> usize {
    let mut failed = 0;
    let mut check = |ok: bool, msg: String| { println!("{} {msg}", if ok { "OK      " } else { "ERROR   " }); if !ok { failed += 1; } };
    check(docker(&["info", "--format", "{{.ServerVersion}}"]).is_some(), "Docker reachable (docker info)".into());
    let compose = docker(&["compose", "version", "--short"]);
    check(compose.is_some(), format!("Docker Compose present{}", compose.map(|v| format!(" ({v})")).unwrap_or_default()));
    let net = ["network", "inspect", "proxy", "--format", "{{range .Containers}}{{.Name}} {{end}}"];
    match docker(&net) {
        Some(c) => check(true, format!("network `proxy` exists, containers in it: {}", if c.is_empty() { "none".into() } else { c })),
        None if fix && docker(&["network", "create", "proxy"]).is_some() => println!("FIXED    network `proxy` created"),
        None => check(false, "network `proxy` is missing (`doctor fix` or `docker network create proxy`)".into()),
    }
    failed
}

/// Checks one stack dir without touching it: renders a temp copy and compares. With `fix`, sets `.env*` to mode 600.
/// Prints `OK name` or one line per problem; returns the number of errors (warnings do not count).
pub(crate) fn doctor_stack(dir: &std::path::Path, fix: bool) -> usize {
    use std::os::unix::fs::PermissionsExt;
    let name = dir.file_name().unwrap().to_string_lossy().to_lowercase();
    let mut issues: Vec<(&str, String)> = vec![];
    let tmp = std::env::temp_dir().join(format!("homelab-doctor-{}", std::process::id())).join(&name);
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();
    for f in ["docker-compose.yml", "docker-compose.yaml", "compose.yml", "compose.yaml", ".env", ".env.generated"] {
        if dir.join(f).exists() { let _ = std::fs::copy(dir.join(f), tmp.join(f)); }
    }
    if let Ok(reason) = std::fs::read_to_string(dir.join(LOCK)) { issues.push(("ERROR", format!("locked ({LOCK}): {}", reason.trim()))); }
    let read = |p: std::path::PathBuf| std::fs::read_to_string(p).unwrap_or_default();
    let me = std::env::current_exe().expect("current_exe");
    let o = std::process::Command::new(me).arg("render").arg(&tmp).output().expect("spawn");
    if !o.status.success() {
        let err = String::from_utf8_lossy(&o.stderr);
        issues.push(("ERROR", format!("render: {}", err.lines().find(|l| !l.trim().is_empty()).unwrap_or("?"))));
    } else {
        let yaml = String::from_utf8_lossy(&o.stdout);
        let before: Vec<String> = parse_env(&read(dir.join(".env.generated"))).into_iter().map(|(k, _)| k).collect();
        let after = parse_env(&read(tmp.join(".env.generated")));
        let new: Vec<&str> = after.iter().map(|(k, _)| k.as_str()).filter(|k| !before.iter().any(|b| b == k))
            .filter(|k| k.starts_with("SERVICE_") && !k.starts_with("SERVICE_FQDN_") && !k.starts_with("SERVICE_URL_")).collect();
        if !new.is_empty() { issues.push(("WARNING", format!("`up` would create new random values (wrong for existing data): {}", new.join(" ")))); }
        let defined: std::collections::BTreeSet<String> = parse_env(&read(dir.join(".env"))).into_iter().chain(after).map(|(k, _)| k).collect();
        let missing: Vec<String> = referenced_vars(&yaml).into_iter().filter(|v| !defined.contains(v) && std::env::var_os(v).is_none()).collect();
        if !missing.is_empty() { issues.push(("ERROR", format!("variables without a value (neither in .env nor .env.generated): {}", missing.join(" ")))); }
    }
    let _ = std::fs::remove_dir_all(&tmp);
    for f in [".env", ".env.generated"] {
        let Ok(m) = std::fs::metadata(dir.join(f)) else { continue };
        if m.permissions().mode() & 0o077 == 0 { continue; }
        if fix && std::fs::set_permissions(dir.join(f), std::fs::Permissions::from_mode(0o600)).is_ok() { issues.push(("FIXED", format!("{f} set to mode 600"))); }
        else { issues.push(("ERROR", format!("{f} is readable by others (`doctor fix` sets mode 600)"))); }
    }
    if issues.is_empty() { println!("OK       {name}"); }
    for (l, m) in &issues { println!("{l:<8} {name}: {m}"); }
    issues.iter().filter(|(l, _)| *l == "ERROR").count()
}

/// `doctor [fix] [<dir> | --all <dir>]`: host checks, plus stack checks if a dir is given. Exit code 1 if errors remain.
pub(crate) fn doctor(args: &[String]) -> i32 {
    let mut rest: Vec<&str> = args.iter().map(String::as_str).collect();
    let fix = rest.first() == Some(&"fix");
    if fix { rest.remove(0); }
    let all = rest.first() == Some(&"--all");
    if all { rest.remove(0); }
    if all && rest.is_empty() { eprintln!("usage: strig doctor [fix] [<dir> | --all <root>]   (see `strig help doctor`)"); return 2; }
    let mut failed = doctor_host(fix);
    if let Some(d) = rest.first() {
        let root = std::path::Path::new(d);
        let mut dirs: Vec<std::path::PathBuf> = if all {
            match std::fs::read_dir(root) { Ok(rd) => rd.filter_map(|e| e.ok().map(|e| e.path())).filter(|p| p.is_dir()).collect(), Err(e) => { eprintln!("{d}: {e}"); return 1 } }
        } else { vec![root.canonicalize().unwrap_or_else(|e| { eprintln!("{d}: {e}"); std::process::exit(1) })] };
        dirs.sort();
        failed += dirs.iter().map(|p| doctor_stack(p, fix)).sum::<usize>();
    }
    (failed > 0) as i32
}

#[cfg(test)]
mod tests {

    #[test]
    fn referenced_vars_skips_defaults_and_escapes() {
        let v = super::referenced_vars("a ${A} ${B:-x} $C $$D ${E:?msg} ${F:+g} ${G-h} $1 ${H}");
        assert_eq!(v.into_iter().collect::<Vec<_>>(), ["A", "C", "E", "H"]);
    }
}
