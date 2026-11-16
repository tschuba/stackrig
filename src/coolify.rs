use serde_json::Value;
use crate::{secrets::*, LOCK};

/// Splits a Coolify service `.env` verbatim into (user `.env`, CLI `.env.generated`) lines.
/// `SERVICE_FQDN/URL/NAME_*` are dropped (the CLI derives them); other `SERVICE_*` are generated secrets.
/// shortcut: line-based, multi-line values are not handled, upgrade if a real `.env` has any.
pub(crate) fn split_coolify_env(text: &str) -> (Vec<String>, Vec<String>) {
    let (mut user, mut generated) = (vec![], vec![]);
    for l in text.lines().filter(|l| l.split_once('=').is_some_and(|(k, _)| !k.starts_with('#'))) {
        let k = l.split_once('=').unwrap().0;
        if ["SERVICE_FQDN_", "SERVICE_URL_", "SERVICE_NAME_"].iter().any(|p| k.starts_with(p)) { continue; }
        if k.starts_with("SERVICE_") { generated.push(l.into()) } else { user.push(l.into()) }
    }
    (user, generated)
}

/// Writes a new file (mode 600), refusing to overwrite.
pub(crate) fn write_new(path: &std::path::Path, content: &str) -> Result<(), String> {
    use std::{io::Write, os::unix::fs::OpenOptionsExt};
    let mut f = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(path)
        .map_err(|e| format!("{}: {e}", path.display()))?;
    f.write_all(content.as_bytes()).map_err(|e| e.to_string())
}

/// Runs a read-only command on `host`: `local` runs it here (CLI on the Pi), anything else goes over ssh.
/// The commands use `sudo -n`, which fails fast instead of asking for a password.
pub(crate) fn run_on(host: &str, cmd: &str) -> Result<String, String> { run_on_in(host, cmd, None) }

/// Like `run_on`, with `input` piped to the command's stdin.
pub(crate) fn run_on_in(host: &str, cmd: &str, input: Option<&str>) -> Result<String, String> {
    use std::{io::Write, process::Stdio};
    let mut c = if host == "local" { let mut c = std::process::Command::new("sh"); c.args(["-c", cmd]); c }
        else { let mut c = std::process::Command::new("ssh"); c.args(["-o", "BatchMode=yes", host, cmd]); c };
    c.stdin(if input.is_some() { Stdio::piped() } else { Stdio::null() }).stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut ch = c.spawn().map_err(|e| format!("{host}: {e}"))?;
    if let Some(i) = input { ch.stdin.take().unwrap().write_all(i.as_bytes()).map_err(|e| format!("{host}: {e}"))?; }
    let o = ch.wait_with_output().map_err(|e| format!("{host}: {e}"))?;
    if !o.status.success() { return Err(format!("{host} `{cmd}`: {}", String::from_utf8_lossy(&o.stderr).trim())); }
    Ok(String::from_utf8_lossy(&o.stdout).into_owned())
}

pub(crate) fn coolify_services(host: &str) -> Result<Vec<(String, String)>, String> { coolify_list(host, "services") }

pub(crate) fn coolify_apps(host: &str) -> Result<Vec<(String, String)>, String> { coolify_list(host, "applications where deleted_at is null") }

/// (uuid, name) of every Coolify service/application. Both end up in shell commands and paths, so anything but `[A-Za-z0-9_-]` is rejected.
pub(crate) fn coolify_list(host: &str, from: &str) -> Result<Vec<(String, String)>, String> {
    let list = run_on(host, &format!("sudo -n docker exec coolify-db psql -U coolify -At -F , -c \"select uuid, name from {from} order by name\""))?;
    let ok = |v: &str| !v.is_empty() && v.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    list.lines().filter_map(|l| l.split_once(',')).map(|(u, n)| if ok(u) && ok(n) { Ok((u.into(), n.into())) } else { Err(format!("unexpected service name/uuid: {n}")) }).collect()
}

pub(crate) fn coolify_env(host: &str, uuid: &str) -> Result<String, String> { run_on(host, &format!("sudo -n cat /data/coolify/services/{uuid}/.env")) }

