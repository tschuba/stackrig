use serde_json::Value;

/// Traefik labels for one service, modelled on what Coolify 4.x generated (see the overview note, spike findings).
fn labels(stack: &str, svc: &str, domain: &str, port: u16) -> Vec<String> {
    let n = |k: &str| format!("{k}-{stack}-{svc}");
    let (h, s) = (n("http"), n("https"));
    let rule = format!("Host(`{domain}`) && PathPrefix(`/`)");
    vec![
        "traefik.enable=true".into(),
        "traefik.docker.network=proxy".into(),
        "traefik.http.middlewares.gzip.compress=true".into(),
        "traefik.http.middlewares.redirect-to-https.redirectscheme.scheme=https".into(),
        format!("traefik.http.routers.{h}.entryPoints=http"),
        format!("traefik.http.routers.{h}.middlewares=redirect-to-https"),
        format!("traefik.http.routers.{h}.rule={rule}"),
        format!("traefik.http.routers.{h}.service={h}"),
        format!("traefik.http.routers.{s}.entryPoints=https"),
        format!("traefik.http.routers.{s}.middlewares=gzip"),
        format!("traefik.http.routers.{s}.rule={rule}"),
        format!("traefik.http.routers.{s}.service={s}"),
        format!("traefik.http.routers.{s}.tls.certresolver=letsencrypt"),
        format!("traefik.http.routers.{s}.tls=true"),
        format!("traefik.http.services.{h}.loadbalancer.server.port={port}"),
        format!("traefik.http.services.{s}.loadbalancer.server.port={port}"),
    ]
}

/// A `SERVICE_URL_X[_PORT]` / `SERVICE_FQDN_X[_PORT]` declaration: (is_url, id, full name, port, domain value).
#[derive(Debug, PartialEq)]
struct Decl { url: bool, id: String, full: String, port: u16, domain: Option<String> }

fn declarations(env: &Value) -> Vec<Decl> {
    let pairs: Vec<(String, Option<String>)> = match env {
        Value::Array(a) => a.iter().filter_map(|v| v.as_str()).map(|s| match s.split_once('=') {
            Some((k, v)) => (k.to_string(), Some(v.to_string())), None => (s.to_string(), None) }).collect(),
        Value::Object(m) => m.iter().map(|(k, v)| (k.clone(), v.as_str().map(String::from))).collect(),
        _ => vec![],
    };
    pairs.into_iter().filter_map(|(k, domain)| {
        let (url, rest) = match (k.strip_prefix("SERVICE_URL_"), k.strip_prefix("SERVICE_FQDN_")) {
            (Some(r), _) => (true, r), (_, Some(r)) => (false, r), _ => return None };
        let (id, port) = match rest.rsplit_once('_') {
            Some((n, p)) if p.parse::<u16>().is_ok() => (n.to_string(), p.parse().unwrap()),
            _ => (rest.to_string(), 80) };
        Some(Decl { url, id, full: k.clone(), port, domain })
    }).collect()
}

fn norm(s: &str) -> String { s.replace(['-', '.', '_'], "").to_uppercase() }

fn random(n: usize, alphabet: &[u8]) -> String {
    use std::io::Read;
    let mut b = vec![0u8; n];
    std::fs::File::open("/dev/urandom").unwrap().read_exact(&mut b).unwrap();
    // shortcut: modulo bias (<1 bit of entropy lost per char), fine for secrets here
    b.iter().map(|x| alphabet[*x as usize % alphabet.len()] as char).collect()
}

fn base64(bytes: &[u8]) -> String {
    const T: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    bytes.chunks(3).map(|c| {
        let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        (0..4).map(|i| if i <= c.len() { T[(n >> (18 - 6 * i) & 63) as usize] as char } else { '=' }).collect::<String>()
    }).collect()
}

