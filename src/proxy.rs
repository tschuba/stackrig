use crate::{coolify::{run_on, run_on_in}, route::{host_port, take_opt}};

const NAME: &str = "stackrig-proxy";

/// Whether stacks get labels for an inner Traefik behind Coolify's: `STRIG_BEHIND_PROXY` (0/1) wins, else a container labelled
/// `stackrig.proxy=true` (also stopped ones, so a restart of the proxy does not flip the mode) decides.
pub(crate) fn behind_proxy() -> bool {
    if let Some(v) = std::env::var_os("STRIG_BEHIND_PROXY") { return v != "0" && !v.is_empty(); }
    match run_on("local", "docker ps -aq --filter label=stackrig.proxy=true") {
        Ok(o) => !o.trim().is_empty(),
        Err(e) => { eprintln!("warning: cannot look for the inner Traefik ({e}); rendering with TLS labels, STRIG_BEHIND_PROXY=1 forces the other mode"); false }
    }
}

/// Compose file of the inner Traefik: plain http on `port`, trusts forwarded headers only from `trust`.
pub(crate) fn proxy_compose(trust: &str, port: u16) -> Result<String, String> {
    let ok = |c: char| c.is_ascii_hexdigit() || ".:/".contains(c);
    if trust.is_empty() || !trust.chars().all(ok) { return Err(format!("bad --trust `{trust}` (an ip or cidr)")); }
    // shortcut: a bare ip becomes /32, so IPv6 needs an explicit /128
    let trust = if trust.contains('/') { trust.to_string() } else { format!("{trust}/32") };
    Ok(format!("name: {NAME}
services:
  traefik:
    image: traefik:v3
    container_name: {NAME}
    restart: unless-stopped
    command:
      - --providers.docker=true
      - --providers.docker.exposedbydefault=false
      - --entrypoints.http.address=:80
      - --entrypoints.http.forwardedHeaders.trustedIPs={trust}
    labels:
      - stackrig.proxy=true
    ports:
      - '{port}:80'
    volumes:
      - /var/run/docker.sock:/var/run/docker.sock:ro
    networks: [proxy]
networks:
  proxy:
    external: true
"))
}

/// `proxy up|down [--to <host>] [--trust <ip>] [--port N] [--dry-run]`; `--to` defaults to `local`.
pub(crate) fn proxy(a: &mut Vec<String>, dry: bool) -> Result<(), String> {
    let (to, trust, port) = (take_opt(a, "--to"), take_opt(a, "--trust"), take_opt(a, "--port"));
    let to = to.as_deref().unwrap_or("local");
    match a.get(2).map(String::as_str) {
        Some("up") => {
            let trust = trust.ok_or("--trust <ip of Coolify's Traefik as seen by the inner one> is required")?;
            let port = port.map_or(Ok(8080), |p| p.parse::<u16>().map_err(|_| format!("bad --port `{p}`")))?;
            let yaml = proxy_compose(&trust, port)?;
            if dry { print!("{yaml}"); eprintln!("dry run: nothing was started"); return Ok(()); }
            let old = run_on(to, &format!("docker inspect {NAME} --format '{{{{json .NetworkSettings.Ports}}}}'")).ok().and_then(|p| host_port(&p));
            run_on_in(to, &format!("(docker network inspect proxy >/dev/null 2>&1 || docker network create proxy >/dev/null) && docker compose -f - up -d 2>&1"), Some(&yaml))?;
            eprintln!("inner Traefik `{NAME}` running on {to}, port {port} (trusts {trust})");
            if let Some(o) = old.filter(|o| *o != port.to_string()) {
                eprintln!("note: port changed {o} -> {port}; the files written by `strig coolify route` still point to {o}, run `strig coolify route` again for each stack");
            }
            Ok(())
        }
        Some("down") => { run_on(to, &format!("docker rm -f {NAME}"))?; eprintln!("`{NAME}` removed on {to}"); Ok(()) }
        _ => Err("usage: strig coolify proxy up --trust <ip> [--port 8080] [--to <host>] [--dry-run] | strig coolify proxy down [--to <host>]".into()),
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn compose_has_label_port_and_trust() {
        let y = super::proxy_compose("192.168.178.50", 8081).unwrap();
        assert!(y.contains("stackrig.proxy=true") && y.contains("'8081:80'") && y.contains("trustedIPs=192.168.178.50/32"));
        assert!(super::proxy_compose("10.0.0.0/24", 80).unwrap().contains("trustedIPs=10.0.0.0/24"));
        assert!(super::proxy_compose("1.1.1.1;rm", 80).is_err());
    }
}
