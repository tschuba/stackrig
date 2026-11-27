//! `strig update`: newer tags / digest pinning / update with pre-pull, Ansible deploy and health check. Git stays the truth: only compose.yml images change.
use crate::{coolify::run_on, ops};
use serde_json::Value;
use std::process::Command;

// ---- image refs and versions ----

/// `repo[:tag][@digest]` -> (repo, tag, digest). A `:` before the last `/` is a registry port, not a tag.
fn split_ref(r: &str) -> (String, Option<String>, Option<String>) {
    let (r, d) = r.split_once('@').map_or((r, None), |(a, b)| (a, Some(b.to_string())));
    match r.rfind(':').filter(|&i| !r[i..].contains('/')) {
        Some(i) => (r[..i].into(), Some(r[i + 1..].into()), d),
        None => (r.into(), None, d),
    }
}

/// (non-digit prefix, numeric parts, suffix): `v1.2.3-alpine` -> ("v", [1,2,3], "-alpine").
fn parse_tag(t: &str) -> Option<(String, Vec<u64>, String)> {
    let i = t.find(|c: char| c.is_ascii_digit())?;
    let rest = &t[i..];
    let j = rest.find(|c: char| !(c.is_ascii_digit() || c == '.')).unwrap_or(rest.len());
    let v: Result<Vec<u64>, _> = rest[..j].split('.').map(str::parse).collect();
    Some((t[..i].into(), v.ok()?, rest[j..].into()))
}

/// Number of leading parts that must stay equal: up to and including the first non-zero one (`0.21.0` -> 2, `1.2.3` -> 1).
fn major_len(v: &[u64]) -> usize { v.iter().position(|&x| x != 0).map_or(v.len(), |i| i + 1) }

/// Highest tag in the same format (prefix, suffix, number of parts) that is newer than `cur`; without `major` only within the same major (see `major_len`).
fn newest(cur: &str, tags: &[String], major: bool) -> Option<String> {
    let (p, v, s) = parse_tag(cur)?;
    let k = if major { 0 } else { major_len(&v) };
    tags.iter().filter_map(|t| { let (tp, tv, ts) = parse_tag(t)?; (tp == p && ts == s && tv.len() == v.len() && tv > v && tv[..k] == v[..k]).then_some((tv, t)) })
        .max().map(|(_, t)| t.clone())
}

/// Plain version tags (no suffix), newest first, at most `n`.
fn recent(tags: &[String], n: usize) -> Vec<String> {
    let mut v: Vec<_> = tags.iter().filter_map(|t| { let (p, v, s) = parse_tag(t)?; (s.is_empty() && (p.is_empty() || p == "v")).then_some((v, t.clone())) }).collect();
    v.sort_by(|a, b| b.cmp(a));
    v.into_iter().take(n).map(|x| x.1).collect()
}

// ---- registry access: crane > curl + docker > curl ----

fn have(c: &str) -> bool { Command::new("sh").args(["-c", &format!("command -v {c}")]).output().is_ok_and(|o| o.status.success()) }

fn registry(repo: &str) -> (String, String) {
    match repo.split_once('/') {
        Some((f, rest)) if f.contains('.') || f.contains(':') || f == "localhost" =>
            (if f == "docker.io" || f == "index.docker.io" { "registry-1.docker.io".into() } else { f.into() }, rest.into()),
        Some(_) => ("registry-1.docker.io".into(), repo.into()),
        None => ("registry-1.docker.io".into(), format!("library/{repo}")),
    }
}

fn http(url: &str, accept: &str, head: bool, tok: Option<&str>) -> Result<(u16, String, String), String> {
    let mut c = Command::new("curl");
    c.args(["-s", "-i", "--max-time", "30", "-H", &format!("Accept: {accept}")]);
    if head { c.arg("-I"); }
    if let Some(t) = tok { c.args(["-H", &format!("Authorization: Bearer {t}")]); }
    let o = c.arg(url).output().map_err(|e| format!("curl: {e}"))?;
    let s = String::from_utf8_lossy(&o.stdout).replace("\r\n", "\n");
    let (h, b) = s.split_once("\n\n").unwrap_or((&s, ""));
    let st = h.lines().next().and_then(|l| l.split_whitespace().nth(1)).and_then(|x| x.parse().ok()).ok_or(format!("{url}: no HTTP response"))?;
    Ok((st, h.to_string(), b.to_string()))
}

fn token(h: &str, repo: &str) -> Result<String, String> {
    let line = h.lines().find(|l| l.to_lowercase().starts_with("www-authenticate:")).ok_or("registry asks for auth without a challenge")?;
    let attr = |k: &str| { let p = format!("{k}=\""); let i = line.find(&p)? + p.len(); Some(line[i..].split('"').next()?.to_string()) };
    let url = format!("{}?service={}&scope={}", attr("realm").ok_or("no realm in auth challenge")?, attr("service").unwrap_or_default(),
        attr("scope").unwrap_or(format!("repository:{repo}:pull")));
    let (_, _, b) = http(&url, "application/json", false, None)?;
    let j: Value = serde_json::from_str(&b).map_err(|e| format!("token: {e}"))?;
    j["token"].as_str().or(j["access_token"].as_str()).map(String::from).ok_or("no token in response".into())
}