/// `file:line` of lines that look like a fixed secret value (key with PASSWORD/SECRET/TOKEN/KEY/DSN, value not a `$` placeholder).
/// shortcut: rough text heuristic, no YAML parsing; upgrade if it misses real secrets.
pub(crate) fn secret_lines(text: &str) -> Vec<usize> {
    text.lines().enumerate().filter(|(_, l)| {
        let Some(i) = l.find(['=', ':']) else { return false };
        let k = l[..i].to_uppercase();
        let v = l[i + 1..].trim().trim_matches(['"', '\'']);
        ["PASSWORD", "SECRET", "TOKEN", "KEY", "DSN"].iter().any(|w| k.contains(w)) && !v.is_empty() && !v.starts_with('$') && !v.starts_with('{')
    }).map(|(n, _)| n + 1).collect()
}

/// Fills empty `SERVICE_FQDN_X[_PORT]` / `SERVICE_URL_X[_PORT]` declarations with the host of `SERVICE_FQDN_X` from Coolify's `.env`.
/// Skips (and reports) declarations without a value or with several comma-separated domains.
/// shortcut: line-based text edit, handles `- KEY` / `KEY: null` forms only; upgrade if other forms appear.
pub(crate) fn fill_domains(compose: &str, env: &str) -> (String, Vec<String>) {
    let val = |k: &str| env.lines().find_map(|l| l.strip_prefix(&format!("{k}=")))
        .map(|v| v.trim().trim_matches(['"', '\'']).trim_start_matches("https://").trim_start_matches("http://").to_string()).filter(|v| !v.is_empty());
    let mut notes = vec![];
    let out: Vec<String> = compose.lines().map(|line| {
        let t = line.trim_start();
        let (list, body) = match t.strip_prefix("- ") { Some(b) => (true, b.trim_matches(['"', '\''])), None => (false, t) };
        let key = if list { body } else { body.split(':').next().unwrap_or("") };
        let is_decl = (key.starts_with("SERVICE_URL_") || key.starts_with("SERVICE_FQDN_")) && key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
        let empty = if list { body == key } else { matches!(body.split_once(':').map(|x| x.1.trim()), Some("null" | "")) };
        if !is_decl || !empty { return line.to_string(); }
        let rest = key.trim_start_matches("SERVICE_URL_").trim_start_matches("SERVICE_FQDN_");
        let id = match rest.rsplit_once('_') { Some((n, p)) if p.parse::<u16>().is_ok() => n, _ => rest };
        let Some(dom) = val(&format!("SERVICE_FQDN_{id}")).or_else(|| val(&format!("SERVICE_URL_{id}"))) else {
            notes.push(format!("{key}: no domain in .env")); return line.to_string() };
        if dom.contains(',') { notes.push(format!("{key}: several domains, not set")); return line.to_string(); }
        let indent = &line[..line.len() - t.len()];
        if list { format!("{indent}- {key}={dom}") } else { format!("{indent}{key}: {dom}") }
    }).collect();
    (out.join("\n") + "\n", notes)
}

/// `--only` filter: empty = everything.
fn pick(list: Vec<(String, String)>, only: &[String]) -> Vec<(String, String)> {
    list.into_iter().filter(|(_, n)| only.is_empty() || only.contains(n)).collect()
}

/// A typo in `--only` must not silently migrate nothing.
fn check_only<'a>(only: &[String], known: impl Iterator<Item = &'a (String, String)>) -> Result<(), String> {
    let known: Vec<&str> = known.map(|(_, n)| n.as_str()).collect();
    match only.iter().find(|o| !known.contains(&o.as_str())) { Some(o) => Err(format!("--only {o}: no such Coolify service/application")), None => Ok(()) }
}

/// Reason `import` writes; `migrate` removes the lock only if it still holds exactly this text (never a hand-made one).
pub(crate) const IMPORT_LOCK: &str = "import coolify: waiting for `strig migrate coolify` (render/up would create new random values)";

