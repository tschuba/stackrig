//! `strig domain add|rm <stack> <domain>`: edits `meta.yml` and the Traefik `Host(...)` rule of a stack, line-wise so comments survive.
use crate::{coolify::run_on, ops};

fn hosts(line: &str) -> Vec<String> { line.split('`').skip(1).step_by(2).map(String::from).collect() }

/// Rewrites the `...rule=Host(...)` label line with `h` added or removed (`Host(`a`) || Host(`b`)`).
fn set_rule(line: &str, h: &str, add: bool) -> Result<String, String> {
    let (pre, rule) = line.split_once("rule=").ok_or("no rule= in label")?;
    // shortcut: only plain `Host(..) || Host(..)` rules, anything else (&&, PathPrefix, HostRegexp) is refused; upgrade when such a stack needs domain edits
    if rule.contains("&&") || rule.contains("Regexp") || rule.contains("Path") { return Err(format!("unsupported rule `{rule}`: edit by hand")); }
    let mut v = hosts(rule);
    if add { if !v.iter().any(|x| x == h) { v.push(h.into()); } } else { v.retain(|x| x != h); }
    if v.is_empty() { return Err("that is the last domain of the service; remove the stack or add another first".into()); }
    Ok(format!("{pre}rule={}", v.iter().map(|x| format!("Host(`{x}`)")).collect::<Vec<_>>().join(" || ")))
}

/// Rewrites the `domains: [a, b]` line of meta.yml (or adds it).
pub(crate) fn set_meta(text: &str, h: &str, add: bool) -> Result<String, String> {
    let mut lines: Vec<String> = text.lines().map(String::from).collect();
    let i = lines.iter().position(|l| l.starts_with("domains:"));
    let mut v: Vec<String> = i.map_or(vec![], |i| {
        let l = &lines[i]["domains:".len()..];
        l.trim().trim_start_matches('[').trim_end_matches(']').split(',').map(|x| x.trim().to_string()).filter(|x| !x.is_empty()).collect()
    });
    if i.is_some_and(|i| !lines[i].contains('[')) { return Err("meta.yml: `domains:` must be a one-line [a, b] list".into()); }
    if add { v.push(h.into()); } else { v.retain(|x| x != h); }
    let new = format!("domains: [{}]", v.join(", "));
    match i { Some(i) => lines[i] = new, None => lines.push(new) }
    Ok(lines.join("\n") + "\n")
}

pub(crate) fn domain(a: &[String], dry: bool) -> Result<(), String> {
    let usage = || "usage: strig domain add|rm <stack> <domain|sub> [--dry-run]".to_string();
    let (Some(op @ ("add" | "rm")), Some(stack), Some(d)) = (a.first().map(String::as_str), a.get(1), a.get(2)) else { return Err(usage()) };
    let add = op == "add";
    let r = ops::root();
    let (stacks, _) = ops::load(&r)?;
    let s = stacks.iter().find(|s| &s.name == stack).ok_or(format!("unknown stack `{stack}`"))?;
    // a bare label (`wiki`) is completed with the parent of the stack's first domain
    let d = if d.contains('.') { d.clone() } else {
        let base = s.domains.first().and_then(|f| f.split_once('.')).ok_or("no existing domain to derive the parent from; pass the full domain")?.1;
        format!("{d}.{base}")
    };
    if !d.chars().all(|c| c.is_ascii_alphanumeric() || "-.".contains(c)) || d.starts_with(['.', '-']) { return Err(format!("bad domain `{d}`")); }
    if add {
        if let Some(o) = stacks.iter().find(|o| o.domains.contains(&d)) { return Err(format!("{d} is already used by stack `{}`", o.name)); }
    } else if !s.domains.contains(&d) { return Err(format!("{d} is not a domain of `{stack}`")); }
    let dir = r.join("stacks").join(stack);
    let cp = ["compose.yml", "compose.yaml", "docker-compose.yml"].iter().map(|f| dir.join(f)).find(|p| p.exists()).ok_or("no compose file")?;
    let text = std::fs::read_to_string(&cp).map_err(|e| e.to_string())?;
    // the service's rule: the one line whose hosts overlap the stack's domains (add: the only Host rule)
    let mut lines: Vec<String> = text.lines().map(String::from).collect();
    let cand: Vec<usize> = (0..lines.len()).filter(|&i| lines[i].contains("rule=Host(") && (add || hosts(&lines[i]).contains(&d))).collect();
    if cand.len() != 1 { return Err(format!("{} has {} Host rules{}; edit by hand", cp.display(), cand.len(), if add { " (add needs exactly one)" } else { " for that domain" })); }
    lines[cand[0]] = set_rule(&lines[cand[0]], &d, add)?;
    let (mp, new_compose) = (dir.join("meta.yml"), lines.join("\n") + if text.ends_with('\n') { "\n" } else { "" });
    let new_meta = set_meta(&std::fs::read_to_string(&mp).unwrap_or_default(), &d, add)?;
    if add {
        // DNS is only advisory: the domain may be set up afterwards
        let dns = run_on("local", &format!("dig +short {d}")).unwrap_or_default();
        if dns.trim().is_empty() { eprintln!("warning: {d} has no DNS record (yet)"); }
    }
    if dry { println!("dry run: {op} {d} on {stack}:\n{}", lines[cand[0]].trim()); return Ok(()); }
    std::fs::write(&cp, new_compose).map_err(|e| e.to_string())?;
    std::fs::write(&mp, new_meta).map_err(|e| e.to_string())?;
    println!("{op} {d} on {stack}: meta.yml and {} changed; run `strig deploy {stack}`", cp.file_name().unwrap().to_string_lossy());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rule_and_meta() {
        let l = "    - traefik.http.routers.h.rule=Host(`a.x`)";
        let two = set_rule(l, "b.x", true).unwrap();
        assert!(two.ends_with("rule=Host(`a.x`) || Host(`b.x`)"));
        assert_eq!(set_rule(&two, "a.x", false).unwrap(), "    - traefik.http.routers.h.rule=Host(`b.x`)");
        assert!(set_rule(l, "a.x", false).is_err());
        assert!(set_rule("x.rule=Host(`a`) && PathPrefix(`/`)", "b", true).is_err());
        assert_eq!(set_meta("# c\ndomains: [a.x]\n", "b.x", true).unwrap(), "# c\ndomains: [a.x, b.x]\n");
        assert_eq!(set_meta("domains: [a.x, b.x]\n", "a.x", false).unwrap(), "domains: [b.x]\n");
    }
}