fn reg_get(repo: &str, suffix: &str, accept: &str, head: bool) -> Result<(String, String), String> {
    let (host, path) = registry(repo);
    let url = format!("https://{host}/v2/{path}/{suffix}");
    let (mut st, mut h, mut b) = http(&url, accept, head, None)?;
    if st == 401 { let t = token(&h, &path)?; (st, h, b) = http(&url, accept, head, Some(&t))?; }
    if st != 200 { return Err(format!("{url}: HTTP {st}")); }
    Ok((h, b))
}

fn out(c: &mut Command) -> Result<String, String> {
    let o = c.output().map_err(|e| e.to_string())?;
    if !o.status.success() { return Err(String::from_utf8_lossy(&o.stderr).trim().to_string()); }
    Ok(String::from_utf8_lossy(&o.stdout).trim().to_string())
}

fn list_tags(repo: &str) -> Result<Vec<String>, String> {
    if have("crane") { return Ok(out(Command::new("crane").args(["ls", repo]))?.lines().map(String::from).collect()); }
    // shortcut: first page only (n=1000, no Link following); upgrade if a repo has more tags than that
    let (_, b) = reg_get(repo, "tags/list?n=1000", "application/json", false)?;
    let j: Value = serde_json::from_str(&b).map_err(|e| e.to_string())?;
    Ok(j["tags"].as_array().into_iter().flatten().filter_map(|t| t.as_str().map(String::from)).collect())
}

/// Digest of the index/manifest behind `repo:tag` (the same on every architecture).
fn digest(repo: &str, tag: &str) -> Result<String, String> {
    let r = format!("{repo}:{tag}");
    if have("crane") { return out(Command::new("crane").args(["digest", &r])); }
    if have("docker") {
        if let Ok(d) = out(Command::new("docker").args(["buildx", "imagetools", "inspect", &r, "--format", "{{.Manifest.Digest}}"])) { return Ok(d); }
    }
    let acc = "application/vnd.oci.image.index.v1+json, application/vnd.docker.distribution.manifest.list.v2+json, application/vnd.oci.image.manifest.v1+json, application/vnd.docker.distribution.manifest.v2+json";
    let (h, _) = reg_get(repo, &format!("manifests/{tag}"), acc, true)?;
    h.lines().find_map(|l| l.to_lowercase().strip_prefix("docker-content-digest:").map(|d| d.trim().to_string())).ok_or(format!("{r}: no digest header"))
}

// ---- per-image logic ----

#[derive(Clone, Copy, PartialEq)]
enum Mode { Check, Pin, Update }

/// What `resolve` found, for `--outdated`: only `Upd` counts as an update.
#[derive(Clone, Copy, PartialEq)]
enum Kind { Upd, Latest, Major, Same }

/// (new image ref if the compose file should change, note for the user)
fn resolve(img: &str, mode: Mode, major: bool) -> Result<(Option<String>, String, Kind), String> {
    let (repo, tag, dig) = split_ref(img);
    match tag.as_deref() {
        None | Some("latest") => {
            if dig.is_some() { return Ok((None, "pinned".into(), Kind::Same)); }
            let d = digest(&repo, "latest")?;
            if mode != Mode::Pin { return Ok((None, format!("latest, registry digest {d}"), Kind::Latest)); }
            let fixed = recent(&list_tags(&repo)?, 20).into_iter().find(|t| digest(&repo, t).is_ok_and(|x| x == d));
            Ok(match fixed {
                Some(t) => (Some(format!("{repo}:{t}@{d}")), format!("latest -> {t}@{d}"), Kind::Same),
                None => (Some(format!("{repo}:latest@{d}")), format!("no fixed tag found for latest, pinned digest only ({d})"), Kind::Same),
            })
        }
        Some(t) => {
            if mode == Mode::Pin { return Ok((None, format!("{t} (fixed)"), Kind::Same)); }
            let tags = list_tags(&repo)?;
            let hint = match (major, newest(t, &tags, true)) { (false, Some(m)) if newest(t, &tags, false).as_ref() != Some(&m) => format!(" (major available: {m}, use --major)"), _ => String::new() };
            let Some(n) = newest(t, &tags, major) else { return Ok((None, format!("{t} up to date{hint}"), if hint.is_empty() { Kind::Same } else { Kind::Major })) };
            if mode == Mode::Check { return Ok((None, format!("{t} -> {n}{hint}"), Kind::Upd)); }
            let new = if dig.is_some() { format!("{repo}:{n}@{}", digest(&repo, &n)?) } else { format!("{repo}:{n}") };
            Ok((Some(new), format!("{t} -> {n}{hint}"), Kind::Upd))
        }
    }
}