/// `import coolify <host|local> <target-dir>`: writes `docker-compose.yml` (domains filled in) and the user's `.env` per Coolify service.
/// Reads only; never overwrites. `.env.generated` is left to `migrate`.
pub(crate) fn import_coolify(host: &str, target: &str, only: &[String]) -> Result<String, String> {
    let mut report = vec![];
    let (all_apps, all_services) = (coolify_apps(host)?, coolify_services(host)?);
    check_only(only, all_apps.iter().chain(&all_services))?;
    // Applications first and before anything is written: a copied file may hold state (e.g. OAuth tokens) that a running container keeps rewriting.
    let mut apps = vec![];
    let mut running = vec![];
    for (uuid, name) in pick(all_apps, only) {
        let base = format!("/data/coolify/applications/{uuid}");
        let app = run_on(host, &format!("sudo -n cat {base}/docker-compose.yaml")).and_then(|c| app_stack(&name, &uuid, &c, &run_on(host, &format!("sudo -n cat {base}/.env"))?));
        if app.as_ref().is_ok_and(|a| !a.files.is_empty())
            && !run_on(host, &format!("sudo -n docker ps -q --filter label=com.docker.compose.project={uuid}"))?.trim().is_empty() { running.push(name.clone()); }
        apps.push((name, app));
    }
    if !running.is_empty() {
        return Err(format!("nothing imported: application(s) {} still running and own files that the container keeps rewriting; stop them in Coolify, then run `import` again", running.join(", ")));
    }
    for (uuid, name) in pick(all_services, only) {
        let dir = std::path::Path::new(target).join(&name);
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let compose = run_on(host, &format!("sudo -n docker exec coolify-db psql -U coolify -At -c \"select docker_compose_raw from services where name='{name}'\""))?;
        let env = coolify_env(host, &uuid)?;
        let (compose_f, notes) = fill_domains(&compose, &env);
        write_new(&dir.join("docker-compose.yml"), &compose_f)?;
        // blocks render/up until `migrate` has copied Coolify's generated values (else they would be freshly randomised)
        write_new(&dir.join(LOCK), IMPORT_LOCK)?;
        let (user, _) = split_coolify_env(&env);
        if !user.is_empty() { write_new(&dir.join(".env"), &(user.join("\n") + "\n"))?; }
        report.extend(notes.iter().map(|n| format!("  {name}: {n}")));
        let hits = secret_lines(&compose);
        report.push(format!("{name}: .env: {} lines{}", user.len(), if hits.is_empty() { String::new() } else { format!(", possible fixed values in docker-compose.yml lines {hits:?}") }));
    }
    for (name, app) in apps {
        let dir = std::path::Path::new(target).join(&name);
        let app = match app { Ok(a) => a, Err(e) => { report.push(format!("ERROR    {name} (application): {e}, not imported")); continue } };
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        write_new(&dir.join("docker-compose.yml"), &app.yaml)?;
        if !app.user_env.is_empty() { write_new(&dir.join(".env"), &(app.user_env.join("\n") + "\n"))?; }
        for (abs, rel) in &app.files {
            let target_file = dir.join(rel);
            std::fs::create_dir_all(target_file.parent().unwrap()).map_err(|e| e.to_string())?;
            write_new(&target_file, &run_on(host, &format!("sudo -n cat {abs}"))?)?;
        }
        report.extend(app.notes.iter().map(|n| format!("  {name}: {n}")));
        report.push(format!("{name} (application): .env: {} lines, {} file(s) copied (mode 600)", app.user_env.len(), app.files.len()));
    }
    Ok(report.join("\n"))
}

/// What `import` makes of one Coolify application (its generated `docker-compose.yaml` is the source, the DB columns can be stale).
#[derive(Debug)]
pub(crate) struct AppStack { yaml: String, user_env: Vec<String>, files: Vec<(String, String)>, notes: Vec<String> }

