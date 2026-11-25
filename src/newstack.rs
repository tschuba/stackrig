//! `strig new` / `strig add`: create a stack or add a service in the homelab repo (compose.yml, meta.yml, vault-encrypted .env, host_vars).
use crate::{ops, secrets};
use std::{io::Write, path::Path, process::{Command, Stdio}};

struct Opts { pos: Vec<String>, host: Option<String>, image: Option<String>, domain: Option<String>, port: u16, auth: bool, yes: bool, env: Vec<String>, dry: bool }

fn parse(a: &[String], dry: bool, usage: &str) -> Result<Opts, String> {
    let mut o = Opts { pos: vec![], host: None, image: None, domain: None, port: 80, auth: false, yes: false, env: vec![], dry };
    let mut it = a.iter();
    while let Some(x) = it.next() {
        let mut val = || it.next().cloned().ok_or(usage.to_string());
        match x.as_str() {
            "--host" => o.host = Some(val()?), "--image" => o.image = Some(val()?), "--domain" => o.domain = Some(val()?),
            "--port" => o.port = val()?.parse().map_err(|_| "bad --port".to_string())?,
            "--env" => o.env.push(val()?),
            "--auth" => o.auth = true, "--yes" => o.yes = true,
            f if f.starts_with('-') => return Err(usage.into()),
            p => o.pos.push(p.into()),
        }
    }
    Ok(o)
}

