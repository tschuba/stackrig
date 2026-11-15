use serde_json::Value;
use crate::secrets::*;

/// Traefik labels for one service, modelled on what Coolify 4.x generated (see the overview note, spike findings).
pub(crate) fn labels(stack: &str, svc: &str, domain: &str, port: u16) -> Vec<String> {
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
#[derive(Debug, PartialEq, Clone)]
pub(crate) struct Decl { pub url: bool, pub id: String, pub full: String, pub port: u16, pub domain: Option<String> }

pub(crate) fn declarations(env: &Value) -> Vec<Decl> {
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

/// Renders `path` for `stack`: writes `.env.generated` next to it and returns the final compose YAML.
pub(crate) fn render(path: &str, stack: &str) -> String {
    let text = std::fs::read_to_string(path).unwrap();
    let mut doc: Value = serde_saphyr::from_str(&text).unwrap();
    let mut decls: Vec<(String, Decl)> = vec![];
    for (svc, def) in doc["services"].as_object().unwrap() {
        for d in declarations(&def["environment"]) { decls.push((svc.clone(), d)); }
    }
    let envp = std::path::Path::new(path).with_file_name(".env.generated");
    let existing = std::fs::read_to_string(&envp).map(|t| parse_env(&t)).unwrap_or_default();
    let entries = env_entries(&text, &decls.iter().map(|(_, d)| d.clone()).collect::<Vec<_>>(), &existing)
        .unwrap_or_else(|e| { eprintln!("error: {e}"); std::process::exit(1) });
    {
        use std::os::unix::fs::OpenOptionsExt;
        use std::io::Write;
        let mut f = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&envp).unwrap();
        for (k, v) in &entries { writeln!(f, "{k}={v}").unwrap(); }
    }
    for (svc, d) in &decls {
        let dom = entries.iter().find(|(k, _)| *k == d.full).unwrap().1.trim_start_matches("https://").to_string();
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

/// Renders every stack dir below `root` in its own process (one failure, e.g. a panic, must not stop the rest).
/// Prints one line per stack, returns 1 if any failed. Writes `.env.generated` like a single `render`.
pub(crate) fn render_all(root: &str) -> i32 {
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

/// Keys whose value is a comma list, so `...` (= the generated elements) may appear in them.
pub(crate) fn is_list_key(k: &str) -> bool { k.ends_with(".middlewares") || k.ends_with(".entryPoints") }

/// Merges generated labels into the file's own: same key and no `...` -> the file wins;
/// `...` inside a list value (start, middle or end) -> replaced by the generated elements.
pub(crate) fn merge_labels(existing: &[String], generated: Vec<String>) -> Result<Vec<String>, String> {
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
pub(crate) fn set_project_name(doc: &mut Value, stack: &str) {
    doc.as_object_mut().unwrap().insert("name".into(), stack.into());
}

/// Joins the given services to the external `proxy` network (keeping `default`) under the unique alias `<stack>-<service>`.
pub(crate) fn attach_proxy(doc: &mut Value, stack: &str, svcs: &[String]) {
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

#[cfg(test)]
mod tests {
    use super::*;

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
}