/// Builds a stack from Coolify's `/data/coolify/applications/<uuid>/docker-compose.yaml` and `.env`: image, restart policy, domain and port
/// (from the Traefik labels, as `SERVICE_FQDN_X_PORT` so `render` adds the labels), env vars and bind mounts below the app dir
/// (returned as `files` = (absolute source, relative target) to copy). Coolify's own labels, networks and limits are dropped.
/// shortcut: first service only, string-form volumes only; upgrade for multi-container applications.
pub(crate) fn app_stack(name: &str, uuid: &str, compose: &str, env: &str) -> Result<AppStack, String> {
    let doc: Value = serde_saphyr::from_str(compose).map_err(|e| e.to_string())?;
    let svc = doc["services"].as_object().and_then(|m| m.values().next()).ok_or("no service in docker-compose.yaml")?;
    let image = svc["image"].as_str().ok_or("no image (not a dockerimage application?)")?;
    let labels: Vec<&str> = svc["labels"].as_array().into_iter().flatten().filter_map(|v| v.as_str()).collect();
    let port = labels.iter().find_map(|l| l.strip_prefix("traefik.http.services.")?.split_once(".loadbalancer.server.port=")?.1.parse::<u16>().ok());
    let mut hosts: Vec<&str> = vec![];
    for l in labels.iter().filter(|l| l.contains(".rule=")) {
        for h in l.split("Host(`").skip(1).filter_map(|r| r.split('`').next()) { if !hosts.contains(&h) { hosts.push(h); } }
    }
    let mut notes = vec![];
    let extra = doc["services"].as_object().map_or(0, |m| m.len()) - 1;
    if extra > 0 { notes.push(format!("{extra} more service(s) in the application ignored, add them by hand")); }
    if labels.iter().any(|l| l.contains("noindex")) { notes.push("Coolify's noindex header (X-Robots-Tag) is not carried over".into()); }
    if hosts.len() > 1 { notes.push(format!("several domains {hosts:?}, only the first is set")); }
    let mut env_decl = vec![];
    if let Some(h) = hosts.first() { env_decl.push(format!("SERVICE_FQDN_{}_{}={h}", norm(name), port.unwrap_or(80))); }
    let prefix = format!("/data/coolify/applications/{uuid}/");
    let (mut volumes, mut files) = (vec![], vec![]);
    for v in svc["volumes"].as_array().into_iter().flatten().filter_map(|v| v.as_str()) {
        match v.split_once(':').and_then(|(src, rest)| Some((src.strip_prefix(&prefix)?, rest))) {
            Some((rel, rest)) if rel.chars().all(|c| c.is_ascii_alphanumeric() || "_./-".contains(c)) && !rel.contains("..") => {
                files.push((format!("{prefix}{rel}"), rel.to_string())); volumes.push(format!("./{rel}:{rest}")) }
            _ => { notes.push(format!("volume kept as is: {v}")); volumes.push(v.into()) }
        }
    }
    let user_env: Vec<String> = split_coolify_env(env).0.into_iter().filter(|l| !l.starts_with("COOLIFY_") && !l.starts_with("SOURCE_COMMIT=")).collect();
    let mut out = serde_json::json!({ "image": image, "restart": svc["restart"].as_str().unwrap_or("unless-stopped") });
    if !user_env.is_empty() { out["env_file"] = serde_json::json!([".env"]); }
    if !env_decl.is_empty() { out["environment"] = env_decl.into(); }
    if !volumes.is_empty() { out["volumes"] = volumes.into(); }
    let yaml = serde_saphyr::to_string(&serde_json::json!({ "services": { name: out } })).map_err(|e| e.to_string())?;
    Ok(AppStack { yaml, user_env, files, notes })
}

/// How many imported values are in use by the running containers: (found, keys not found). Values are never printed.
/// Skips empty values, references (`$`) and derived `SERVICE_FQDN/URL/NAME_*`.
/// shortcut: values of 12+ characters also count when part of a longer value (URLs); shorter ones must match exactly.
pub(crate) fn value_coverage(vars: &[(String, String)], running: &[String]) -> (usize, Vec<String>) {
    let (mut found, mut missing) = (0, vec![]);
    for (k, v) in vars {
        let v = v.trim().trim_matches(['"', '\'']);
        if v.is_empty() || v.contains('$') || ["SERVICE_FQDN_", "SERVICE_URL_", "SERVICE_NAME_"].iter().any(|p| k.starts_with(p)) { continue; }
        if running.iter().any(|r| r == v || (v.len() >= 12 && r.contains(v))) { found += 1 } else { missing.push(k.clone()) }
    }
    (found, missing)
}

/// Absolute host paths a rendered compose file bind-mounts (string form `/src:/dst` and map form `type: bind`).
/// Relative paths and the docker socket are ignored.
pub(crate) fn bind_sources(yaml: &str) -> Vec<String> {
    let doc: Value = serde_saphyr::from_str(yaml).unwrap_or(Value::Null);
    let mut out = vec![];
    for (_, svc) in doc["services"].as_object().into_iter().flatten() {
        for v in svc["volumes"].as_array().into_iter().flatten() {
            let src = match v { Value::String(s) => s.split(':').next().map(String::from), Value::Object(_) if v["type"] == "bind" => v["source"].as_str().map(String::from), _ => None };
            if let Some(p) = src.filter(|p| p.starts_with('/') && p != "/var/run/docker.sock") { if !out.contains(&p) { out.push(p); } }
        }
    }
    out
}