/// Generates a value for a credential-type magic variable (per Coolify docs); None for URL/FQDN/unknown types.
fn generate(name: &str) -> Option<String> {
    const AN: &[u8] = b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";
    const SYM: &[u8] = b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789!@#$%^&*-_=+";
    let rest = name.strip_prefix("SERVICE_")?;
    let size = |after: &str, default: usize| -> usize {
        after.split('_').next().and_then(|p| p.parse().ok()).unwrap_or(default)
    };
    if rest.starts_with("USER_") { Some(random(16, AN)) }
    else if let Some(a) = rest.strip_prefix("PASSWORDWITHSYMBOLS_") { Some(random(size(a, 32), SYM)) }
    else if let Some(a) = rest.strip_prefix("PASSWORD_") { Some(random(size(a, 32), AN)) }
    else if let Some(a) = rest.strip_prefix("BASE64_") { Some(random(size(a, 32), AN)) }
    else if let Some(a) = rest.strip_prefix("REALBASE64_") { Some(base64(random(size(a, 32), AN).as_bytes())) }
    else if let Some(a) = rest.strip_prefix("HEX_") { Some(random(size(a, 32), b"0123456789abcdef")) }
    else { None }
}

/// Every `SERVICE_*` token mentioned anywhere in the compose text.
fn tokens(text: &str) -> Vec<String> {
    let mut out: Vec<String> = vec![];
    let mut rest = text;
    let mut prev = ' ';
    while let Some(i) = rest.find("SERVICE_") {
        prev = rest[..i].chars().last().unwrap_or(prev);
        let t: String = rest[i..].chars().take_while(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || *c == '_').collect();
        // a preceding identifier char means it is the tail of another name (e.g. ZITADEL_SERVICE_USER_TOKEN_FILE)
        if !(prev.is_ascii_alphanumeric() || prev == '_') && !out.contains(&t) { out.push(t.clone()); }
        prev = t.chars().last().unwrap_or(' ');
        rest = &rest[i + t.len().max(1)..];
    }
    out
}

fn parse_env(text: &str) -> Vec<(String, String)> {
    text.lines().filter_map(|l| l.split_once('=')).map(|(k, v)| (k.into(), v.into())).collect()
}

/// Existing `.env` values win; only missing credentials are generated, domain variables follow the declarations.
fn env_entries(text: &str, decls: &[Decl], existing: &[(String, String)]) -> Result<Vec<(String, String)>, String> {
    let mut out = existing.to_vec();
    let mut add = |k: String, v: String| if !out.iter().any(|(e, _)| *e == k) { out.push((k, v)); };
    for d in decls {
        let dom = d.domain.clone().filter(|v| v != "null" && !v.is_empty()).ok_or(format!("{}: domain missing", d.full))?;
        let dom = dom.trim_start_matches("https://").trim_start_matches("http://").to_string();
        let (url, fqdn) = (format!("https://{dom}"), dom);
        // Coolify offers both variants per service, so a declaration of one provides both (the declared one with its port suffix too)
        add(d.full.clone(), if d.url { url.clone() } else { fqdn.clone() });
        add(format!("SERVICE_URL_{}", d.id), url);
        add(format!("SERVICE_FQDN_{}", d.id), fqdn);
    }
    for t in tokens(text) { if let Some(v) = generate(&t) { add(t, v); } }
    Ok(out)
}

