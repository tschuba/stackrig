//! Read-only inventory (`ls`, `drift`) and the `deploy` wrapper for the homelab repo (Git = target state, hosts = actual state).
use crate::coolify::run_on;
use serde_json::Value;
use std::collections::BTreeMap;

pub(crate) struct Stack { pub name: String, pub host: String, pub domains: Vec<String>, pub auth: Vec<String>, pub services: Vec<(String, String, Vec<String>)> }

fn yaml(p: &std::path::Path) -> Value { std::fs::read_to_string(p).ok().and_then(|t| serde_saphyr::from_str(&t).ok()).unwrap_or(Value::Null) }
fn strs(v: &Value) -> Vec<String> { v.as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect()).unwrap_or_default() }

/// Per service in compose order: (service name, `name:tag` with digest dropped and `latest` if no tag, hostnames from its Traefik `Host(...)` labels).
pub(crate) fn services(compose: &Value) -> Vec<(String, String, Vec<String>)> {
    compose["services"].as_object().into_iter().flatten().filter_map(|(n, s)| {
        let i = s["image"].as_str()?;
        let r = i.split('@').next().unwrap();
        let img = if r.rsplit('/').next().unwrap().contains(':') { r.to_string() } else { format!("{r}:latest") };
        let labels: Vec<&str> = match &s["labels"] { Value::Array(a) => a.iter().filter_map(Value::as_str).collect(), Value::Object(o) => o.values().filter_map(Value::as_str).collect(), _ => vec![] };
        // shortcut: only `Host(`a`, `b`)`, not HostRegexp; upgrade if such a rule shows up
        let hosts = labels.iter().flat_map(|l| l.split("Host(").skip(1).flat_map(|p| p.split(')').next().unwrap_or("").split('`').skip(1).step_by(2).map(String::from).collect::<Vec<_>>()).collect::<Vec<_>>()).collect();
        Some((n.clone(), img, hosts))
    }).collect()
}

/// Domains behind forward-auth: all if `auth`, else `auth_domains`.
pub(crate) fn auth_domains(meta: &Value) -> Vec<String> {
    if meta["auth"].as_bool() == Some(true) { strs(&meta["domains"]) } else { strs(&meta["auth_domains"]) }
}

pub(crate) fn root() -> std::path::PathBuf { std::env::var_os("STRIG_HOME").map(Into::into).unwrap_or_else(|| ".".into()) }