/// `migrate coolify <host|local> <target-dir>`: for every service already imported into `<target>/<name>`, writes `.env.generated`
/// from Coolify's generated values (never overwrites), renders (fills in what Coolify never had), and compares with the running containers.
/// Reads only on the host and starts nothing. Returns the number of stacks that failed.
pub(crate) fn migrate_coolify(host: &str, target: &str, only: &[String]) -> Result<usize, String> {
    let services = coolify_services(host)?;
    check_only(only, services.iter())?;
    let me = std::env::current_exe().map_err(|e| e.to_string())?;
    // one line per running container: compose project, then the sources of its mounts (shortcut: paths with spaces break this)
    let mounts = run_on(host, "sudo -n docker inspect $(sudo -n docker ps -q) --format '{{index .Config.Labels \"com.docker.compose.project\"}} {{range .Mounts}}{{.Source}} {{end}}'").unwrap_or_default();
    let mut failed = 0;
    for (uuid, name) in pick(services, only) {
        let dir = std::path::Path::new(target).join(&name);
        let result = (|| -> Result<(String, Vec<String>), String> {
            if !dir.join("docker-compose.yml").exists() { return Err("no docker-compose.yml, run `import coolify` first".into()); }
            // a hand-made lock means "do not touch"; only the lock `import` wrote may be passed
            if let Ok(r) = std::fs::read_to_string(dir.join(LOCK)) { if r != IMPORT_LOCK { return Ok((format!("SKIPPED, locked ({LOCK}): {}", r.trim()), vec![])); } }
            let (_, generated) = split_coolify_env(&coolify_env(host, &uuid)?);
            if !generated.is_empty() { write_new(&dir.join(".env.generated"), &(generated.join("\n") + "\n"))?; }
            if std::fs::read_to_string(dir.join(LOCK)).is_ok_and(|t| t == IMPORT_LOCK) { let _ = std::fs::remove_file(dir.join(LOCK)); }
            let o = std::process::Command::new(&me).arg("render").arg(&dir).output().map_err(|e| e.to_string())?;
            if !o.status.success() { return Err(format!("render: {}", String::from_utf8_lossy(&o.stderr).lines().find(|l| !l.trim().is_empty()).unwrap_or("?"))); }
            let mut notes = vec![];
            let names = run_on(host, &"sudo -n docker ps -a --filter label=com.docker.compose.project=UUID --format '{{.Names}}'".replace("UUID", &uuid))?;
            let names: Vec<&str> = names.lines().filter(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c))).collect();
            let summary = if names.is_empty() { format!("no containers with project name {uuid} (stack not running, or the assumption is wrong)") } else {
                let env = run_on(host, &format!("sudo -n docker inspect {} --format '{{{{range .Config.Env}}}}{{{{println .}}}}{{{{end}}}}'", names.join(" ")))?;
                let running: Vec<String> = env.lines().filter_map(|l| l.split_once('=')).map(|(_, v)| v.to_string()).collect();
                let rd = |f: &str| std::fs::read_to_string(dir.join(f)).map(|t| parse_env(&t)).unwrap_or_default();
                let vars: Vec<_> = rd(".env").into_iter().chain(rd(".env.generated")).collect();
                let (found, missing) = value_coverage(&vars, &running);
                if !missing.is_empty() { notes.push(format!("WARNING values not found in the running containers: {}", missing.join(" "))); }
                format!("{found}/{} values found in running containers", found + missing.len())
            };
            for src in bind_sources(&String::from_utf8_lossy(&o.stdout)) {
                for l in mounts.lines().filter(|l| l.split_whitespace().skip(1).any(|s| s == src)) {
                    notes.push(format!("WARNING bind mount {src} is used by a running container (project {}), stop the Coolify stack before `up`", l.split_whitespace().next().unwrap_or("?")));
                }
            }
            Ok((format!("{} lines of .env.generated taken over, render OK, {summary}", generated.len()), notes))
        })();
        match result {
            Ok((line, notes)) => { println!("{name}: {line}"); notes.iter().for_each(|n| println!("  {n}")); }
            Err(e) => { failed += 1; println!("ERROR    {name}: {e}"); }
        }
    }
    Ok(failed)
}

#[cfg(test)]
mod tests {