/// Calls `f(service, image)` for every `image:` value (also ones with `${VAR}`: the caller decides); `Some` replaces it. Everything else stays byte-identical.
/// shortcut: the service is the key one indent level below `services:`, found line by line (no YAML parse, so anchors/merge keys are not followed).
fn map_images(text: &str, mut f: impl FnMut(&str, &str) -> Option<String>) -> String {
    let (mut in_svcs, mut svc_indent, mut svc) = (false, None::<usize>, String::new());
    text.split_inclusive('\n').map(|l| {
        let t = l.trim_start();
        let indent = l.len() - t.len();
        if !t.is_empty() && !t.starts_with('#') {
            if indent == 0 { in_svcs = t.trim_end() == "services:"; svc_indent = None; svc.clear(); }
            else if in_svcs {
                if svc_indent.is_none() { svc_indent = Some(indent); }
                if Some(indent) == svc_indent { if let Some((k, _)) = t.split_once(':') { svc = k.trim_matches(['"', '\'']).to_string(); } }
            }
        }
        let Some(rest) = t.strip_prefix("image:") else { return l.to_string() };
        let Some(v) = rest.split_whitespace().next().map(|v| v.trim_matches(['"', '\''])) else { return l.to_string() };
        if v.is_empty() { return l.to_string(); }
        f(&svc, v).map_or(l.to_string(), |n| l.replacen(v, &n, 1))
    }).collect()
}

/// An image whose tag is a variable: `repo:${VAR}`, `repo:${VAR:-default}` or `repo:${VAR-default}`, optionally followed by a suffix (`-alpine`).
struct Var<'a> { pre: &'a str, name: &'a str, def: Option<&'a str>, post: &'a str }

fn split_var(img: &str) -> Option<Var<'_>> {
    let s = img.find("${")?;
    let e = s + img[s..].find('}')?;
    let inner = &img[s + 2..e];
    let name = &inner[..inner.find(|c: char| !(c.is_ascii_alphanumeric() || c == '_')).unwrap_or(inner.len())];
    let rest = &inner[name.len()..];
    let def = if rest.is_empty() { None } else { Some(rest.strip_prefix(":-").or(rest.strip_prefix('-'))?) };
    let (pre, post) = (&img[..s], &img[e + 1..]);
    // the variable must sit in the tag (after the last `/`), and nothing else may be a variable
    (!name.is_empty() && pre.ends_with(':') && pre.rfind('/') < pre.rfind(':') && ![pre, post, def.unwrap_or("")].iter().any(|x| x.contains('$')))
        .then_some(Var { pre, name, def, post })
}

/// Value of `name` in `.env` text (surrounding quotes removed).
fn env_get(text: &str, name: &str) -> Option<String> {
    crate::secrets::parse_env(text).into_iter().rev().find(|(k, _)| k == name).map(|(_, v)| v.trim_matches(['"', '\'']).to_string())
}

/// Sets `name=value` in `.env` text: replaces the line or appends one.
fn env_set(text: &str, name: &str, value: &str) -> String {
    let line = format!("{name}={value}");
    if text.lines().any(|l| l.split_once('=').is_some_and(|(k, _)| k == name)) {
        return text.lines().map(|l| if l.split_once('=').is_some_and(|(k, _)| k == name) { line.clone() } else { l.to_string() }).collect::<Vec<_>>().join("\n") + "\n";
    }
    format!("{text}{}{line}\n", if text.is_empty() || text.ends_with('\n') { "" } else { "\n" })
}

/// Text of `<stack>/.env` and whether it is vault-encrypted (a missing file counts as empty and plain).
fn env_read(root: &std::path::Path, dir: &std::path::Path) -> Result<(String, bool), String> {
    let p = dir.join(".env");
    if !p.exists() { return Ok((String::new(), false)); }
    let t = std::fs::read_to_string(&p).map_err(|e| e.to_string())?;
    if !t.starts_with("$ANSIBLE_VAULT") { return Ok((t, false)); }
    crate::newstack::vault(root, &["decrypt", "--output", "-", &p.to_string_lossy()], None).map(|t| (t, true))
}

fn env_write(root: &std::path::Path, dir: &std::path::Path, text: &str, enc: bool) -> Result<(), String> {
    let p = dir.join(".env");
    if enc { crate::newstack::vault(root, &["encrypt", "--output", &p.to_string_lossy(), "-"], Some(text)).map(|_| ()) } else { std::fs::write(p, text).map_err(|e| e.to_string()) }
}

// ---- interactive ----

/// Reads one trimmed line from stdin; EOF counts as `q`.
fn ask(prompt: &str) -> String {
    use std::io::{BufRead, Write};
    eprint!("{prompt}");
    let _ = std::io::stderr().flush();
    let mut l = String::new();
    if std::io::stdin().lock().read_line(&mut l).unwrap_or(0) == 0 { return "q".into(); }
    l.trim().to_string()
}

