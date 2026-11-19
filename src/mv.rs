use crate::{coolify::*, proxy::proxy, render::render, route::{route, RouteOpts}};

/// Only these chars end up in shell commands (names, uuids, hosts).
fn safe(s: &str) -> bool { !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || "-_.@:".contains(c)) }

/// Shell snippet running `cmd` on `host` (no single quotes in `cmd`), for piping between two hosts.
fn on(host: &str, cmd: &str) -> String { if host == "local" { cmd.into() } else { format!("ssh -o BatchMode=yes {host} '{cmd}'") } }

/// `move coolify <name> <dir> ...`: stop, import, migrate, copy volumes, up, route. See `strig help move`.
pub(crate) fn move_coolify(name: &str, target: &str, from: &str, stop: bool, trust: Option<&str>, dry: bool) -> Result<(), String> {
    if !safe(from) { return Err(format!("bad --from {from}")); }
    let (apps, services) = (coolify_apps(from)?, coolify_services(from)?);
    let (uuid, is_app) = match (apps.iter().find(|(_, n)| n == name), services.iter().find(|(_, n)| n == name)) {
        (Some((u, _)), _) => (u.clone(), true),
        (_, Some((u, _))) => (u.clone(), false),
        _ => return Err(format!("{name}: no such Coolify service/application")),
    };
    let stack = name.to_lowercase();
    let ps = |a: &str| run_on(from, &format!("sudo -n docker ps {a} -q --filter label=com.docker.compose.project={uuid}"));
    let (running, all) = (ps("")?, ps("-a")?);
    if !running.trim().is_empty() && !stop { return Err(format!("{name} is still running in Coolify; add --stop to stop it (it stays stopped, not removed)")); }
    // (old volume, new volume) pairs; compose creates `<stack>_<key>` for key
    let vols: Vec<(String, String)> = if all.trim().is_empty() { vec![] } else {
        let m = run_on(from, &format!("sudo -n docker inspect {} --format '{{{{range .Mounts}}}}{{{{if eq .Type \"volume\"}}}}{{{{println .Name}}}}{{{{end}}}}{{{{end}}}}'", all.split_whitespace().collect::<Vec<_>>().join(" ")))?;
        let mut v: Vec<(String, String)> = vec![];
        for o in m.lines().filter(|l| !l.is_empty()) {
            if !safe(o) { return Err(format!("unexpected volume name {o}")); }
            if !v.iter().any(|(x, _)| x == o) { v.push((o.into(), format!("{stack}_{}", vol_key(o, &uuid)))); }
        }
        v
    };
    let has_proxy = !run_on("local", "docker ps -aq --filter label=stackrig.proxy=true")?.trim().is_empty();
    if !has_proxy && trust.is_none() { return Err("no inner Traefik here: pass --trust <ip of Coolify's Traefik> to start one".into()); }
    if dry {
        println!("{name} ({}): stop {} container(s) | proxy up: {} | import{} -> {target}/{name} | volumes: {} | up | route",
            if is_app { "application" } else { "service" }, running.split_whitespace().count(), !has_proxy, if is_app { "" } else { " + migrate" },
            vols.iter().map(|(o, n)| format!("{o} -> {n}")).collect::<Vec<_>>().join(", "));
        return Ok(());
    }
    let only = [name.to_string()];
    let stopped: Vec<&str> = running.split_whitespace().collect();
    if !stopped.is_empty() { run_on(from, &format!("sudo -n docker stop {}", stopped.join(" ")))?; }
    let result = (|| -> Result<(), String> {
        if !has_proxy { proxy(&mut ["strig", "proxy", "up", "--trust", trust.unwrap()].map(String::from).to_vec(), false)?; }
        println!("{}", import_coolify(from, target, &only)?);
        if !is_app && migrate_coolify(from, target, &only)? > 0 { return Err("migrate reported problems (see above)".into()); }
        let dir = std::path::Path::new(target).join(name);
        for (old, new) in &vols {
            let key = &new[stack.len() + 1..];
            run_on("local", &format!("docker volume create --label com.docker.compose.project={stack} --label com.docker.compose.volume={key} {new} >/dev/null"))?;
            let cmd = format!("{} | {}", on(from, &format!("sudo -n docker run --rm -v {old}:/v:ro alpine tar -C /v -cf - .")), on("local", &format!("docker run --rm -i -v {new}:/v alpine tar -C /v -xf -")));
            run_on("local", &format!("bash -o pipefail -c \"{cmd}\"")).map_err(|e| format!("copy volume {old}: {e}"))?;
            eprintln!("volume {old} -> {new} copied");
        }
        let me = std::env::current_exe().map_err(|e| e.to_string())?;
        if !std::process::Command::new(me).arg("up").arg(&dir).status().map_err(|e| e.to_string())?.success() { return Err("up failed".into()); }
        let compose = ["docker-compose.yml", "docker-compose.yaml"].iter().map(|f| dir.join(f)).find(|p| p.exists()).ok_or("no compose file after import")?;
        let (_, domains) = render(compose.to_str().unwrap(), &stack, false);
        route(&stack, &domains, &RouteOpts { from: Some(from.into()), ..Default::default() })
    })();
    if result.is_err() && !stopped.is_empty() { eprintln!("rollback: `docker compose down` in {target}/{name} if started, then on {from}: docker start {}", stopped.join(" ")); }
    result
}

#[cfg(test)]
mod tests {
    #[test]
    fn on_wraps_ssh_only_for_remote() {
        assert_eq!(super::on("local", "x"), "x");
        assert_eq!(super::on("u@h", "x y"), "ssh -o BatchMode=yes u@h 'x y'");
        assert!(super::safe("u@h") && !super::safe("a;b"));
    }
}