fn ident(s: &str) -> bool { !s.is_empty() && s.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-') && !s.starts_with('-') }
fn dom_ok(d: &str) -> bool { d.contains('.') && d.chars().all(|c| c.is_ascii_alphanumeric() || "-.".contains(c)) && !d.starts_with(['.', '-']) }

/// One service block, indented for `services:`. With a domain: Traefik labels in the format of the existing stacks and the `proxy` network.
fn service_block(stack: &str, svc: &str, image: &str, domain: Option<(&str, u16)>, env: &[String]) -> String {
    let mut s = format!("  {svc}:\n    image: {image}\n    restart: unless-stopped\n    env_file:\n    - .env\n");
    if !env.is_empty() { s += "    environment:\n"; for e in env { s += &format!("    - {e}\n"); } }
    if let Some((d, port)) = domain {
        let r = format!("http-{stack}-{svc}");
        s += &format!("    labels:\n    - traefik.enable=true\n    - traefik.docker.network=proxy\n    - traefik.http.routers.{r}.entryPoints=http\n    - traefik.http.routers.{r}.rule=Host(`{d}`)\n    - traefik.http.routers.{r}.service={r}\n    - traefik.http.services.{r}.loadbalancer.server.port={port}\n    networks:\n      default: null\n      proxy:\n        aliases:\n        - {stack}-{svc}\n");
    }
    s
}

const NETWORKS: &str = "networks:\n  proxy:\n    external: true\n";

/// Inserts `block` at the end of the `services:` section; adds the external `proxy` network if `proxied` and missing.
fn insert_service(text: &str, block: &str, proxied: bool) -> Result<String, String> {
    let mut l: Vec<String> = text.lines().map(String::from).collect();
    let s = l.iter().position(|x| x.trim_end() == "services:").ok_or("compose.yml has no top-level `services:`")?;
    // end of the section: next top-level key (not blank, comment or indented)
    let end = (s + 1..l.len()).find(|&i| l[i].chars().next().is_some_and(|c| !c.is_whitespace() && c != '#')).unwrap_or(l.len());
    let at = (s + 1..end).rev().find(|&i| l[i].starts_with(' ') && !l[i].trim().is_empty()).map_or(s + 1, |i| i + 1);
    for (k, line) in block.lines().enumerate() { l.insert(at + k, line.into()); }
    let mut out = l.join("\n") + "\n";
    if proxied {
        if let Some(i) = out.lines().position(|x| x.trim_end() == "networks:") {
            let rest: Vec<&str> = out.lines().skip(i + 1).take_while(|x| x.starts_with(' ') || x.is_empty()).collect();
            if !rest.iter().any(|x| x.trim() == "proxy:") { return Err("top-level `networks:` has no `proxy:`; add it by hand (external: true)".into()); }
        } else { out += NETWORKS; }
    }
    Ok(out)
}

fn vault(root: &Path, args: &[&str], input: Option<&str>) -> Result<String, String> {
    let mut c = Command::new("ansible-vault").current_dir(root).args(args).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().map_err(|e| format!("ansible-vault: {e}"))?;
    c.stdin.take().unwrap().write_all(input.unwrap_or("").as_bytes()).map_err(|e| e.to_string())?;
    let o = c.wait_with_output().map_err(|e| e.to_string())?;
    if !o.status.success() { return Err(format!("ansible-vault {}: {}", args[0], String::from_utf8_lossy(&o.stderr).trim())); }
    Ok(String::from_utf8_lossy(&o.stdout).into_owned())
}

/// Existing (decrypted) .env of the stack plus every missing credential token of `text`. FQDN/URL tokens need `domain`. Existing values are never changed.
fn env_text(existing: &str, text: &str, domain: Option<&str>) -> Result<String, String> {
    let mut out = existing.to_string();
    let mut have: Vec<String> = secrets::parse_env(existing).into_iter().map(|(k, _)| k).collect();
    for t in secrets::tokens(text) {
        if have.contains(&t) { continue; }
        let v = if let Some(v) = secrets::generate(&t) { v }
            else if t.starts_with("SERVICE_FQDN_") || t.starts_with("SERVICE_URL_") {
                let d = domain.ok_or(format!("{t} needs --domain"))?;
                if t.starts_with("SERVICE_URL_") { format!("https://{d}") } else { d.into() }
            } else { continue };
        if !out.is_empty() && !out.ends_with('\n') { out.push('\n'); }
        out += &format!("{t}={v}\n");
        have.push(t);
    }
    Ok(out)
}

fn confirm(name: &str, yes: bool) -> Result<(), String> {
    if !yes {
        eprint!("deploy {name} now? [y/N] ");
        let mut l = String::new();
        let _ = std::io::stdin().read_line(&mut l);
        if !l.trim().eq_ignore_ascii_case("y") { println!("not deployed; run `strig deploy {name}` when ready"); return Ok(()); }
    }
    match ops::deploy(&[name.to_string()])? { 0 => Ok(()), c => Err(format!("deploy failed (exit {c})")) }
}

fn write_env(root: &Path, dir: &Path, text: &str) -> Result<(), String> {
    vault(root, &["encrypt", "--output", &dir.join(".env").to_string_lossy(), "-"], Some(text)).map(|_| ())
}

/// `strig new <name> --host <h> --image <i> --domain <d> [--port 80] [--auth] [--env K=V]... [--yes] [--dry-run]`
pub(crate) fn new(a: &[String], dry: bool) -> Result<(), String> {
    let usage = "usage: strig new <name> --host <host> --image <image> --domain <domain> [--port 80] [--auth] [--env K=V]... [--yes] [--dry-run]";
    let o = parse(a, dry, usage)?;
    let (Some(name), Some(host), Some(image), Some(domain), 1) = (o.pos.first(), &o.host, &o.image, &o.domain, o.pos.len()) else { return Err(usage.into()) };
    if !ident(name) { return Err(format!("bad stack name `{name}` (lowercase letters, digits, -)")); }
    if !dom_ok(domain) { return Err(format!("bad domain `{domain}`")); }
    let r = ops::root();
    let (stacks, _) = ops::load(&r)?;
    if stacks.iter().any(|s| &s.name == name) || r.join("stacks").join(name).exists() { return Err(format!("stack `{name}` already exists")); }
    if let Some(s) = stacks.iter().find(|s| s.domains.contains(domain)) { return Err(format!("{domain} is already used by stack `{}`", s.name)); }
    let hv = r.join("host_vars").join(host).join("stacks.yml");
    let hv_text = std::fs::read_to_string(&hv).map_err(|_| format!("unknown host `{host}` (no {})", hv.display()))?;
    let compose = insert_service(&format!("name: {name}\nservices:\n"), &service_block(name, name, image, Some((domain, o.port)), &o.env), true)?;
    let meta = format!("domains: [{domain}]\n{}", if o.auth { "auth: true\n" } else { "" });
    let env = env_text("", &compose, Some(domain))?;
    let hv_new = crate::mvstack::append(&hv_text, name)?;
    if o.dry { println!("dry run: new stack `{name}` on {host}\n--- compose.yml\n{compose}--- meta.yml\n{meta}--- .env: {} variable(s)", secrets::parse_env(&env).len()); return Ok(()); }
    let dir = r.join("stacks").join(name);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    // vault first: without a working vault nothing else is left behind
    if let Err(e) = write_env(&r, &dir, &env) { let _ = std::fs::remove_dir_all(&dir); return Err(e); }
    std::fs::write(dir.join("compose.yml"), compose).and_then(|_| std::fs::write(dir.join("meta.yml"), meta)).and_then(|_| std::fs::write(&hv, hv_new)).map_err(|e| e.to_string())?;
    println!("created stacks/{name} and added it to host_vars/{host}");
    confirm(name, o.yes)
}

/// `strig add <stack> <service> --image <i> [--domain <d> [--port 80]] [--env K=V]... [--yes] [--dry-run]`
pub(crate) fn add(a: &[String], dry: bool) -> Result<(), String> {
    let usage = "usage: strig add <stack> <service> --image <image> [--domain <domain> [--port 80]] [--env K=V]... [--yes] [--dry-run]";
    let o = parse(a, dry, usage)?;
    let (Some(stack), Some(svc), Some(image), 2) = (o.pos.first(), o.pos.get(1), &o.image, o.pos.len()) else { return Err(usage.into()) };
    if !ident(svc) { return Err(format!("bad service name `{svc}`")); }
    if let Some(d) = &o.domain { if !dom_ok(d) { return Err(format!("bad domain `{d}`")); } }
    let r = ops::root();
    let (stacks, _) = ops::load(&r)?;
    let s = stacks.iter().find(|s| &s.name == stack).ok_or(format!("unknown stack `{stack}`"))?;
    if s.services.iter().any(|x| &x.0 == svc) { return Err(format!("`{stack}` already has a service `{svc}`")); }
    if let Some(d) = &o.domain { if let Some(x) = stacks.iter().find(|x| x.domains.contains(d)) { return Err(format!("{d} is already used by stack `{}`", x.name)); } }
    let dir = r.join("stacks").join(stack);
    let cp = ["compose.yml", "compose.yaml", "docker-compose.yml"].iter().map(|f| dir.join(f)).find(|p| p.exists()).ok_or("no compose file")?;
    let compose = insert_service(&std::fs::read_to_string(&cp).map_err(|e| e.to_string())?, &service_block(stack, svc, image, o.domain.as_deref().map(|d| (d, o.port)), &o.env), o.domain.is_some())?;
    let meta = match &o.domain { Some(d) => Some(crate::domain::set_meta(&std::fs::read_to_string(dir.join("meta.yml")).unwrap_or_default(), d, true)?), None => None };
    let envp = dir.join(".env");
    let old = if envp.exists() { vault(&r, &["decrypt", "--output", "-", &envp.to_string_lossy()], None)? } else { String::new() };
    let env = env_text(&old, &compose, o.domain.as_deref())?;
    if o.dry { println!("dry run: add `{svc}` to `{stack}`\n--- compose.yml\n{compose}--- .env: {} new variable(s)", secrets::parse_env(&env).len() - secrets::parse_env(&old).len()); return Ok(()); }
    if env != old { write_env(&r, &dir, &env)?; }
    std::fs::write(&cp, compose).map_err(|e| e.to_string())?;
    if let Some(m) = meta { std::fs::write(dir.join("meta.yml"), m).map_err(|e| e.to_string())?; }
    println!("added `{svc}` to stacks/{stack}");
    confirm(stack, o.yes)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn insert_keeps_following_sections() {
        let t = "name: s\nservices:\n  a:\n    image: x\n\nnetworks:\n  proxy:\n    external: true\n";
        let o = insert_service(t, &service_block("s", "b", "y", None, &[]), true).unwrap();
        assert_eq!(o, "name: s\nservices:\n  a:\n    image: x\n  b:\n    image: y\n    restart: unless-stopped\n    env_file:\n    - .env\n\nnetworks:\n  proxy:\n    external: true\n");
        assert!(insert_service("services:\n  a: {}\nnetworks:\n  x: {}\n", "  b:\n", true).is_err());
        assert!(insert_service("services:\n  a: {}\n", "  b:\n", true).unwrap().ends_with(NETWORKS));
    }
    #[test]
    fn env_never_overwrites() {
        let t = "${SERVICE_PASSWORD_DB} ${SERVICE_FQDN_APP}";
        let first = env_text("", t, Some("a.x")).unwrap();
        assert!(first.contains("SERVICE_FQDN_APP=a.x\n") && first.contains("SERVICE_PASSWORD_DB="));
        assert_eq!(env_text(&first, t, Some("a.x")).unwrap(), first);
        assert_eq!(env_text("SERVICE_PASSWORD_DB=keep\n", t, Some("b.x")).unwrap().lines().next(), Some("SERVICE_PASSWORD_DB=keep"));
        assert!(env_text("", t, None).is_err());
    }
}