/// Asks about one image (`who` = stack and service label). `proposal` is the new ref `resolve` suggests (if any). Returns the new ref, `None` to keep, or Err(()) to quit.
/// Any other input is taken as a tag and checked against the registry.
fn choose(who: &str, img: &str, proposal: Option<&str>, note: &str) -> Result<Option<String>, ()> {
    let (repo, _, dig) = split_ref(img);
    loop {
        eprintln!("{who} {img}  {note}");
        let a = ask(&match proposal { Some(p) => format!("  apply {p}? [Y]es / [n]o / <tag> / [q]uit: "), None => "  [Enter] keep / <tag> / [q]uit: ".to_string() });
        match a.as_str() {
            "q" | "Q" => return Err(()),
            "n" | "N" => return Ok(None),
            "" | "y" | "Y" if proposal.is_some() || a.is_empty() => return Ok(proposal.map(String::from)),
            tag => match digest(&repo, tag) {
                Ok(d) => return Ok(Some(if dig.is_some() { format!("{repo}:{tag}@{d}") } else { format!("{repo}:{tag}") })),
                Err(e) => eprintln!("  tag `{tag}` not found: {}", e.lines().last().unwrap_or("")),
            },
        }
    }
}

// ---- health ----

/// Healthy = every container healthy (or running without restarts / exited 0 if it has no healthcheck) for `min` seconds; error after `max`.
fn health(t: &str, n: &str, min: u64, max: u64, poll: u64) -> Result<(), String> {
    let cmd = r#"for c in $(docker ps -aq --filter label=com.docker.compose.project=__N__); do docker inspect --format '{{.Name}} {{.State.Status}} {{.State.ExitCode}} {{.RestartCount}} {{if .State.Health}}{{.State.Health.Status}}{{else}}-{{end}}' $c; done"#.replace("__N__", n);
    let start = std::time::Instant::now();
    let mut shown = 0;
    let mut first: std::collections::BTreeMap<String, String> = Default::default();
    loop {
        let o = run_on(t, &cmd)?;
        let mut bad = vec![];
        for l in o.lines() {
            let f: Vec<&str> = l.split_whitespace().collect();
            let [name, st, code, rc, h] = f[..] else { continue };
            let rc0 = first.entry(name.into()).or_insert(rc.into()).clone();
            let ok = match (h, st) { ("healthy", _) => true, ("-", "running") => rc == rc0, ("-", "exited") => code == "0", _ => false };
            if !ok { bad.push(format!("{name} ({st}, health {h}, restarts {rc})")); }
        }
        if o.trim().is_empty() { bad.push("no containers".into()); }
        let el = start.elapsed().as_secs();
        if bad.is_empty() && el >= min { return Ok(()); }
        if el >= max { return Err(format!("{n}: not healthy after {max} s: {}", bad.join(", "))); }
        if !bad.is_empty() && el / 15 > shown { shown = el / 15; println!("{n:14} [4/4] health  waiting ({el} s): {}", bad.join(", ")); }
        std::thread::sleep(std::time::Duration::from_secs(poll));
    }
}

// ---- command ----

/// `*` matches any run of characters (also `/`); everything else literally
fn glob(p: &str, t: &str) -> bool {
    match p.split_once('*') {
        None => p == t,
        Some((h, r)) => t.strip_prefix(h).is_some_and(|t| (0..=t.len()).filter(|i| t.is_char_boundary(*i)).any(|i| glob(r, &t[i..]))),
    }
}