    #[test]
    fn app_stack_from_coolify_compose() {
        let c = "services:\n  x-1:\n    image: 'ghcr.io/a/b:latest'\n    restart: unless-stopped\n    labels:\n      - coolify.managed=true\n      - 'traefik.http.routers.r.rule=Host(`qr.example.net`) && PathPrefix(`/`)'\n      - traefik.http.services.r.loadbalancer.server.port=8080\n    volumes:\n      - '/data/coolify/applications/U/app/c.json:/app/c.json'\n      - '/other:/o'\n";
        let a = super::app_stack("mini-qr", "U", c, "COOLIFY_URL=x\nSOURCE_COMMIT=y\nPORT=80\n").unwrap();
        assert_eq!(a.user_env, vec!["PORT=80"]);
        assert_eq!(a.files, vec![("/data/coolify/applications/U/app/c.json".to_string(), "app/c.json".to_string())]);
        for want in ["SERVICE_FQDN_MINIQR_8080=qr.example.net", "./app/c.json:/app/c.json", "/other:/o", "env_file"] { assert!(a.yaml.contains(want), "{want} in {}", a.yaml); }
        assert!(!a.yaml.contains("coolify"));
    }

    #[test]
    fn split_coolify_env_routes_keys() {
        let (u, g) = super::split_coolify_env("A=1\nSERVICE_PASSWORD_X=p\nSERVICE_FQDN_X=d\nSERVICE_URL_X_80=u\nSERVICE_NAME_X=x\nSERVICE_USER_X=\"n\"\n# c\nnoeq\n");
        assert_eq!(u, ["A=1"]);
        assert_eq!(g, ["SERVICE_PASSWORD_X=p", "SERVICE_USER_X=\"n\""]);
    }

    #[test]
    fn secret_lines_flags_fixed_values_only() {
        let t = "a:\n  - DB_PASSWORD=${DB_PASSWORD}\n  - SECRET_KEY=abc\n  - TOKEN: \"x\"\n  - API_KEY: \"${K}\"\n  - B_KEY=\n  - NAME=x\n";
        assert_eq!(super::secret_lines(t), [3, 4]);
    }

    #[test]
    fn fill_domains_both_forms() {
        let c = "s:\n  environment:\n    - SERVICE_URL_APP_80\n    - 'SERVICE_FQDN_B'\n    SERVICE_FQDN_C_3000: null\n    SERVICE_FQDN_D_1: x.y\n    - SERVICE_URL_E_80\n    - SERVICE_URL_F_80\n";
        let e = "SERVICE_FQDN_APP=a.example\nSERVICE_FQDN_B=b.example\nSERVICE_FQDN_C=c.example\nSERVICE_FQDN_D=d\nSERVICE_FQDN_E=e1,e2\n";
        let (o, n) = super::fill_domains(c, e);
        assert_eq!(o, "s:\n  environment:\n    - SERVICE_URL_APP_80=a.example\n    - SERVICE_FQDN_B=b.example\n    SERVICE_FQDN_C_3000: c.example\n    SERVICE_FQDN_D_1: x.y\n    - SERVICE_URL_E_80\n    - SERVICE_URL_F_80\n");
        assert_eq!(n.len(), 2);
    }

    #[test]
    fn write_new_refuses_overwrite() {
        let p = std::env::temp_dir().join(format!("wn{}", std::process::id()));
        assert!(super::write_new(&p, "a").is_ok());
        assert!(super::write_new(&p, "b").is_err());
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "a");
        std::fs::remove_file(p).unwrap();
    }

    #[test]
    fn value_coverage_skips_refs_and_derived() {
        let v = |k: &str, x: &str| (k.to_string(), x.to_string());
        let vars = [v("A", "\"abc\""), v("B", "missing"), v("C", "${X}"), v("D", ""), v("SERVICE_URL_X", "https://h"), v("E", "longsecret123")];
        let running = ["abc".to_string(), "postgres://u:longsecret123@db/x".to_string()];
        assert_eq!(super::value_coverage(&vars, &running), (2, vec!["B".to_string()]));
    }

    #[test]
    fn bind_sources_both_forms() {
        let y = "services:\n  a:\n    volumes:\n      - /etc/x:/d\n      - ./rel:/r\n      - data:/v\n      - /var/run/docker.sock:/s\n      - type: bind\n        source: /mnt/y\n        target: /t\n";
        assert_eq!(super::bind_sources(y), ["/etc/x", "/mnt/y"]);
    }
}

#[cfg(test)]
mod only_tests {
    #[test]
    fn pick_filters_and_check_only_rejects_unknown() {
        let l = vec![("1".to_string(), "a".to_string()), ("2".to_string(), "b".to_string())];
        assert_eq!(super::pick(l.clone(), &["b".into()]), vec![l[1].clone()]);
        assert_eq!(super::pick(l.clone(), &[]).len(), 2);
        assert!(super::check_only(&["x".into()], l.iter()).is_err());
        assert!(super::check_only(&["a".into()], l.iter()).is_ok());
    }
}
