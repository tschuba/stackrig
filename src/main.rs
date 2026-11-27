mod coolify;
mod doctor;
mod domain;
mod help;
mod mv;
mod mvstack;
mod newstack;
mod ops;
mod proxy;
mod render;
mod route;
mod secrets;
mod update;

use crate::{coolify::*, doctor::*, help::*, mv::*, proxy::*, render::*, route::*};

/// A stack dir with this file is refused by render/up; the file's text is the reason. Also usable by hand.
const LOCK: &str = ".lock";

fn main() {
    use std::process::{exit, Command};
    let mut a: Vec<String> = std::env::args().collect();
    // `strig coolify <cmd> ...` is the only way in; internally it maps to the old argument shapes
    let via = a.get(1).map(String::as_str) == Some("coolify");
    if a.get(1).map(String::as_str) == Some("help") && a.get(2).map(String::as_str) == Some("coolify") && a.len() > 3 { a.remove(2); if a[2] == "move" { a[2] = "coolify-move".into(); } }
    if via {
        match a.get(2).map(String::as_str) {
            Some(c @ ("import" | "migrate" | "move")) => { let c = if c == "move" { "coolify-move".to_string() } else { c.to_string() }; a.remove(2); a.splice(1..2, [c, "coolify".into()]); }
            Some("-h" | "--help") => {}
            Some("proxy" | "route") => { a.remove(1); }
            Some(_) => { eprintln!("error: unknown coolify command `{}`\n\n{}", a[2], help_for("coolify").unwrap()); exit(2) }
            None => { eprintln!("{}", help_for("coolify").unwrap()); exit(2) }
        }
    }
    if let Some(c) = help_main(&a) { exit(c); }
    if !via && matches!(a.get(1).map(String::as_str), Some("import" | "migrate" | "proxy" | "route")) {
        eprintln!("error: `strig {0}` moved to `strig coolify {0}`", a[1]); exit(2)
    }
    let dry = a.iter().skip(1).any(|x| x == "--dry-run");
    a.retain(|x| x != "--dry-run");
    // `--only a,b`: restrict import/migrate to these Coolify names
    let mut only: Vec<String> = vec![];
    if let Some(i) = a.iter().position(|x| x == "--only") {
        let Some(v) = a.get(i + 1).cloned() else { eprintln!("--only needs a comma-separated list of names"); exit(2) };
        only = v.split(',').filter(|n| !n.is_empty()).map(String::from).collect();
        a.drain(i..=i + 1);
    }
    let ro = if a.get(1).map(String::as_str) == Some("route") {
        RouteOpts { from: take_opt(&mut a, "--from"), to: take_opt(&mut a, "--to"), upstream: take_opt(&mut a, "--upstream"),
            dest: take_opt(&mut a, "--dest"), resolver: take_opt(&mut a, "--resolver"), dry }
    } else { RouteOpts::default() };
    if a.get(1).map(String::as_str) == Some("render") && a.get(2).map(String::as_str) == Some("--all") {
        let Some(root) = a.get(3) else { eprintln!("usage: strig render --all <root>   (see `strig help render`)"); exit(2) };
        exit(render_all(root));
    }
    if a.get(1).map(String::as_str) == Some("proxy") {
        match proxy(&mut a, dry) { Ok(()) => exit(0), Err(e) => { eprintln!("error: {e}"); exit(if e.starts_with("usage") { 2 } else { 1 }) } }
    }
    if a.get(1).map(String::as_str) == Some("coolify-move") {
        let (from, trust) = (take_opt(&mut a, "--from"), take_opt(&mut a, "--trust"));
        let stop = a.iter().position(|x| x == "--stop").map(|i| a.remove(i)).is_some();
        let (Some("coolify"), Some(n), Some(d)) = (a.get(2).map(String::as_str), a.get(3), a.get(4)) else {
            eprintln!("usage: strig coolify move <name> <target-dir> [--from <host>] [--stop] [--trust <ip>] [--dry-run]   (see `strig help coolify move`)"); exit(2)
        };
        match move_coolify(n, d, from.as_deref().unwrap_or("local"), stop, trust.as_deref(), dry) { Ok(()) => exit(0), Err(e) => { eprintln!("error: {e}"); exit(1) } }
    }
    match a.get(1).map(String::as_str) {
        Some("ls") => exit(ops::ls(&a[2..]).map_or_else(|e| { eprintln!("error: {e}"); 1 }, |_| 0)),
        Some("drift") => exit(ops::drift().map_or_else(|e| { eprintln!("error: {e}"); 2 }, |n| (n > 0) as i32)),
        Some("update") => exit(update::update(&a[2..]).unwrap_or_else(|e| { eprintln!("error: {e}"); 2 })),
        Some("mv" | "move") => exit(mvstack::mv(&a[2..], dry).map_or_else(|e| { eprintln!("error: {e}"); 2 }, |_| 0)),
        Some("new") => exit(newstack::new(&a[2..], dry).map_or_else(|e| { eprintln!("error: {e}"); 2 }, |_| 0)),
        Some("add") => exit(newstack::add(&a[2..], dry).map_or_else(|e| { eprintln!("error: {e}"); 2 }, |_| 0)),
        Some("env") => exit(newstack::env(&a[2..]).map_or_else(|e| { eprintln!("error: {e}"); 2 }, |_| 0)),
        Some("domain") => exit(domain::domain(&a[2..], dry).map_or_else(|e| { eprintln!("error: {e}"); 2 }, |_| 0)),
        Some("deploy") => exit(ops::deploy(&a[2..]).unwrap_or_else(|e| { eprintln!("error: {e}"); 2 })),
        _ => {}
    }
    if a.get(1).map(String::as_str) == Some("doctor") { exit(doctor(&a[2..])); }
    if let (Some(cmd @ ("import" | "migrate")), Some("coolify"), Some(h), Some(d)) = (a.get(1).map(String::as_str), a.get(2).map(String::as_str), a.get(3), a.get(4)) {
        // dry run = the real code on a throw-away copy of the target (import: empty, migrate: copy of the existing one), so nothing below `d` changes
        let tmp = if dry { Some(run_on("local", "mktemp -d").map(|t| t.trim().to_string()).unwrap_or_else(|e| { eprintln!("error: {e}"); exit(1) })) } else { None };
        let d = match &tmp {
            Some(t) => { if cmd == "migrate" { if let Err(e) = run_on("local", &format!("cp -a '{d}/.' '{t}/'")) { eprintln!("error: {e}"); exit(1) } } t }
            None => d,
        };
        let r = if cmd == "import" { import_coolify(h, d, &only).map(|m| { println!("{m}"); 0 }) } else { migrate_coolify(h, d, &only) };
        if let Some(t) = &tmp { let _ = std::fs::remove_dir_all(t); eprintln!("dry run: nothing was written to the target"); }
        match r { Ok(n) => exit((n > 0) as i32), Err(e) => { eprintln!("error: {e}"); exit(1) } }
    }
    if matches!(a.get(1).map(String::as_str), Some("import" | "migrate")) {
        eprintln!("usage: strig coolify import|migrate <host|local> <target-dir> [--only a,b] [--dry-run]   (see `strig help coolify import`)"); exit(2)
    }
    let usage = || -> ! { eprintln!("{HELP}"); exit(2) };
    let (cmd, dir) = match (a.get(1).map(String::as_str), a.get(2)) { (Some(c @ ("render" | "up" | "route")), Some(d)) => (c, std::path::Path::new(d)), _ => usage() };
    let dir = dir.canonicalize().unwrap_or_else(|e| { eprintln!("{}: {e}", dir.display()); exit(1) });
    if let Ok(reason) = std::fs::read_to_string(dir.join(LOCK)) {
        eprintln!("error: {} is locked ({LOCK}): {}", dir.display(), reason.trim()); exit(1)
    }
    let stack = dir.file_name().unwrap().to_string_lossy().to_lowercase();
    let src = ["docker-compose.yml", "docker-compose.yaml", "compose.yml", "compose.yaml"].iter().map(|f| dir.join(f)).find(|p| p.exists())
        .unwrap_or_else(|| { eprintln!("no compose file in {}", dir.display()); exit(1) });
    let (out, domains) = render(src.to_str().unwrap(), &stack, cmd != "route" && behind_proxy());
    if cmd == "route" {
        match route(&stack, &domains, &ro) { Ok(()) => return, Err(e) => { eprintln!("error: {e}"); exit(1) } }
    }
    if cmd == "render" { print!("{out}"); return; }
    // `up`: hand the rendered file and both env files to compose (forgetting an --env-file would start with empty secrets)
    let genf = dir.join("docker-compose.generated.yml");
    std::fs::write(&genf, out).unwrap();
    let mut c = Command::new("docker");
    c.args(["compose", "--project-directory"]).arg(&dir).arg("-f").arg(&genf);
    if dir.join(".env").exists() { c.arg("--env-file").arg(dir.join(".env")); }
    c.arg("--env-file").arg(dir.join(".env.generated")).arg("up");
    let extra: Vec<&String> = a.iter().skip(3).collect();
    if extra.is_empty() { c.arg("-d"); } else { c.args(extra); }
    exit(c.status().ok().and_then(|s| s.code()).unwrap_or(1));
}