/// Stacks per host from `host_vars/<host>/stacks.yml`, plus the ssh target per host from `inventory.yml`.
pub(crate) fn load(root: &std::path::Path) -> Result<(Vec<Stack>, BTreeMap<String, String>), String> {
    let inv = yaml(&root.join("inventory.yml"));
    let hv = root.join("host_vars");
    let rd = std::fs::read_dir(&hv).map_err(|e| format!("{}: {e} (run in the homelab repo or set STRIG_HOME)", hv.display()))?;
    let (mut out, mut ssh) = (vec![], BTreeMap::new());
    let mut hosts: Vec<_> = rd.flatten().filter(|d| d.path().is_dir()).map(|d| d.file_name().to_string_lossy().into_owned()).collect();
    hosts.sort();
    for h in hosts {
        let i = &inv["all"]["children"]["homelab"]["hosts"][&h];
        if let Some(addr) = i["ansible_host"].as_str() {
            ssh.insert(h.clone(), i["ansible_user"].as_str().map_or(addr.to_string(), |u| format!("{u}@{addr}")));
        }
        for n in strs(&yaml(&hv.join(&h).join("stacks.yml"))["stacks"]) {
            let d = root.join("stacks").join(&n);
            let meta = yaml(&d.join("meta.yml"));
            let compose = ["compose.yml", "compose.yaml", "docker-compose.yml"].iter().map(|f| yaml(&d.join(f))).find(|v| !v.is_null()).unwrap_or(Value::Null);
            out.push(Stack { name: n, host: h.clone(), domains: strs(&meta["domains"]), auth: auth_domains(&meta), services: services(&compose) });
        }
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Ok((out, ssh))
}

/// (project, running containers, all containers) per compose project on a host.
fn running(ssh: &str) -> Result<BTreeMap<String, (usize, usize)>, String> {
    let o = run_on(ssh, r#"docker ps -a --format '{{.Label "com.docker.compose.project"}} {{.State}}'"#)?;
    let mut m: BTreeMap<String, (usize, usize)> = BTreeMap::new();
    for l in o.lines() {
        let Some((p, s)) = l.split_once(' ') else { continue };
        if p.is_empty() { continue; }
        let e = m.entry(p.into()).or_default();
        e.1 += 1;
        if s == "running" { e.0 += 1; }
    }
    Ok(m)
}

fn table(rows: Vec<Vec<String>>) {
    let w: Vec<usize> = (0..rows[0].len()).map(|i| rows.iter().map(|r| r[i].chars().count()).max().unwrap()).collect();
    for r in rows { println!("{}", r.iter().zip(&w).map(|(c, w)| format!("{c:w$}")).collect::<Vec<_>>().join("  ").trim_end()); }
}

/// `[<stack>...] [--host <host>...]` filter of ls: stacks OR-ed, hosts OR-ed, both AND-ed.
/// Every unknown stack/host, or a stack not on the given hosts, is an error (nothing is printed).
fn filtered(cmd: &str, a: &[String]) -> Result<(Vec<Stack>, BTreeMap<String, String>), String> {
    let usage = || format!("usage: strig {cmd} [<stack>...] [--host <host>...]");
    let (mut names, mut hosts) = (vec![], vec![]);
    let mut it = a.iter();
    while let Some(x) = it.next() {
        match x.as_str() {
            "--host" => hosts.push(it.next().ok_or_else(usage)?),
            n if !n.starts_with('-') => names.push(n),
            _ => return Err(usage()),
        }
    }
    let (stacks, ssh) = load(&root())?;
    let mut bad: Vec<String> = names.iter().filter(|n| !stacks.iter().any(|s| s.name == **n)).map(|n| format!("unknown stack `{n}`")).collect();
    bad.extend(hosts.iter().filter(|h| !stacks.iter().any(|s| &s.host == **h)).map(|h| format!("unknown host `{h}`")));
    let keep = |s: &Stack| (names.is_empty() || names.contains(&s.name.as_str())) && (hosts.is_empty() || hosts.contains(&&s.host));
    if bad.is_empty() { bad.extend(names.iter().filter(|n| !stacks.iter().any(|s| s.name == **n && keep(s))).map(|n| format!("stack `{n}` is not on the given host"))); }
    if !bad.is_empty() { return Err(bad.join("\n")); }
    let stacks: Vec<_> = stacks.into_iter().filter(keep).collect();
    let ssh = ssh.into_iter().filter(|(h, _)| stacks.iter().any(|s| &s.host == h)).collect();
    Ok((stacks, ssh))
}

pub(crate) fn ls(a: &[String]) -> Result<(), String> {
    const COLS: [&str; 8] = ["stack", "host", "state", "service", "image", "tag", "domains", "auth"];
    let usage = || "usage: strig ls [<stack>...] [--host <host>...] [--show <stack,host,state,service,image,tag,domains,auth>]".to_string();
    let (mut cols, mut rest, mut it) = (None, vec![], a.iter());
    while let Some(x) = it.next() {
        if x == "--show" { if cols.replace(it.next().ok_or_else(usage)?).is_some() { return Err(usage()); } } else { rest.push(x.clone()); }
    }
    let shown = cols.is_some();
    let cols: Vec<&str> = cols.map_or(COLS.to_vec(), |c| c.split(',').collect());
    if let Some(c) = cols.iter().find(|c| !COLS.contains(c)) { return Err(format!("unknown column `{c}` (one of: {})", COLS.join(", "))); }
    let (stacks, ssh) = filtered("ls", &rest)?;
    // `name:tag` -> (name, tag); the first `:` after the last `/` splits (an earlier one is a registry port, a later one is inside `${VAR:-x}`)
    let split = |i: &str| { let base = i.rfind('/').map_or(0, |c| c + 1); i[base..].find(':').map_or((i.to_string(), String::new()), |c| (i[..base + c].to_string(), i[base + c + 1..].to_string())) };
    let mut live = BTreeMap::new();
    // only `state` needs ssh
    if cols.contains(&"state") { for (h, t) in &ssh { match running(t) { Ok(m) => { live.insert(h.clone(), m); } Err(e) => eprintln!("warning: {h}: {e}") } } }
    // full rows first (every cell filled), then sort by the shown columns in order, then blank repeated leading cells
    let mut data: Vec<Vec<String>> = vec![];
    for s in &stacks {
        // lines of (service, image, domain): each service with its own domains (from its labels), then leftover meta domains
        let mut lines: Vec<(String, String, String)> = vec![];
        for (svc, img, hosts) in &s.services {
            let mine: Vec<&String> = s.domains.iter().filter(|d| hosts.contains(d)).collect();
            for i in 0..mine.len().max(1) { lines.push((svc.clone(), img.clone(), mine.get(i).map_or(String::new(), |d| d.to_string()))); }
        }
        let used: Vec<&String> = s.services.iter().flat_map(|(_, _, h)| h).collect();
        lines.extend(s.domains.iter().filter(|d| !used.contains(d)).map(|d| (String::new(), String::new(), d.clone())));
        if lines.is_empty() { lines.push(Default::default()); }
        for (svc, img, dom) in &lines {
            data.push(cols.iter().map(|c| match *c {
                "domains" => dom.clone(),
                "auth" => if dom.is_empty() { String::new() } else if s.auth.contains(dom) { "yes".into() } else { "no".into() },
                "service" => svc.clone(),
                "image" => split(img).0,
                "tag" => split(img).1,
                "stack" => s.name.clone(),
                "host" => s.host.clone(),
                _ => match live.get(&s.host) { None => "?".into(), Some(m) => m.get(&s.name).map_or("not deployed".into(), |(r, a)| format!("{r}/{a} up")) },
            }).collect());
        }
    }
    // rows without any visible value (e.g. a domain-only line when no domain column is shown) carry no information
    data.retain(|r| r.iter().any(|c| !c.is_empty()));
    // only with --show: by the shown columns (stable, empty last); the default view keeps stacks by name and services in compose order
    if shown { data.sort_by(|a, b| a.iter().zip(b).map(|(x, y)| (x.is_empty(), x).cmp(&(y.is_empty(), y))).find(|o| o.is_ne()).unwrap_or(std::cmp::Ordering::Equal)); }
    let mut rows = vec![cols.iter().map(|c| c.to_uppercase()).collect::<Vec<_>>()];
    let mut prev: Option<Vec<String>> = None;
    for mut r in data {
        let orig = r.clone();
        let mut same = prev.is_some();
        for i in 0..r.len() { if same && prev.as_ref().unwrap()[i] == orig[i] { r[i].clear(); } else { same = false; } }
        let dup = prev.as_ref() == Some(&orig);
        prev = Some(orig);
        if !dup { rows.push(r); }
    }
    table(rows);
    Ok(())
}

/// Git vs hosts: missing (in Git, no containers), extra (containers, not in Git), differs (deployed compose.yml != Git). Returns the number of findings.
pub(crate) fn drift() -> Result<usize, String> {
    let r = root();
    let (stacks, ssh) = load(&r)?;
    let mut n = 0;
    for (h, t) in &ssh {
        let live = match running(t) { Ok(l) => l, Err(e) => { eprintln!("warning: {h}: skipped: {}", e.lines().last().unwrap_or("")); continue } };
        let want: Vec<&Stack> = stacks.iter().filter(|s| &s.host == h).collect();
        // compose.yml hashes on the host (stacks_dir = ~/homelab/stacks, see group_vars)
        let rem = run_on(t, "cd homelab/stacks 2>/dev/null && sha256sum */compose.yml || true")?;
        let rem: BTreeMap<&str, &str> = rem.lines().filter_map(|l| l.split_once("  ")).map(|(h, f)| (f.split('/').next().unwrap(), h)).collect();
        for s in &want {
            let local = run_on("local", &format!("shasum -a 256 '{}'", r.join("stacks").join(&s.name).join("compose.yml").display()))
                .ok().and_then(|o| o.split_whitespace().next().map(String::from));
            let msg = if !live.contains_key(&s.name) { Some("missing (in Git, no containers)") }
                else if local.is_some() && rem.get(s.name.as_str()).map(|x| Some(*x) != local.as_deref()) != Some(false) { Some("differs (deployed compose.yml != Git)") }
                else { None };
            if let Some(m) = msg { println!("{h}: {}: {m}", s.name); n += 1; }
        }
        for p in live.keys().filter(|p| !want.iter().any(|s| &s.name == *p)) { println!("{h}: {p}: extra (running, not in Git)"); n += 1; }
    }
    if n == 0 { println!("no drift"); }
    Ok(n)
}

/// `strig deploy <stack>|--host <host> [ansible-playbook flags]`: the only deploy path stays Ansible.
pub(crate) fn deploy(a: &[String]) -> Result<i32, String> {
    let usage = || "usage: strig deploy <stack> | --host <host>  [--pull] [ansible flags, e.g. --check --diff]".to_string();
    let (host, only, rest) = match a.first().map(String::as_str) {
        Some("--host") => (a.get(1).ok_or_else(usage)?.clone(), None, &a[2..]),
        Some(n) if !n.starts_with('-') => (load(&root())?.0.iter().find(|s| s.name == n).ok_or(format!("unknown stack `{n}` (not in any host_vars)"))?.host.clone(), Some(n), &a[1..]),
        _ => return Err(usage()),
    };
    let mut c = std::process::Command::new("ansible-playbook");
    c.current_dir(root()).args(["site.yml", "--limit", &host]);
    if let Some(n) = only { c.arg("-e").arg(format!("only={n}")); }
    if rest.iter().any(|f| f == "--pull") { c.args(["-e", "pull_always=true"]); }
    Ok(c.args(rest.iter().filter(|f| *f != "--pull")).status().map_err(|e| format!("ansible-playbook: {e}"))?.code().unwrap_or(1))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn y(s: &str) -> Value { serde_saphyr::from_str(s).unwrap() }
    #[test]
    fn tag_and_auth() {
        assert_eq!(services(&y("services:\n b: {image: 'localhost:5000/z'}\n a: {image: 'ghcr.io/x/y:1.2@sha256:ab', labels: ['traefik.x.rule=Host(`h.org`) && PathPrefix(`/`)', 'o=Host(`a.org`, `b.org`)']}\n c: {image: nginx}")),
            [("b".to_string(), "localhost:5000/z:latest".to_string(), vec![]), ("a".into(), "ghcr.io/x/y:1.2".into(), vec!["h.org".into(), "a.org".into(), "b.org".into()]), ("c".into(), "nginx:latest".into(), vec![])]);
        assert_eq!(auth_domains(&y("domains: [a]\nauth: true")), ["a"]);
        assert_eq!(auth_domains(&y("domains: [a,b]\nauth_domains: [b]")), ["b"]);
    }
}
