use crate::coolify::{run_on, run_on_in};
use serde_json::Value;

/// Removes `name value` from `a` and returns the value.
pub(crate) fn take_opt(a: &mut Vec<String>, name: &str) -> Option<String> {
    let i = a.iter().position(|x| x == name)?;
    let v = a.get(i + 1).cloned().unwrap_or_else(|| { eprintln!("{name} needs a value"); std::process::exit(2) });
    a.drain(i..=i + 1);
    Some(v)
}

/// `route` options; `from`/`to` default to `local`.
#[derive(Default)]
pub(crate) struct RouteOpts { pub from: Option<String>, pub to: Option<String>, pub upstream: Option<String>, pub dest: Option<String>, pub resolver: Option<String>, pub dry: bool }

/// Everything that ends up in the YAML and in shell commands is restricted to this set (no quoting needed).
fn safe(s: &str) -> bool { !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || "-_.:/".contains(c)) }

/// Traefik dynamic config sending every domain of `stack` to `upstream`.
pub(crate) fn route_yaml(stack: &str, domains: &[String], upstream: &str, resolver: &str) -> Result<String, String> {
    if domains.is_empty() { return Err(format!("{stack}: no SERVICE_URL/FQDN domain to route")); }
    if !upstream.starts_with("http") || !(safe(stack) && safe(upstream) && safe(resolver) && domains.iter().all(|d| safe(d))) {
        return Err(format!("bad stack name, domain, resolver or upstream (http(s)://host:port, only [A-Za-z0-9-_.:/]): {stack} {upstream}"));
    }
    let rule = domains.iter().map(|d| format!("Host(`{d}`)")).collect::<Vec<_>>().join(" || ");
    Ok(format!("http:\n  routers:\n    stackrig-{stack}:\n      rule: '{rule}'\n      entryPoints:\n        - https\n      service: stackrig-{stack}\n      tls:\n        certResolver: {resolver}\n  services:\n    stackrig-{stack}:\n      loadBalancer:\n        servers:\n          - url: '{upstream}'\n"))
}

/// Host path of Traefik's file-provider dir: `--providers.file.directory` is a path inside the container, mapped through its bind mounts.
pub(crate) fn host_dir(mounts: &str, cmd: &str) -> Option<String> {
    let (m, c): (Value, Value) = (serde_json::from_str(mounts).ok()?, serde_json::from_str(cmd).ok()?);
    let dir = c.as_array()?.iter().filter_map(Value::as_str).find_map(|a| a.strip_prefix("--providers.file.directory="))?.trim_end_matches('/');
    m.as_array()?.iter().filter_map(|x| Some((x["Source"].as_str()?, x["Destination"].as_str()?)))
        .filter(|(_, d)| dir == *d || dir.strip_prefix(d).is_some_and(|r| r.starts_with('/')))
        .max_by_key(|(_, d)| d.len()).map(|(s, d)| format!("{s}{}", &dir[d.len()..]))
}

/// First published host port out of `{{json .NetworkSettings.Ports}}`.
pub(crate) fn host_port(ports: &str) -> Option<String> {
    let v: Value = serde_json::from_str(ports.lines().next()?).ok()?;
    v.as_object()?.values().find_map(|b| b.as_array()?.first()?["HostPort"].as_str().map(String::from))
}

/// `http://<LAN ip>:<port>` of the inner Traefik (container labelled `stackrig.proxy=true`) on `to`.
/// shortcut: the ip is the source address of the default route (needs Linux `ip`), upgrade if the host has several LANs.
fn derive_upstream(to: &str) -> Result<String, String> {
    let ports = run_on(to, "sudo -n docker inspect $(sudo -n docker ps -q --filter label=stackrig.proxy=true) --format '{{json .NetworkSettings.Ports}}'")
        .map_err(|e| format!("inner Traefik (label stackrig.proxy=true) not found, or use --upstream: {e}"))?;
    let port = host_port(&ports).ok_or("inner Traefik publishes no port, use --upstream")?;
    let ip = run_on(to, "ip -4 route get 1.1.1.1 | sed -n 's/.* src \\([0-9.]*\\).*/\\1/p'")?.trim().to_string();
    if ip.is_empty() { return Err("could not determine the address of the stack host, use --upstream".into()); }
    Ok(format!("http://{ip}:{port}"))
}

fn derive_dest(from: &str) -> Result<String, String> {
    let q = |f: &str| run_on(from, &format!("sudo -n docker inspect coolify-proxy --format '{f}'")).map_err(|e| format!("{e} (is --from the Coolify host?)"));
    host_dir(&q("{{json .Mounts}}")?, &q("{{json .Config.Cmd}}")?).ok_or("no file-provider directory in coolify-proxy, use --dest".into())
}

/// `route`: builds the config for `stack`, prints it (`--dry-run`) or writes `<dest>/stackrig-<stack>.yaml` on `--from`.
pub(crate) fn route(stack: &str, domains: &[String], o: &RouteOpts) -> Result<(), String> {
    let (from, to) = (o.from.as_deref().unwrap_or("local"), o.to.as_deref().unwrap_or("local"));
    let upstream = match &o.upstream { Some(u) => u.clone(), None => derive_upstream(to)? };
    let yaml = route_yaml(stack, domains, &upstream, o.resolver.as_deref().unwrap_or("letsencrypt"))?;
    if o.dry { print!("{yaml}"); eprintln!("dry run: nothing was written"); return Ok(()); }
    let dest = match &o.dest { Some(d) => d.clone(), None => derive_dest(from)? };
    if !safe(&dest) { return Err(format!("bad destination dir: {dest}")); }
    run_on_in(from, &format!("sudo -n tee {dest}/stackrig-{stack}.yaml >/dev/null"), Some(&yaml))?;
    eprintln!("wrote {dest}/stackrig-{stack}.yaml on {from}: {} -> {upstream}", domains.join(", "));
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn yaml_has_all_domains_and_rejects_unsafe() {
        let y = super::route_yaml("app", &["a.x.net".into(), "b.x.net".into()], "http://10.0.0.1:80", "le").unwrap();
        assert!(y.contains("Host(`a.x.net`) || Host(`b.x.net`)") && y.contains("url: 'http://10.0.0.1:80'") && y.contains("certResolver: le"));
        assert!(super::route_yaml("app", &["a.x.net".into()], "http://x';rm -rf /", "le").is_err());
        assert!(super::route_yaml("app", &[], "http://x:1", "le").is_err());
    }

    #[test]
    fn derives_host_dir_and_port() {
        let m = r#"[{"Source":"/data/coolify/proxy","Destination":"/traefik"},{"Source":"/var/run/docker.sock","Destination":"/var/run/docker.sock"}]"#;
        let c = r#"["--api.dashboard=true","--providers.file.directory=/traefik/dynamic/"]"#;
        assert_eq!(super::host_dir(m, c).as_deref(), Some("/data/coolify/proxy/dynamic"));
        assert_eq!(super::host_dir(m, r#"["--providers.file.directory=/other"]"#), None);
        assert_eq!(super::host_port(r#"{"80/tcp":[{"HostIp":"0.0.0.0","HostPort":"8080"}]}"#).as_deref(), Some("8080"));
        assert_eq!(super::host_port(r#"{"80/tcp":null}"#), None);
    }
}