pub(crate) fn update(a: &[String]) -> Result<i32, String> {
    let usage = || "usage: strig update [-i] [--major] [--outdated [--deployed]] [--deploy] --check|--pin|(plain) <stack>|--all   [--commit|--branch|--single-branch[=<name>]|--push|--pr] [--include <pattern>...] [--exclude <pattern>...] limit the services)".to_string();
    // `--include/--exclude <pattern>` (repeatable) are taken out first so their values are not mistaken for the stack
    let (mut inc, mut exc, mut rest, mut it) = (vec![], vec![], vec![], a.iter());
    while let Some(x) = it.next() {
        let (list, v) = match x.as_str() {
            "--include" | "--exclude" => (if x == "--include" { &mut inc } else { &mut exc }, it.next().ok_or_else(usage)?.clone()),
            _ => { rest.push(x.clone()); continue }
        };
        list.push(if v.contains('/') { v } else { format!("*/{v}") });
    }
    let a = &rest[..];
    let has = |f: &str| a.iter().any(|x| x == f);
    let mode = if has("--check") { Mode::Check } else if has("--pin") { Mode::Pin } else { Mode::Update };
    let inter = has("-i") || has("--interactive");
    let dep = has("--deploy");
    let outdated = has("--outdated");
    let deployed = has("--deployed");
    if deployed && !(outdated && mode == Mode::Check) { return Err("--deployed needs --check --outdated".into()); }
    if outdated && mode == Mode::Pin { return Err("--outdated does not work with --pin".into()); }
    if inter && mode != Mode::Update { return Err("-i/--interactive only works with a plain update (not --check/--pin)".into()); }
    if dep && mode != Mode::Update { return Err("--deploy only works with a plain update (not --check/--pin)".into()); }
    let name = a.iter().find(|x| !x.starts_with('-'));
    if name.is_none() == !has("--all") { return Err(usage()); }
    let single = a.iter().find_map(|x| x.strip_prefix("--single-branch=").map(String::from));
    let single_on = single.is_some() || has("--single-branch");
    let (branch, pr) = (has("--branch"), has("--pr"));
    let push = has("--push") || pr;
    let commit = has("--commit") || branch || single_on;
    if (commit || push) && mode == Mode::Check { return Err("--commit/--branch/--single-branch/--push/--pr do not work with --check".into()); }
    if branch && single_on { return Err("--branch and --single-branch cannot be combined".into()); }
    if single_on && name.is_some() { return Err("--single-branch needs --all (for one stack use --branch)".into()); }
    if push && !branch && !single_on { return Err("--push/--pr need --branch or --single-branch".into()); }
    let root = ops::root();
    let (stacks, ssh) = ops::load(&root)?;
    let targets: Vec<_> = stacks.iter().filter(|s| name.is_none_or(|n| *n == s.name)).collect();
    if targets.is_empty() { return Err(format!("unknown stack `{}`", name.unwrap())); }
    // a pattern without `*` that matches nothing is a typo (error); with `*` it only warns
    for p in inc.iter().chain(&exc) {
        if targets.iter().any(|t| t.services.iter().any(|(sv, _, _)| glob(p, &format!("{}/{sv}", t.name)))) { continue; }
        if !p.contains('*') { return Err(format!("no service `{p}` with an image in the selected stacks")); }
        eprintln!("warning: pattern `{p}` matches no service");
    }
    let want0 = |st: &str, sv: &str| { let k = format!("{st}/{sv}"); (inc.is_empty() || inc.iter().any(|p| glob(p, &k))) && !exc.iter().any(|p| glob(p, &k)) };
    let mut rc = 0;
    let (mut n_upd, mut n_stk, mut n_lat, mut n_maj, mut n_skp) = (0, 0, 0, 0, 0); // for the --outdated summary
    let date = String::from_utf8_lossy(&Command::new("date").arg("+%Y%m%d").output().map_err(|e| e.to_string())?.stdout).trim().to_string();
    let shared = single_on.then(|| single.clone().unwrap_or(format!("strig/update-{date}")));
    let def = if branch || single_on { Some(crate::gitops::default_branch(&root)?) } else { None };
    if let Some(d) = &def {
        let names: Vec<String> = match &shared { Some(b) => vec![b.clone()], None => targets.iter().map(|s| format!("strig/update-{}-{date}", s.name)).collect() };
        crate::gitops::preflight(&root, d, &names)?;
    }
    let mut on_shared = false; // the shared branch is created lazily, at the first stack with changes
    for s in targets {
        let want = |sv: &str| want0(&s.name, sv);
        let dir = root.join("stacks").join(&s.name);
        let Some(file) = ["compose.yml", "compose.yaml", "docker-compose.yml"].iter().map(|f| dir.join(f)).find(|p| p.exists()) else { eprintln!("{}: no compose file", s.name); rc = 1; continue };
        let text = std::fs::read_to_string(&file).map_err(|e| e.to_string())?;
        let mut errs = 0;
        let mut quit = false;
        let (mut w, mut wi) = (7, 5); // service column gets +1 below so names never touch the image
        map_images(&text, |sv, i| { if want(sv) { w = w.max(sv.len() + 1); wi = wi.max(i.len()); } None });
        let mut header = inter;
        let mut env: Option<Result<(String, bool), String>> = None;
        let mut notes: Vec<String> = vec![]; // one line per changed service, for the commit body
        let mut chg: Vec<(String, Option<String>)> = vec![]; // (variable, new value) per image that uses it
        let upd0 = n_upd;
        let new = map_images(&text, |sv, img| {
            if quit || !want(sv) { return None; }
            let who = format!("{:14} {sv:w$}", s.name);
            if !header && !outdated { header = true; println!("{:14} {:w$} {:wi$}  CHANGE", "STACK", "SERVICE", "IMAGE"); }
            let mut skip = |why: &str| { if outdated { n_skp += 1; return; } let m = format!("{who} {img:wi$}  skipped ({why})"); if inter { eprintln!("{m}"); } else { println!("{m}"); } };
            // `repo:${VAR}`: the version lives in the stack's .env (default as fallback), so that is what is checked and changed
            let var = split_var(img);
            if var.is_none() && img.contains('$') { skip("variable"); return None; }
            let plain = match &var {
                None => img.to_string(),
                Some(v) => {
                    let cur = match env.get_or_insert_with(|| env_read(&root, &dir)) {
                        Ok((t, _)) => env_get(t, v.name).or(v.def.map(String::from)),
                        Err(e) if mode == Mode::Check => { skip(&format!("variable: .env not readable: {}", e.lines().last().unwrap_or(""))); return None; }
                        Err(e) => { eprintln!("{who} {img}  error: .env not readable: {}", e.lines().last().unwrap_or("")); errs += 1; return None; }
                    };
                    let Some(cur) = cur else { skip("variable"); return None };
                    format!("{}{cur}{}", v.pre, v.post)
                }
            };
            // a variable's new value goes to the .env (collected, applied once per variable below); the compose line stays
            let mut fin = |n: Option<String>| match &var {
                None => n,
                Some(v) => { chg.push((v.name.to_string(), n.and_then(|n| n.strip_prefix(v.pre)?.strip_suffix(v.post).map(String::from)))); None }
            };
            match resolve(&plain, mode, has("--major")) {
                // `latest` or a digest pin is only an update/drift if the host's pulled digest differs from the registry's/pinned one
                Ok((_, note, k)) if deployed && (k == Kind::Latest || (k == Kind::Same && split_ref(&plain).2.is_some())) => {
                    let (reg, what) = match split_ref(&plain).2 { Some(d) => (d, "pinned"), None => (note.rsplit(' ').next().unwrap_or("").to_string(), "latest") };
                    let local = ssh.get(&s.host).ok_or(format!("no ssh target for host {}", s.host)).and_then(|t| run_on(t, &format!("docker image inspect --format '{{{{range .RepoDigests}}}}{{{{println .}}}}{{{{end}}}}' '{}'", split_ref(&plain).0 + ":" + split_ref(&plain).1.as_deref().unwrap_or("latest"))));
                    let msg = match local {
                        Ok(l) if l.contains(&reg) => None,
                        Ok(l) => Some(format!("{what}, deployed {} -> registry {reg}", l.lines().next().and_then(|x| x.rsplit('@').next()).unwrap_or("none"))),
                        // image never pulled on the host: a deploy would fetch something new
                        Err(e) if e.contains("No such image") || e.contains("No such object") => Some(format!("{what}, not pulled on host -> registry {reg}")),
                        Err(e) => { eprintln!("{who} {img}  error: {}", e.lines().last().unwrap_or("")); errs += 1; None }
                    };
                    if let Some(m) = msg { n_upd += 1; if !header { header = true; println!("{:14} {:w$} {:wi$}  CHANGE", "STACK", "SERVICE", "IMAGE"); } println!("{who} {img:wi$}  {m}"); }
                    None
                }
                Ok((_, _, k)) if outdated && k != Kind::Upd => { if k == Kind::Latest { n_lat += 1; } else if k == Kind::Major { n_maj += 1; } None }
                Ok((n, note, _)) if inter => { n_upd += 1; match choose(&who, img, n.as_deref(), &note) { Ok(c) => { if c.is_some() { notes.push(format!("- {sv}: {note}")); } fin(c) } Err(()) => { quit = true; None } } },
                Ok((n, note, _)) => { n_upd += 1; if !header { header = true; println!("{:14} {:w$} {:wi$}  CHANGE", "STACK", "SERVICE", "IMAGE"); } println!("{who} {img:wi$}  {note}"); if n.is_some() { notes.push(format!("- {sv}: {note}")); } fin(n) }
                Err(e) => { eprintln!("{who} {img}  error: {e}"); errs += 1; None }
            }
        });
        if n_upd > upd0 { n_stk += 1; }
        if quit { eprintln!("aborted, {} not written", s.name); return Ok(1); }
        if errs > 0 { rc = 1; if mode != Mode::Check { eprintln!("{}: skipped", s.name); } continue; }
        // one value per variable: changed only if every image using it agrees
        let mut set: Vec<(String, String)> = vec![];
        for (n, _) in &chg {
            if set.iter().any(|(x, _)| x == n) { continue; }
            let vals: Vec<_> = chg.iter().filter(|(x, _)| x == n).map(|x| &x.1).collect();
            if vals.iter().any(|v| **v != *vals[0]) { eprintln!("{:14} {n}: images using it get different results, not changed", s.name); }
            else if let Some(v) = vals[0] { set.push((n.clone(), v.clone())); }
        }
        let (mut envt, enc) = match env { Some(Ok(x)) => x, _ => (String::new(), false) };
        if mode == Mode::Check { continue; }
        let files = !set.is_empty() || new != text;
        let fl = if files { "files changed" } else { "files unchanged" };
        let rel = |f: &std::path::Path| f.strip_prefix(&root).unwrap_or(f).to_string_lossy().into_owned();
        let mut changed: Vec<String> = vec![];
        if !set.is_empty() { changed.push(rel(&dir.join(".env"))); }
        if new != text { changed.push(rel(&file)); }
        let (mut own, mut committed) = (None::<String>, false);
        if commit && files {
            if crate::gitops::dirty(&root, &changed)? { eprintln!("{}: {} has uncommitted changes, skipped", s.name, changed.join(", ")); rc = 1; continue; }
            if branch { let b = format!("strig/update-{}-{date}", s.name); crate::gitops::git(&root, &["switch", "-c", &b])?; own = Some(b); }
            else if let (Some(b), false) = (&shared, on_shared) { crate::gitops::git(&root, &["switch", "-c", b])?; on_shared = true; }
        }
        if !set.is_empty() {
            for (n, v) in &set { envt = env_set(&envt, n, v); }
            env_write(&root, &dir, &envt, enc)?;
            println!("{:14} [1/4] files   {} written ({}; review and commit)", s.name, dir.join(".env").display(), set.iter().map(|(n, v)| format!("{n}={v}")).collect::<Vec<_>>().join(", "));
        }
        if new != text { std::fs::write(&file, &new).map_err(|e| e.to_string())?; println!("{:14} [1/4] files   {} written (review and commit)", s.name, file.display()); }
        let title = format!("chore({}): {}", s.name, if mode == Mode::Pin { "pin image digests" } else { "update images" });
        if commit && files {
            crate::gitops::commit(&root, &changed, &format!("{title}\n\n{}", notes.join("\n")))?;
            committed = true;
            println!("{:14} committed: {title}", s.name);
        }
        let rc0 = rc;
        'done: {
        if mode == Mode::Pin { break 'done; }
        // safety: without --deploy (or -i, which asks) nothing touches the host
        if !dep && !inter { println!("{:14} {fl}, NOT deployed; run: strig deploy {} (or update with --deploy)", s.name, s.name); break 'done; }
        let t = ssh.get(&s.host).ok_or(format!("no ssh target for host {}", s.host))?;
        let mut imgs = vec![];
        map_images(&new, |sv, i| { if want(sv) { match split_var(i) { Some(v) => if let Some(c) = env_get(&envt, v.name).or(v.def.map(String::from)) { imgs.push(format!("{}{c}{}", v.pre, v.post)); }, None if !i.contains('$') => imgs.push(i.to_string()), _ => {} } } None });
        imgs.sort(); imgs.dedup(); // services sharing an image are pulled once
        println!("{:14} plan: on {t}: pull {} image(s), ansible-playbook --limit {} -e only={}, health check up to 120 s", s.name, imgs.len(), s.host, s.name);
        if inter && !dep && !matches!(ask(&format!("{}: deploy now? [Y/n]: ", s.name)).as_str(), "" | "y" | "Y") { println!("{:14} {fl}, NOT deployed", s.name); break 'done; }
        for (n, i) in imgs.iter().enumerate() {
            println!("{:14} [2/4] pull    {i} ({}/{})", s.name, n + 1, imgs.len());
            let t0 = std::time::Instant::now();
            match run_on(t, &format!("docker pull '{i}'")) {
                Ok(_) => println!("{:14} [2/4] pull    done ({} s)", s.name, t0.elapsed().as_secs()),
                Err(e) => eprintln!("{}: pull {i} failed: {}", s.name, e.lines().last().unwrap_or("")),
            }
        }
        println!("{:14} [3/4] deploy  ansible-playbook --limit {} -e only={}", s.name, s.host, s.name);
        let c = ops::deploy(&[s.name.clone()])?;
        if c != 0 { eprintln!("{}: {fl}, deploy FAILED (exit {c}); roll back with git revert and `strig deploy {}`", s.name, s.name); rc = 1; break 'done; }
        println!("{:14} [4/4] health  waiting (max 120 s)", s.name);
        let t0 = std::time::Instant::now();
        match health(t, &s.name, 10, 120, 5) {
            Ok(()) => println!("{:14} [4/4] health  healthy ({} s)\n{:14} {fl}, deployed, healthy", s.name, t0.elapsed().as_secs(), s.name),
            Err(e) => { eprintln!("{e}\n{}: {fl}, deployed, NOT healthy; roll back: git revert, then `strig deploy {}`", s.name, s.name); rc = 1; }
        }
        }
        // only a tested state is published (a failed deploy/health check leaves the commit local)
        if let (Some(b), Some(d)) = (&own, &def) {
            if push && committed && rc == rc0 {
                crate::gitops::push(&root, b)?;
                println!("{:14} pushed {b}", s.name);
                if pr { println!("{:14} PR: {}", s.name, crate::gitops::pr(&root, d, b, &title, &notes.join("\n"))?); }
            }
            crate::gitops::git(&root, &["switch", d])?;
        }
    }
    if let (true, Some(b), Some(d)) = (on_shared, &shared, &def) {
        if push && rc == 0 {
            crate::gitops::push(&root, b)?;
            println!("pushed {b}");
            if pr { println!("PR: {}", crate::gitops::pr(&root, d, b, "chore: update images", &format!("Updated by `strig update --all --single-branch`; one commit per stack."))?); }
        }
        crate::gitops::git(&root, &["switch", d])?;
    }
    if outdated {
        let mut extra = vec![];
        if n_maj > 0 { extra.push(format!("{n_maj} with major update (use --major)")); }
        if n_lat > 0 { extra.push(format!("{n_lat} on latest, not comparable")); }
        if n_skp > 0 { extra.push(format!("{n_skp} skipped")); }
        let head = if n_upd == 0 { "all up to date".to_string() } else { format!("{n_upd} updates in {n_stk} stacks") };
        println!("{head}{}", if extra.is_empty() { String::new() } else { format!(" ({})", extra.join(", ")) });
    }
    Ok(rc)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn v(x: &[&str]) -> Vec<String> { x.iter().map(|s| s.to_string()).collect() }
    #[test]
    fn globs() {
        assert!(glob("*/db", "shop/db") && !glob("*/db", "shop/db2") && glob("shop/*", "shop/x") && glob("a*b*c", "aXbYc") && !glob("a*b", "abc"));
    }
    #[test]
    fn refs_and_versions() {
        assert_eq!(split_ref("localhost:5000/a/b:1@sha256:x"), ("localhost:5000/a/b".into(), Some("1".into()), Some("sha256:x".into())));
        assert_eq!(split_ref("localhost:5000/a").1, None);
        assert_eq!(newest("0.21.0", &v(&["0.21.0", "0.21.2", "0.22.1", "0.9.9", "latest", "0.23.0-alpine", "1.0"]), false), Some("0.21.2".into()));
        assert_eq!(newest("0.21.0", &v(&["0.21.2", "0.22.1", "1.0.0"]), true), Some("1.0.0".into()));
        assert_eq!(newest("1.2.3", &v(&["1.9.0", "2.0.0"]), false), Some("1.9.0".into()));
        assert_eq!(newest("v1.2-alpine", &v(&["v1.3-alpine", "v1.4", "1.5-alpine"]), false), Some("v1.3-alpine".into()));
        assert_eq!(recent(&v(&["1.0", "1.10", "1.9", "latest", "2.0-rc"]), 2), ["1.10", "1.9"]);
        assert_eq!(registry("nginx").1, "library/nginx");
        assert_eq!(registry("ghcr.io/a/b").0, "ghcr.io");
        assert_eq!(registry("henrygd/beszel").0, "registry-1.docker.io");
    }
    #[test]
    fn rewrites_only_images() {
        let t = "services:\n  a:\n    image: \"x/y:1\" # c\n    command: image: z\n  b:\n    image: ${V}\n";
        assert_eq!(map_images(t, |_, i| (i == "x/y:1").then(|| "x/y:2".into())), t.replace("x/y:1", "x/y:2"));
        let mut seen = vec![];
        let t = "services:\n  a:\n    image: x:1\n  # c\n  \"b\":\n    image: y:1\nvolumes:\n  v: {}\n";
        map_images(t, |s, i| { seen.push((s.to_string(), i.to_string())); None });
        assert_eq!(seen, [("a".to_string(), "x:1".to_string()), ("b".into(), "y:1".into())]);
    }
    #[test]
    fn health_with_fake_docker() {
        let d = std::env::temp_dir().join(format!("strig-health-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        let dk = d.join("docker");
        // `ps` lists one id per stack name; `inspect` prints the state line stored in a file named after the stack
        std::fs::write(&dk, format!("#!/bin/sh\ncase \"$1\" in ps) echo c;; inspect) cat '{}'/state;; esac\n", d.display())).unwrap();
        std::fs::set_permissions(&dk, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
        // shortcut: PATH is process-wide; this is the only test touching it
        unsafe { std::env::set_var("PATH", format!("{}:{}", d.display(), std::env::var("PATH").unwrap())); }
        let st = |l: &str| std::fs::write(d.join("state"), format!("{l}\n")).unwrap();
        st("/a running 0 0 healthy");
        assert!(health("local", "a", 0, 3, 1).is_ok());
        st("/a running 0 0 -");
        assert!(health("local", "a", 0, 3, 1).is_ok());
        st("/a running 0 0 starting");
        assert!(health("local", "a", 0, 2, 1).unwrap_err().contains("starting"));
        st("/a exited 1 0 -");
        assert!(health("local", "a", 0, 2, 1).unwrap_err().contains("exited"));
        st("");
        assert!(health("local", "a", 0, 2, 1).unwrap_err().contains("no containers"));
        let _ = std::fs::remove_dir_all(&d);
    }
}