/// Renders `path` for `stack`: writes `.env.generated` next to it and returns the final compose YAML.
fn render(path: &str, stack: &str) -> String {
    let text = std::fs::read_to_string(path).unwrap();
    let mut doc: Value = serde_saphyr::from_str(&text).unwrap();
    let mut decls: Vec<(String, Decl)> = vec![];
    for (svc, def) in doc["services"].as_object().unwrap() {
        for d in declarations(&def["environment"]) { decls.push((svc.clone(), d)); }
    }
    let envp = std::path::Path::new(path).with_file_name(".env.generated");
    let existing = std::fs::read_to_string(&envp).map(|t| parse_env(&t)).unwrap_or_default();
    let entries = env_entries(&text, &decls.iter().map(|(_, d)| Decl { ..clone_decl(d) }).collect::<Vec<_>>(), &existing)
        .unwrap_or_else(|e| { eprintln!("error: {e}"); std::process::exit(1) });
    {
        use std::os::unix::fs::OpenOptionsExt;
        use std::io::Write;
        let mut f = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&envp).unwrap();
        for (k, v) in &entries { writeln!(f, "{k}={v}").unwrap(); }
    }
    for (svc, d) in &decls {
        let dom = entries.iter().find(|(k, _)| *k == d.full).unwrap().1.trim_start_matches("https://").to_string();
        let _ = norm(svc);
        let ls = labels(stack, svc, &dom, d.port);
        let arr = doc["services"][svc].as_object_mut().unwrap().entry("labels").or_insert(Value::Array(vec![]));
        let arr = arr.as_array_mut().expect("map-form labels not handled");
        let existing: Vec<String> = arr.iter().filter_map(|v| v.as_str().map(String::from)).collect();
        let merged = merge_labels(&existing, ls).unwrap_or_else(|e| { eprintln!("error: {svc}: {e}"); std::process::exit(1) });
        *arr = merged.into_iter().map(Value::String).collect();
    }
    set_project_name(&mut doc, stack);
    let proxied: Vec<String> = decls.iter().map(|(s, _)| s.clone()).collect();
    attach_proxy(&mut doc, stack, &proxied);
    serde_saphyr::to_string(&doc).unwrap()
}

/// Splits a Coolify service `.env` verbatim into (user `.env`, CLI `.env.generated`) lines.
/// `SERVICE_FQDN/URL/NAME_*` are dropped (the CLI derives them); other `SERVICE_*` are generated secrets.
/// shortcut: line-based, multi-line values are not handled, upgrade if a real `.env` has any.
fn split_coolify_env(text: &str) -> (Vec<String>, Vec<String>) {
    let (mut user, mut generated) = (vec![], vec![]);
    for l in text.lines().filter(|l| l.split_once('=').is_some_and(|(k, _)| !k.starts_with('#'))) {
        let k = l.split_once('=').unwrap().0;
        if ["SERVICE_FQDN_", "SERVICE_URL_", "SERVICE_NAME_"].iter().any(|p| k.starts_with(p)) { continue; }
        if k.starts_with("SERVICE_") { generated.push(l.into()) } else { user.push(l.into()) }
    }
    (user, generated)
}

/// Writes a new file (mode 600), refusing to overwrite.
fn write_new(path: &std::path::Path, content: &str) -> Result<(), String> {
    use std::{io::Write, os::unix::fs::OpenOptionsExt};
    let mut f = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(path)
        .map_err(|e| format!("{}: {e}", path.display()))?;
    f.write_all(content.as_bytes()).map_err(|e| e.to_string())
}

/// Runs a read-only command on `host`: `local` runs it here (CLI on the Pi), anything else goes over ssh.
/// The commands use `sudo -n`, which fails fast instead of asking for a password.
fn run_on(host: &str, cmd: &str) -> Result<String, String> {
    let mut c = if host == "local" { let mut c = std::process::Command::new("sh"); c.args(["-c", cmd]); c }
        else { let mut c = std::process::Command::new("ssh"); c.args(["-o", "BatchMode=yes", host, cmd]); c };
    let o = c.output().map_err(|e| format!("{host}: {e}"))?;
    if !o.status.success() { return Err(format!("{host} `{cmd}`: {}", String::from_utf8_lossy(&o.stderr).trim())); }
    Ok(String::from_utf8_lossy(&o.stdout).into_owned())
}

fn coolify_services(host: &str) -> Result<Vec<(String, String)>, String> { coolify_list(host, "services") }
fn coolify_apps(host: &str) -> Result<Vec<(String, String)>, String> { coolify_list(host, "applications where deleted_at is null") }

