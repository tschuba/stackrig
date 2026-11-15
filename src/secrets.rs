use crate::render::Decl;

pub(crate) fn norm(s: &str) -> String { s.replace(['-', '.', '_'], "").to_uppercase() }

pub(crate) fn random(n: usize, alphabet: &[u8]) -> String {
    use std::io::Read;
    let mut b = vec![0u8; n];
    std::fs::File::open("/dev/urandom").unwrap().read_exact(&mut b).unwrap();
    // shortcut: modulo bias (<1 bit of entropy lost per char), fine for secrets here
    b.iter().map(|x| alphabet[*x as usize % alphabet.len()] as char).collect()
}

pub(crate) fn base64(bytes: &[u8]) -> String {
    const T: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    bytes.chunks(3).map(|c| {
        let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        (0..4).map(|i| if i <= c.len() { T[(n >> (18 - 6 * i) & 63) as usize] as char } else { '=' }).collect::<String>()
    }).collect()
}

/// Generates a value for a credential-type magic variable (per Coolify docs); None for URL/FQDN/unknown types.
pub(crate) fn generate(name: &str) -> Option<String> {
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
pub(crate) fn tokens(text: &str) -> Vec<String> {
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

pub(crate) fn parse_env(text: &str) -> Vec<(String, String)> {
    text.lines().filter_map(|l| l.split_once('=')).map(|(k, v)| (k.into(), v.into())).collect()
}

/// Existing `.env` values win; only missing credentials are generated, domain variables follow the declarations.
pub(crate) fn env_entries(text: &str, decls: &[Decl], existing: &[(String, String)]) -> Result<Vec<(String, String)>, String> {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn d(url: bool, id: &str, full: &str, port: u16, dom: Option<&str>) -> Decl {
        Decl { url, id: id.into(), full: full.into(), port, domain: dom.map(String::from) }
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