/// (uuid, name) of every Coolify service/application. Both end up in shell commands and paths, so anything but `[A-Za-z0-9_-]` is rejected.
fn coolify_list(host: &str, from: &str) -> Result<Vec<(String, String)>, String> {
    let list = run_on(host, &format!("sudo -n docker exec coolify-db psql -U coolify -At -F , -c \"select uuid, name from {from} order by name\""))?;
    let ok = |v: &str| !v.is_empty() && v.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    list.lines().filter_map(|l| l.split_once(',')).map(|(u, n)| if ok(u) && ok(n) { Ok((u.into(), n.into())) } else { Err(format!("unexpected service name/uuid: {n}")) }).collect()
}

fn coolify_env(host: &str, uuid: &str) -> Result<String, String> { run_on(host, &format!("sudo -n cat /data/coolify/services/{uuid}/.env")) }

/// `file:line` of lines that look like a fixed secret value (key with PASSWORD/SECRET/TOKEN/KEY/DSN, value not a `$` placeholder).
/// shortcut: rough text heuristic, no YAML parsing; upgrade if it misses real secrets.
fn secret_lines(text: &str) -> Vec<usize> {
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
fn fill_domains(compose: &str, env: &str) -> (String, Vec<String>) {
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

/// A stack dir with this file is refused by render/up; the file's text is the reason. Also usable by hand.
const LOCK: &str = ".lock";
/// Reason `import` writes; `migrate` removes the lock only if it still holds exactly this text (never a hand-made one).
const IMPORT_LOCK: &str = "import coolify: waiting for `strig migrate coolify` (render/up would create new random values)";

/// `import coolify <host|local> <target-dir>`: writes `docker-compose.yml` (domains filled in) and the user's `.env` per Coolify service.
/// Reads only; never overwrites. `.env.generated` is left to `migrate`.
fn import_coolify(host: &str, target: &str) -> Result<String, String> {
    let mut report = vec![];
    // Applications first and before anything is written: a copied file may hold state (e.g. OAuth tokens) that a running container keeps rewriting.
    let mut apps = vec![];
    let mut running = vec![];
    for (uuid, name) in coolify_apps(host)? {
        let base = format!("/data/coolify/applications/{uuid}");
        let app = run_on(host, &format!("sudo -n cat {base}/docker-compose.yaml")).and_then(|c| app_stack(&name, &uuid, &c, &run_on(host, &format!("sudo -n cat {base}/.env"))?));
        if app.as_ref().is_ok_and(|a| !a.files.is_empty())
            && !run_on(host, &format!("sudo -n docker ps -q --filter label=com.docker.compose.project={uuid}"))?.trim().is_empty() { running.push(name.clone()); }
        apps.push((name, app));
    }
    if !running.is_empty() {
        return Err(format!("nothing imported: application(s) {} still running and own files that the container keeps rewriting; stop them in Coolify, then run `import` again", running.join(", ")));
    }
    for (uuid, name) in coolify_services(host)? {
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
struct AppStack { yaml: String, user_env: Vec<String>, files: Vec<(String, String)>, notes: Vec<String> }

/// Builds a stack from Coolify's `/data/coolify/applications/<uuid>/docker-compose.yaml` and `.env`: image, restart policy, domain and port
/// (from the Traefik labels, as `SERVICE_FQDN_X_PORT` so `render` adds the labels), env vars and bind mounts below the app dir
/// (returned as `files` = (absolute source, relative target) to copy). Coolify's own labels, networks and limits are dropped.
/// shortcut: first service only, string-form volumes only; upgrade for multi-container applications.
fn app_stack(name: &str, uuid: &str, compose: &str, env: &str) -> Result<AppStack, String> {
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
fn value_coverage(vars: &[(String, String)], running: &[String]) -> (usize, Vec<String>) {
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
fn bind_sources(yaml: &str) -> Vec<String> {
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
fn migrate_coolify(host: &str, target: &str) -> Result<usize, String> {
    let me = std::env::current_exe().map_err(|e| e.to_string())?;
    // one line per running container: compose project, then the sources of its mounts (shortcut: paths with spaces break this)
    let mounts = run_on(host, "sudo -n docker inspect $(sudo -n docker ps -q) --format '{{index .Config.Labels \"com.docker.compose.project\"}} {{range .Mounts}}{{.Source}} {{end}}'").unwrap_or_default();
    let mut failed = 0;
    for (uuid, name) in coolify_services(host)? {
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

/// Renders every stack dir below `root` in its own process (one failure, e.g. a panic, must not stop the rest).
/// Prints one line per stack, returns 1 if any failed. Writes `.env.generated` like a single `render`.
fn render_all(root: &str) -> i32 {
    let mut dirs: Vec<_> = match std::fs::read_dir(root) {
        Ok(rd) => rd.filter_map(|e| e.ok().map(|e| e.path())).filter(|p| p.is_dir()).collect(),
        Err(e) => { eprintln!("{root}: {e}"); return 1 }
    };
    dirs.sort();
    let me = std::env::current_exe().expect("current_exe");
    let mut failed = 0;
    for d in &dirs {
        let name = d.file_name().unwrap().to_string_lossy();
        let o = std::process::Command::new(&me).arg("render").arg(d).output().expect("spawn");
        if o.status.success() { println!("OK       {name}"); continue; }
        failed += 1;
        let err = String::from_utf8_lossy(&o.stderr);
        println!("ERROR    {name}: {}", err.lines().find(|l| !l.trim().is_empty()).unwrap_or("?"));
    }
    println!("{} stacks, {failed} with errors", dirs.len());
    (failed > 0) as i32
}

/// Variables a compose text needs from the environment: `$X`, `${X}`, `${X:?..}`; not `$$X` or ones with a default (`:-`, `-`, `:+`, `+`).
/// shortcut: a variable inside the default of another (`${A:-${B}}`) counts as required; upgrade if that gives false alarms.
fn referenced_vars(text: &str) -> std::collections::BTreeSet<String> {
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

fn docker(args: &[&str]) -> Option<String> {
    std::process::Command::new("docker").args(args).output().ok().filter(|o| o.status.success()).map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
}

/// Host checks. With `fix`, creates the missing network `proxy`. Returns the number of problems left.
fn doctor_host(fix: bool) -> usize {
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
fn doctor_stack(dir: &std::path::Path, fix: bool) -> usize {
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
fn doctor(args: &[String]) -> i32 {
    let mut rest: Vec<&str> = args.iter().map(String::as_str).collect();
    let fix = rest.first() == Some(&"fix");
    if fix { rest.remove(0); }
    let all = rest.first() == Some(&"--all");
    if all { rest.remove(0); }
    if all && rest.is_empty() { eprintln!("usage: strig doctor [fix] [<stack-dir> | --all <dir-with-stack-dirs>]"); return 2; }
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

fn main() {
    use std::process::{exit, Command};
    let mut a: Vec<String> = std::env::args().collect();
    let dry = a.iter().skip(1).any(|x| x == "--dry-run");
    a.retain(|x| x != "--dry-run");
    if a.get(1).map(String::as_str) == Some("render") && a.get(2).map(String::as_str) == Some("--all") {
        let Some(root) = a.get(3) else { eprintln!("usage: strig render --all <dir-with-stack-dirs>"); exit(2) };
        exit(render_all(root));
    }
    if a.get(1).map(String::as_str) == Some("doctor") { exit(doctor(&a[2..])); }
    if let (Some(cmd @ ("import" | "migrate")), Some("coolify"), Some(h), Some(d)) = (a.get(1).map(String::as_str), a.get(2).map(String::as_str), a.get(3), a.get(4)) {
        // dry run = the real code on a throw-away copy of the target (import: empty, migrate: copy of the existing one), so nothing below `d` changes
        let tmp = if dry { Some(run_on("local", "mktemp -d").map(|t| t.trim().to_string()).unwrap_or_else(|e| { eprintln!("error: {e}"); exit(1) })) } else { None };
        let d = match &tmp {
            Some(t) => { if cmd == "migrate" { if let Err(e) = run_on("local", &format!("cp -a '{d}/.' '{t}/'")) { eprintln!("error: {e}"); exit(1) } } t }
            None => d,
        };
        let r = if cmd == "import" { import_coolify(h, d).map(|m| { println!("{m}"); 0 }) } else { migrate_coolify(h, d) };
        if let Some(t) = &tmp { let _ = std::fs::remove_dir_all(t); eprintln!("dry run: nothing was written to the target"); }
        match r { Ok(n) => exit((n > 0) as i32), Err(e) => { eprintln!("error: {e}"); exit(1) } }
    }
    if matches!(a.get(1).map(String::as_str), Some("import" | "migrate")) {
        eprintln!("usage: strig import|migrate coolify <host|local> <target-dir> [--dry-run]   (host = user@host over ssh, local = run here)"); exit(2)
    }
    let usage = || -> ! {
        eprintln!("usage: strig render <dir> | render --all <dir> | up <dir> [compose up args] | doctor [fix] [<dir> | --all <dir>] | import|migrate coolify <host|local> <dir> [--dry-run]");
        eprintln!("everything else: docker compose -p <stack> <command>   (stack = dir name, lowercase)");
        exit(2)
    };
    let (cmd, dir) = match (a.get(1).map(String::as_str), a.get(2)) { (Some(c @ ("render" | "up")), Some(d)) => (c, std::path::Path::new(d)), _ => usage() };
    let dir = dir.canonicalize().unwrap_or_else(|e| { eprintln!("{}: {e}", dir.display()); exit(1) });
    if let Ok(reason) = std::fs::read_to_string(dir.join(LOCK)) {
        eprintln!("error: {} is locked ({LOCK}): {}", dir.display(), reason.trim()); exit(1)
    }
    let stack = dir.file_name().unwrap().to_string_lossy().to_lowercase();
    let src = ["docker-compose.yml", "docker-compose.yaml", "compose.yml", "compose.yaml"].iter().map(|f| dir.join(f)).find(|p| p.exists())
        .unwrap_or_else(|| { eprintln!("no compose file in {}", dir.display()); exit(1) });
    let out = render(src.to_str().unwrap(), &stack);
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

/// Keys whose value is a comma list, so `...` (= the generated elements) may appear in them.
fn is_list_key(k: &str) -> bool { k.ends_with(".middlewares") || k.ends_with(".entryPoints") }

/// Merges generated labels into the file's own: same key and no `...` -> the file wins;
/// `...` inside a list value (start, middle or end) -> replaced by the generated elements.
fn merge_labels(existing: &[String], generated: Vec<String>) -> Result<Vec<String>, String> {
    let gen_val = |k: &str| generated.iter().find_map(|l| l.split_once('=').filter(|(gk, _)| *gk == k).map(|(_, v)| v.to_string()));
    let mut out = vec![];
    for l in existing {
        let Some((k, v)) = l.split_once('=') else { out.push(l.clone()); continue };
        if !v.split(',').any(|p| p.trim() == "...") { out.push(l.clone()); continue }
        if !is_list_key(k) { return Err(format!("`...` is only allowed in list labels (middlewares, entryPoints), not in {k}")); }
        let g = gen_val(k).unwrap_or_default();
        let mut items: Vec<String> = vec![];
        for p in v.split(',').map(str::trim) {
            let chunk: Vec<&str> = if p == "..." { g.split(',').filter(|x| !x.is_empty()).collect() } else { vec![p] };
            for c in chunk { if !items.iter().any(|i| i == c) { items.push(c.to_string()); } }
        }
        out.push(format!("{k}={}", items.join(",")));
    }
    let have: Vec<&str> = existing.iter().map(|l| l.split('=').next().unwrap()).collect();
    out.extend(generated.into_iter().filter(|l| !have.contains(&l.split('=').next().unwrap())));
    Ok(out)
}

/// Sets the compose project name to the stack, so containers become `<stack>-<service>-<n>` and stay scalable.
fn set_project_name(doc: &mut Value, stack: &str) {
    doc.as_object_mut().unwrap().insert("name".into(), stack.into());
}

/// Joins the given services to the external `proxy` network (keeping `default`) under the unique alias `<stack>-<service>`.
fn attach_proxy(doc: &mut Value, stack: &str, svcs: &[String]) {
    for svc in svcs {
        let alias = serde_json::json!({ "aliases": [format!("{stack}-{svc}")] });
        let def = doc["services"][svc].as_object_mut().unwrap();
        // aliases need the map form, so list form is converted
        let mut map = match def.remove("networks") {
            None => serde_json::Map::from_iter([("default".to_string(), Value::Null)]),
            Some(Value::Array(a)) => a.into_iter().filter_map(|v| v.as_str().map(|k| (k.to_string(), Value::Null))).collect(),
            Some(Value::Object(m)) => m,
            Some(_) => continue,
        };
        map.insert("proxy".into(), alias);
        def.insert("networks".into(), Value::Object(map));
    }
    if !svcs.is_empty() {
        let root = doc.as_object_mut().unwrap();
        let nets = root.entry("networks").or_insert(Value::Object(Default::default()));
        // `networks: null` is valid compose; treat as empty
        if nets.is_null() { *nets = Value::Object(Default::default()); }
        nets.as_object_mut().unwrap().insert("proxy".into(), serde_json::json!({"external": true}));
    }
}

fn clone_decl(d: &Decl) -> Decl { Decl { url: d.url, id: d.id.clone(), full: d.full.clone(), port: d.port, domain: d.domain.clone() } }

#[cfg(test)]
mod tests {
    use super::*;

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
    fn referenced_vars_skips_defaults_and_escapes() {
        let v = super::referenced_vars("a ${A} ${B:-x} $C $$D ${E:?msg} ${F:+g} ${G-h} $1 ${H}");
        assert_eq!(v.into_iter().collect::<Vec<_>>(), ["A", "C", "E", "H"]);
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

    fn d(url: bool, id: &str, full: &str, port: u16, dom: Option<&str>) -> Decl {
        Decl { url, id: id.into(), full: full.into(), port, domain: dom.map(String::from) }
    }
    #[test]
    fn decl_forms() {
        let v: Value = serde_saphyr::from_str("- SERVICE_URL_VAULTWARDEN\n- 'A=b'\n- SERVICE_URL_AUTHENTIKSERVER_9000=auth.x").unwrap();
        assert_eq!(declarations(&v), vec![d(true, "VAULTWARDEN", "SERVICE_URL_VAULTWARDEN", 80, None),
            d(true, "AUTHENTIKSERVER", "SERVICE_URL_AUTHENTIKSERVER_9000", 9000, Some("auth.x"))]);
        let m: Value = serde_saphyr::from_str("SERVICE_FQDN_APP_3000: a.x\nX: y").unwrap();
        assert_eq!(declarations(&m), vec![d(false, "APP", "SERVICE_FQDN_APP_3000", 3000, Some("a.x"))]);
    }
    #[test]
    fn vaultwarden_matches_coolify() {
        let got = labels("STACK", "vaultwarden", "vault.schubs.net", 80);
        assert!(got.contains(&"traefik.http.routers.https-STACK-vaultwarden.tls.certresolver=letsencrypt".to_string()));
        assert_eq!(got.len(), 16);
    }
    #[test]
    fn generated_shapes() {
        assert_eq!(generate("SERVICE_PASSWORD_DB").unwrap().len(), 32);
        assert_eq!(generate("SERVICE_PASSWORD_64_DB").unwrap().len(), 64);
        assert_eq!(generate("SERVICE_USER_PG").unwrap().len(), 16);
        assert_eq!(generate("SERVICE_HEX_64_X").unwrap().len(), 64);
        assert_eq!(generate("SERVICE_REALBASE64_32_X").unwrap().len(), 44);
        assert!(generate("SERVICE_FQDN_APP").is_none());
        assert!(generate("SERVICE_PASSWORD_ZITADELDB").is_some());
    }
    #[test]
    fn proxy_network() {
        let mut v: Value = serde_saphyr::from_str("services:\n  a: {image: x}\n  b: {image: y, networks: [n1]}\n  c: {image: z, networks: {n1: {}}}").unwrap();
        attach_proxy(&mut v, "st", &["a".into(), "b".into(), "c".into()]);
        let al = serde_json::json!({"aliases": ["st-a"]});
        assert_eq!(v["services"]["a"]["networks"], serde_json::json!({"default": null, "proxy": al}));
        assert_eq!(v["services"]["b"]["networks"]["n1"], Value::Null);
        assert_eq!(v["services"]["b"]["networks"]["proxy"]["aliases"][0], "st-b");
        assert_eq!(v["services"]["c"]["networks"]["n1"], serde_json::json!({}));
        assert_eq!(v["networks"]["proxy"]["external"], true);
    }
    #[test]
    fn project_name() {
        let mut v: Value = serde_saphyr::from_str("name: other\nservices:\n  a: {image: x}").unwrap();
        set_project_name(&mut v, "st");
        assert_eq!(v["name"], "st");
    }
    #[test]
    fn label_merge() {
        let g = || vec!["a.middlewares=gzip,x".to_string(), "a.rule=Host(`h`)".to_string(), "a.port=80".to_string()];
        let m = |e: &str| merge_labels(&[e.to_string()], g());
        let get = |v: Vec<String>, k: &str| v.into_iter().find(|l| l.starts_with(k)).unwrap();
        assert_eq!(get(m("a.middlewares=auth").unwrap(), "a.middlewares"), "a.middlewares=auth");
        assert_eq!(get(m("a.middlewares=...,auth").unwrap(), "a.middlewares"), "a.middlewares=gzip,x,auth");
        assert_eq!(get(m("a.middlewares=auth,...").unwrap(), "a.middlewares"), "a.middlewares=auth,gzip,x");
        assert_eq!(get(m("a.middlewares=auth,...,last").unwrap(), "a.middlewares"), "a.middlewares=auth,gzip,x,last");
        assert_eq!(get(m("a.middlewares=gzip,...").unwrap(), "a.middlewares"), "a.middlewares=gzip,x");
        assert!(m("a.rule=...").is_err());
        assert_eq!(merge_labels(&["b.middlewares=...,z".into()], g()).unwrap()[0], "b.middlewares=z");
        assert_eq!(m("a.middlewares=auth").unwrap().iter().filter(|l| l.starts_with("a.middlewares")).count(), 1);
    }
    #[test]
    fn token_boundary() {
        assert_eq!(tokens("A_SERVICE_USER_X ${SERVICE_PASSWORD_B}"), vec!["SERVICE_PASSWORD_B".to_string()]);
    }
    #[test]
    fn stable_and_domains() {
        let text = "${SERVICE_PASSWORD_A} ${SERVICE_FQDN_APP}";
        let decls = vec![d(false, "APP", "SERVICE_FQDN_APP_3000", 3000, Some("a.x"))];
        let first = env_entries(text, &decls, &[]).unwrap();
        let second = env_entries(text, &decls, &first).unwrap();
        assert_eq!(first, second);
        assert!(first.contains(&("SERVICE_FQDN_APP".into(), "a.x".into())));
        assert!(env_entries(text, &[d(false, "APP", "SERVICE_FQDN_APP", 80, None)], &[]).is_err());
    }
}
